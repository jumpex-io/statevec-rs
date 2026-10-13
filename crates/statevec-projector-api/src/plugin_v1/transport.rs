use super::{ffi::*, ProjectorAbiFailure as A};
use crate::{ProjectorCheckpointRejection as R, ProjectorCheckpointStatus as S, ProjectorRecovery};
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
        if self.tag != 1 && self.offset != SvConsumeOffset::default() { return Err(A::InvalidHandle); }
        match self.tag {
            0 => Ok(ProjectorRecovery::Continuous { offset: None }),
            1 => Ok(ProjectorRecovery::Continuous { offset: Some(self.offset.decode()?) }),
            2 => Ok(ProjectorRecovery::LatestState),
            actual => Err(A::Flag { actual }),
        }
    }
    pub fn encode(value: &ProjectorRecovery) -> Result<Self, A> {
        match value {
            ProjectorRecovery::Continuous { offset: None } => Ok(Self::default()),
            ProjectorRecovery::Continuous { offset: Some(offset) } => Ok(Self { tag: 1, reserved: 0, offset: SvConsumeOffset::encode(offset).map_err(|_| A::InvalidHandle)? }),
            ProjectorRecovery::LatestState => Ok(Self { tag: 2, ..Self::default() }),
            ProjectorRecovery::ReplayOnly => Err(A::UnsupportedCapabilities { required: CONTINUOUS_HISTORY_CAPABILITY }),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlReply { Pending, Accepted, Rejected(R), Status(S), Disconnected }

impl SvControlReply {
    pub fn decode(self) -> Result<ControlReply, A> {
        let Self { tag, cause, value0: a, value1: b } = self;
        if matches!(tag, 0 | 1 | 4) && (cause != 0 || a != 0 || b != 0) { return Err(A::Reserved { actual: u64::from(cause) | a | b }); }
        if tag == 2 && cause != 5 && (a != 0 || b != 0) { return Err(A::Reserved { actual: a | b }); }
        if tag == 3 && cause != 4 && b != 0 { return Err(A::Reserved { actual: b }); }
        if tag == 3 && cause <= 1 && a != 0 { return Err(A::Reserved { actual: a }); }
        Ok(match tag {
            0 => ControlReply::Pending, 1 => ControlReply::Accepted, 4 => ControlReply::Disconnected,
            2 => ControlReply::Rejected(match cause {
                0 => R::NotAttached, 1 => R::CheckpointsDisabled, 2 => R::RestoreNotVerified, 3 => R::OwnerNotRunning,
                4 => R::IdentityMismatch, 5 => R::BeyondApplied { applied: a, projector: b }, 6 => R::OffsetRegressed,
                7 => R::OffsetConflict, 8 => R::WrongRecoveryMode, 9 => R::ProjectorIdentityMismatch,
                10 => R::NotRebuilding, 11 => R::RebuildNotComplete, 12 => R::RebuildMismatch,
                actual => return Err(A::Flag { actual }),
            }),
            3 => ControlReply::Status(match cause {
                0 => S::Disabled, 1 => S::ReplayOnly,
                2 if a != 0 => S::Recovering { projector_raft_index: a },
                3 => S::Active { durable_raft_index: a, waiting_for_projector: None },
                4 if b > a => S::Active { durable_raft_index: a, waiting_for_projector: Some(b) },
                5 => S::Rebuilding { target_raft_index: a }, 6 => S::LatestStateActive { baseline_raft_index: a },
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
            ControlReply::Pending => {}, ControlReply::Accepted => out.tag = 1, ControlReply::Disconnected => out.tag = 4,
            ControlReply::Rejected(value) => {
                out.tag = 2;
                out.cause = match value {
                    R::NotAttached => 0, R::CheckpointsDisabled => 1, R::RestoreNotVerified => 2, R::OwnerNotRunning => 3, R::IdentityMismatch => 4,
                    R::BeyondApplied { applied, projector } => { out.value0 = applied; out.value1 = projector; 5 }
                    R::OffsetRegressed => 6, R::OffsetConflict => 7, R::WrongRecoveryMode => 8, R::ProjectorIdentityMismatch => 9,
                    R::NotRebuilding => 10, R::RebuildNotComplete => 11, R::RebuildMismatch => 12,
                };
            }
            ControlReply::Status(value) => {
                out.tag = 3;
                out.cause = match value {
                    S::Disabled => 0, S::ReplayOnly => 1,
                    S::Recovering { projector_raft_index } => { out.value0 = projector_raft_index; 2 }
                    S::Active { durable_raft_index, waiting_for_projector } => {
                        out.value0 = durable_raft_index;
                        match waiting_for_projector { None => 3, Some(index) => { out.value1 = index; 4 } }
                    }
                    S::Rebuilding { target_raft_index } => { out.value0 = target_raft_index; 5 }
                    S::LatestStateActive { baseline_raft_index } => { out.value0 = baseline_raft_index; 6 }
                };
            }
        }
        out
    }
}
