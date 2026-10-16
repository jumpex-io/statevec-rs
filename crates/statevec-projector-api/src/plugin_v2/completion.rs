use super::{
    decode_failure, ProjectorAbiFailure as A, ProjectorCallFailure as C,
    ProjectorFailure as F, ProjectorWorkerFailure as W, SvStatus,
    ffi::{SvProjectorCompletion, SvProjectorFailureObservation},
};

/// Wire slot numbers are one-based; completion array indices are zero-based.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub(super) enum FailureSlot {
    Run = 1, FrameRelease = 2, StateRelease = 3, ReplyRelease = 4,
    ServiceJoin = 5, Notification = 6, WorkerJoin = 7,
}
impl FailureSlot {
    const ALL: [Self; 7] = [Self::Run, Self::FrameRelease, Self::StateRelease,
        Self::ReplyRelease, Self::ServiceJoin, Self::Notification, Self::WorkerJoin];
    pub const fn code(self) -> u32 { self as u32 }
    pub const fn index(self) -> usize { self as usize - 1 }
}

/// Validated observations only. `joined` is a report from the exporter, not a
/// replacement for the host's actual invocation/custody checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProjectorCompletion {
    pub joined: bool,
    pub run_failure: Option<F>,
    pub frame_release_failure: Option<F>,
    pub state_release_failure: Option<F>,
    pub reply_release_failure: Option<F>,
    pub notification_failure: Option<F>,
    pub worker_join_panicked: bool,
    pub failed_service_joins: u64,
}

pub fn decode_observation(raw: SvProjectorFailureObservation) -> Result<Option<W>, A> {
    if raw.reserved != 0 { return Err(A::Reserved { actual: raw.reserved.into() }); }
    if raw.slot == 0 {
        if raw.cause.is_success() { return Ok(None); }
        return Err(A::Flag { actual: raw.slot });
    }
    let failure = decode_failure(raw.cause)?.ok_or(A::Flag { actual: raw.slot })?;
    let release = matches!(failure, C::Operation(F::Host(_) | F::HostPanicked | F::AbiContract(_)));
    match (raw.slot, failure) {
        (1, C::Operation(source)) => Ok(Some(W::Run(source))),
        (2, C::Operation(source)) if release => Ok(Some(W::FrameRelease(source))),
        (3, C::Operation(source)) if release => Ok(Some(W::StateRelease(source))),
        (4, C::Operation(source)) if release => Ok(Some(W::ReplyRelease(source))),
        (5, C::ServiceJoinPanicked) => Ok(Some(W::ServiceJoinPanicked)),
        (6, C::Operation(source)) if release => Ok(Some(W::Notification(source))),
        (7, C::WorkerJoinPanicked) => Ok(Some(W::WorkerJoinPanicked)),
        _ => Err(A::Flag { actual: raw.slot }),
    }
}

impl SvProjectorCompletion {
    /// Preserve each origin independently. An earlier release observation may
    /// precede (but must not erase or conflict with) a later Run failure.
    pub fn decode(self, observed: Option<W>) -> Result<ProjectorCompletion, A> {
        if self.reserved != 0 { return Err(A::Reserved { actual: self.reserved.into() }); }
        if self.joined > 1 { return Err(A::Flag { actual: self.joined }); }
        let slots = [self.run_failure, self.frame_release_failure, self.state_release_failure,
            self.reply_release_failure, self.service_join_failure, self.notification_failure, self.worker_join_failure];
        let mut decoded = [None; 7];
        for (slot, cause) in FailureSlot::ALL.into_iter().zip(slots) {
            let index = slot.index();
            if !cause.is_success() {
                decoded[index] = decode_observation(SvProjectorFailureObservation { slot: slot.code(), reserved: 0, cause })?;
            }
        }
        if decoded[FailureSlot::ServiceJoin.index()].is_some() != (self.failed_service_joins != 0) {
            return Err(A::CallbackCount { expected: u64::from(decoded[FailureSlot::ServiceJoin.index()].is_some()), actual: self.failed_service_joins });
        }
        if let Some(prior) = observed
            && !decoded.contains(&Some(prior)) { return Err(A::InvalidHandle); }
        if self.joined == 0 && [FailureSlot::FrameRelease, FailureSlot::StateRelease, FailureSlot::ReplyRelease, FailureSlot::ServiceJoin, FailureSlot::WorkerJoin].into_iter().all(|slot| decoded[slot.index()].is_none()) {
            return Err(A::Flag { actual: self.joined });
        }
        Ok(ProjectorCompletion {
            joined: self.joined == 1,
            run_failure: match decoded[FailureSlot::Run.index()] { Some(W::Run(f)) => Some(f), _ => None },
            frame_release_failure: match decoded[FailureSlot::FrameRelease.index()] { Some(W::FrameRelease(f)) => Some(f), _ => None },
            state_release_failure: match decoded[FailureSlot::StateRelease.index()] { Some(W::StateRelease(f)) => Some(f), _ => None },
            reply_release_failure: match decoded[FailureSlot::ReplyRelease.index()] { Some(W::ReplyRelease(f)) => Some(f), _ => None },
            notification_failure: match decoded[FailureSlot::Notification.index()] { Some(W::Notification(f)) => Some(f), _ => None },
            worker_join_panicked: decoded[FailureSlot::WorkerJoin.index()].is_some(), failed_service_joins: self.failed_service_joins,
        })
    }
}

impl From<&W> for SvProjectorFailureObservation {
    fn from(source: &W) -> Self {
        let (slot, cause) = match source {
            W::Run(f) => (FailureSlot::Run, f.into()), W::FrameRelease(f) => (FailureSlot::FrameRelease, f.into()),
            W::StateRelease(f) => (FailureSlot::StateRelease, f.into()), W::ReplyRelease(f) => (FailureSlot::ReplyRelease, f.into()),
            W::ServiceJoinPanicked => (FailureSlot::ServiceJoin, SvStatus::from(&C::ServiceJoinPanicked)),
            W::Notification(f) => (FailureSlot::Notification, f.into()),
            W::WorkerJoinPanicked => (FailureSlot::WorkerJoin, SvStatus::from(&C::WorkerJoinPanicked)),
        };
        Self { slot: slot.code(), reserved: 0, cause }
    }
}
