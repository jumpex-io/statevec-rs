use super::{ffi::*, ProjectorAbiFailure as A};
use crate::{ProjectorCheckpointRejection as R, ProjectorCheckpointStatus as S, plugin_v2::ProjectorRecovery};
use std::num::NonZeroUsize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeliveryLimits {
    pub frame_capacity: NonZeroUsize,
    pub maximum_accounted_bytes: NonZeroUsize,
}

impl SvDeliveryLimits {
    pub fn decode(self) -> Result<DeliveryLimits, A> {
        let checked = |value| usize::try_from(value).ok().and_then(NonZeroUsize::new)
            .ok_or(A::Length { actual: value, maximum: usize::MAX as u64 });
        Ok(DeliveryLimits { frame_capacity: checked(self.frame_capacity)?, maximum_accounted_bytes: checked(self.maximum_accounted_bytes)? })
    }
}
impl From<DeliveryLimits> for SvDeliveryLimits {
    fn from(value: DeliveryLimits) -> Self { Self { frame_capacity: value.frame_capacity.get() as u64, maximum_accounted_bytes: value.maximum_accounted_bytes.get() as u64 } }
}
impl SvRecovery {
    pub fn decode(self) -> Result<ProjectorRecovery, A> {
        if self.reserved != 0 { return Err(A::Reserved { actual: self.reserved.into() }); }
        if self.tag != recovery_tag::HISTORY_FROM_OFFSET && self.offset != SvConsumeOffset::default() {
            let bytes = self.offset.cluster.iter().chain(&self.offset.projector_id.bytes)
                .fold(0u64, |value, byte| value | u64::from(*byte));
            return Err(A::Reserved { actual: bytes | self.offset.projector_id.len | self.offset.raft_term | self.offset.raft_index });
        }
        match self.tag {
            recovery_tag::HISTORY_FROM_GENESIS => Ok(ProjectorRecovery::ContinuousHistory { offset: None }),
            recovery_tag::HISTORY_FROM_OFFSET => Ok(ProjectorRecovery::ContinuousHistory { offset: Some(self.offset.decode()?) }),
            recovery_tag::LATEST_STATE => Ok(ProjectorRecovery::LatestStateOnly),
            actual => Err(A::Flag { actual }),
        }
    }
    pub fn encode(value: &ProjectorRecovery) -> Result<Self, A> {
        match value {
            ProjectorRecovery::ContinuousHistory { offset: None } => Ok(Self { tag: recovery_tag::HISTORY_FROM_GENESIS, ..Self::default() }),
            ProjectorRecovery::ContinuousHistory { offset: Some(offset) } => Ok(Self { tag: recovery_tag::HISTORY_FROM_OFFSET, reserved: 0, offset: SvConsumeOffset::encode(offset).map_err(|_| A::InvalidHandle)? }),
            ProjectorRecovery::LatestStateOnly => Ok(Self { tag: recovery_tag::LATEST_STATE, ..Self::default() }),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlReply { Pending, Accepted, Rejected(R), Status(S), Disconnected }

impl SvControlReply {
    pub fn decode(self) -> Result<ControlReply, A> {
        let Self { tag, cause, value0: a, value1: b } = self;
        if matches!(tag, reply_tag::PENDING | reply_tag::ACCEPTED | reply_tag::DISCONNECTED) && (cause != 0 || a != 0 || b != 0) { return Err(A::Reserved { actual: u64::from(cause) | a | b }); }
        if tag == reply_tag::REJECTED && cause != rejection_reason::BEYOND_APPLIED && (a != 0 || b != 0) { return Err(A::Reserved { actual: a | b }); }
        if tag == reply_tag::STATUS && cause != status_kind::BLOCKED && b != 0 { return Err(A::Reserved { actual: b }); }
        if tag == reply_tag::STATUS && matches!(cause, status_kind::DISABLED | status_kind::REPLAY_ONLY) && a != 0 { return Err(A::Reserved { actual: a }); }
        Ok(match tag {
            reply_tag::PENDING => ControlReply::Pending, reply_tag::ACCEPTED => ControlReply::Accepted, reply_tag::DISCONNECTED => ControlReply::Disconnected,
            reply_tag::REJECTED => ControlReply::Rejected(match cause {
                rejection_reason::NOT_ATTACHED => R::NotAttached, rejection_reason::CHECKPOINTS_DISABLED => R::CheckpointsDisabled, rejection_reason::RESTORE_NOT_VERIFIED => R::RestoreNotVerified, rejection_reason::OWNER_NOT_RUNNING => R::OwnerNotRunning,
                rejection_reason::IDENTITY_MISMATCH => R::IdentityMismatch, rejection_reason::BEYOND_APPLIED => R::BeyondApplied { applied: a, projector: b }, rejection_reason::OFFSET_REGRESSED => R::OffsetRegressed,
                rejection_reason::OFFSET_CONFLICT => R::OffsetConflict, rejection_reason::WRONG_RECOVERY_MODE => R::WrongRecoveryMode, rejection_reason::PROJECTOR_IDENTITY_MISMATCH => R::ProjectorIdentityMismatch,
                rejection_reason::NOT_REBUILDING => R::NotRebuilding, rejection_reason::REBUILD_NOT_COMPLETE => R::RebuildNotComplete, rejection_reason::REBUILD_MISMATCH => R::RebuildMismatch,
                actual => return Err(A::Flag { actual }),
            }),
            reply_tag::STATUS => ControlReply::Status(match cause {
                status_kind::DISABLED => S::Disabled, status_kind::REPLAY_ONLY => S::ReplayOnly,
                status_kind::RECOVERING if a != 0 => S::Recovering { projector_raft_index: a },
                status_kind::ACTIVE => S::Active { durable_raft_index: a, engine_checkpoint_blocked_at: None },
                status_kind::BLOCKED if b > a => S::Active { durable_raft_index: a, engine_checkpoint_blocked_at: Some(b) },
                status_kind::REBUILDING => S::Rebuilding { target_raft_index: a }, status_kind::LATEST_ACTIVE => S::LatestStateActive { baseline_raft_index: a },
                actual => return Err(A::Flag { actual }),
            }),
            actual => return Err(A::Flag { actual }),
        })
    }
}
impl From<ControlReply> for SvControlReply {
    fn from(value: ControlReply) -> Self {
        let mut out = Self::default();
        match value {
            ControlReply::Pending => out.tag = reply_tag::PENDING, ControlReply::Accepted => out.tag = reply_tag::ACCEPTED, ControlReply::Disconnected => out.tag = reply_tag::DISCONNECTED,
            ControlReply::Rejected(value) => {
                out.tag = reply_tag::REJECTED;
                out.cause = match value {
                    R::NotAttached => rejection_reason::NOT_ATTACHED, R::CheckpointsDisabled => rejection_reason::CHECKPOINTS_DISABLED, R::RestoreNotVerified => rejection_reason::RESTORE_NOT_VERIFIED, R::OwnerNotRunning => rejection_reason::OWNER_NOT_RUNNING, R::IdentityMismatch => rejection_reason::IDENTITY_MISMATCH,
                    R::BeyondApplied { applied, projector } => { out.value0 = applied; out.value1 = projector; rejection_reason::BEYOND_APPLIED }
                    R::OffsetRegressed => rejection_reason::OFFSET_REGRESSED, R::OffsetConflict => rejection_reason::OFFSET_CONFLICT, R::WrongRecoveryMode => rejection_reason::WRONG_RECOVERY_MODE, R::ProjectorIdentityMismatch => rejection_reason::PROJECTOR_IDENTITY_MISMATCH,
                    R::NotRebuilding => rejection_reason::NOT_REBUILDING, R::RebuildNotComplete => rejection_reason::REBUILD_NOT_COMPLETE, R::RebuildMismatch => rejection_reason::REBUILD_MISMATCH,
                };
            }
            ControlReply::Status(value) => {
                out.tag = reply_tag::STATUS;
                out.cause = match value {
                    S::Disabled => status_kind::DISABLED, S::ReplayOnly => status_kind::REPLAY_ONLY,
                    S::Recovering { projector_raft_index } => { out.value0 = projector_raft_index; status_kind::RECOVERING }
                    S::Active { durable_raft_index, engine_checkpoint_blocked_at } => {
                        out.value0 = durable_raft_index;
                        match engine_checkpoint_blocked_at { None => status_kind::ACTIVE, Some(index) => { out.value1 = index; status_kind::BLOCKED } }
                    }
                    S::Rebuilding { target_raft_index } => { out.value0 = target_raft_index; status_kind::REBUILDING }
                    S::LatestStateActive { baseline_raft_index } => { out.value0 = baseline_raft_index; status_kind::LATEST_ACTIVE }
                };
            }
        }
        out
    }
}
