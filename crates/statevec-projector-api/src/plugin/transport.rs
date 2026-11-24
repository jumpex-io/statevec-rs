use super::ProjectorAbiError as A;
use super::ffi::*;
use crate::plugin::StartRequest;
use crate::{ProjectorControlRejection as R, ProjectorStatus as S};
use std::num::NonZeroUsize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct DeliveryLimits {
    pub frame_capacity: NonZeroUsize,
    pub maximum_accounted_bytes: NonZeroUsize,
}

impl DeliveryLimits {
    pub const fn new(frame_capacity: NonZeroUsize, maximum_accounted_bytes: NonZeroUsize) -> Self {
        Self { frame_capacity, maximum_accounted_bytes }
    }
}

impl SvDeliveryLimits {
    pub fn decode(self) -> Result<DeliveryLimits, A> {
        let checked = |value| usize::try_from(value).ok().and_then(NonZeroUsize::new)
            .ok_or(A::Length { actual: value, maximum: usize::MAX as u64 });
        Ok(DeliveryLimits::new(checked(self.frame_capacity)?, checked(self.maximum_accounted_bytes)?))
    }
}
impl From<DeliveryLimits> for SvDeliveryLimits {
    fn from(value: DeliveryLimits) -> Self { Self { frame_capacity: value.frame_capacity.get() as u64, maximum_accounted_bytes: value.maximum_accounted_bytes.get() as u64 } }
}
impl SvRecovery {
    pub fn decode(self) -> Result<StartRequest, A> {
        if self.reserved != 0 { return Err(A::Reserved { actual: self.reserved.into() }); }
        if self.tag != recovery_tag::HISTORY_FROM_OFFSET && self.offset != SvConsumeOffset::default() {
            let bytes = self.offset.cluster.iter().chain(&self.offset.projector_id.bytes)
                .fold(0u64, |value, byte| value | u64::from(*byte));
            return Err(A::Reserved { actual: bytes | self.offset.projector_id.len | self.offset.raft_term | self.offset.raft_index });
        }
        match self.tag {
            recovery_tag::HISTORY_FROM_GENESIS => Ok(StartRequest::Continuous { offset: None }),
            recovery_tag::HISTORY_FROM_OFFSET => Ok(StartRequest::Continuous { offset: Some(self.offset.decode()?) }),
            recovery_tag::LATEST_STATE => Ok(StartRequest::LatestState),
            actual => Err(A::Flag { actual }),
        }
    }
    pub fn encode(value: &StartRequest) -> Result<Self, A> {
        match value {
            StartRequest::Continuous { offset: None } => Ok(Self { tag: recovery_tag::HISTORY_FROM_GENESIS, ..Self::default() }),
            StartRequest::Continuous { offset: Some(offset) } => Ok(Self { tag: recovery_tag::HISTORY_FROM_OFFSET, reserved: 0, offset: SvConsumeOffset::encode(offset).map_err(|_| A::InvalidHandle)? }),
            StartRequest::LatestState => Ok(Self { tag: recovery_tag::LATEST_STATE, ..Self::default() }),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlReply { Pending, Accepted, Rejected(R), Status(S), Disconnected }

impl SvControlReply {
    #[doc(hidden)]
    pub fn decode(self) -> Result<ControlReply, A> {
        let Self { tag, cause, value0, value1 } = self;
        match tag {
            reply_tag::PENDING | reply_tag::ACCEPTED | reply_tag::DISCONNECTED
                if cause != 0 || value0 != 0 || value1 != 0 =>
            {
                Err(A::Reserved { actual: u64::from(cause) | value0 | value1 })
            }
            reply_tag::PENDING => Ok(ControlReply::Pending),
            reply_tag::ACCEPTED => Ok(ControlReply::Accepted),
            reply_tag::DISCONNECTED => Ok(ControlReply::Disconnected),
            reply_tag::REJECTED => decode_rejection(cause, value0, value1).map(ControlReply::Rejected),
            reply_tag::STATUS => decode_status(cause, value0, value1).map(ControlReply::Status),
            actual => Err(A::Flag { actual }),
        }
    }
}

fn decode_rejection(cause: u32, value0: u64, value1: u64) -> Result<R, A> {
    let rejection = match cause {
        rejection_reason::NOT_ATTACHED => R::NotAttached,
        rejection_reason::CHECKPOINTS_DISABLED => R::CheckpointsDisabled,
        rejection_reason::RESTORE_NOT_VERIFIED => R::RestoreNotVerified,
        rejection_reason::OWNER_NOT_RUNNING => R::OwnerNotRunning,
        rejection_reason::IDENTITY_MISMATCH => R::IdentityMismatch,
        rejection_reason::BEYOND_APPLIED => R::BeyondApplied { applied: value0, projector: value1 },
        rejection_reason::OFFSET_REGRESSED => R::OffsetRegressed,
        rejection_reason::OFFSET_CONFLICT => R::OffsetConflict,
        rejection_reason::WRONG_RECOVERY_MODE => R::WrongHistoryMode,
        rejection_reason::PROJECTOR_IDENTITY_MISMATCH => R::ProjectorIdentityMismatch,
        rejection_reason::NOT_REBUILDING => R::NotRebuilding,
        rejection_reason::REBUILD_NOT_COMPLETE => R::RebuildNotComplete,
        rejection_reason::REBUILD_MISMATCH => R::RebuildMismatch,
        observed => return Ok(R::Unrecognized { observed, value0, value1 }),
    };
    if cause != rejection_reason::BEYOND_APPLIED && (value0 != 0 || value1 != 0) {
        return Err(A::Reserved { actual: value0 | value1 });
    }
    Ok(rejection)
}

fn decode_status(cause: u32, value0: u64, value1: u64) -> Result<S, A> {
    let status = match cause {
        status_kind::DISABLED => S::Disabled,
        status_kind::REPLAY_ONLY => S::ReplayOnly,
        status_kind::RECOVERING => S::Recovering { projector_raft_index: value0 },
        status_kind::ACTIVE => S::Active { durable_raft_index: value0, engine_checkpoint_blocked_at: None },
        status_kind::BLOCKED => {
            S::Active { durable_raft_index: value0, engine_checkpoint_blocked_at: Some(value1) }
        }
        status_kind::REBUILDING => S::Rebuilding { target_raft_index: value0 },
        status_kind::LATEST_ACTIVE => S::LatestStateActive { baseline_raft_index: value0 },
        observed => return Ok(S::Unrecognized { observed, value0, value1 }),
    };
    if cause != status_kind::BLOCKED && value1 != 0 {
        return Err(A::Reserved { actual: value1 });
    }
    if matches!(cause, status_kind::DISABLED | status_kind::REPLAY_ONLY) && value0 != 0 {
        return Err(A::Reserved { actual: value0 });
    }
    if (cause == status_kind::RECOVERING && value0 == 0) || (cause == status_kind::BLOCKED && value1 <= value0) {
        return Err(A::Flag { actual: cause });
    }
    Ok(status)
}

/// Encode assigned causes using their named variants. An `Unrecognized` value
/// forwards the original cause and payload verbatim, so its code must be
/// unassigned in this SDK. The host constructs its replies from named causes.
#[doc(hidden)]
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
                    R::OffsetRegressed => rejection_reason::OFFSET_REGRESSED, R::OffsetConflict => rejection_reason::OFFSET_CONFLICT, R::WrongHistoryMode => rejection_reason::WRONG_RECOVERY_MODE, R::ProjectorIdentityMismatch => rejection_reason::PROJECTOR_IDENTITY_MISMATCH,
                    R::NotRebuilding => rejection_reason::NOT_REBUILDING, R::RebuildNotComplete => rejection_reason::REBUILD_NOT_COMPLETE, R::RebuildMismatch => rejection_reason::REBUILD_MISMATCH,
                    R::Unrecognized { observed, value0, value1 } => {
                        out.value0 = value0;
                        out.value1 = value1;
                        observed
                    }
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
                    S::Unrecognized { observed, value0, value1 } => {
                        out.value0 = value0;
                        out.value1 = value1;
                        observed
                    }
                };
            }
        }
        out
    }
}
