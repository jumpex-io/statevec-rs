use super::{
    decode_failure, ProjectorAbiFailure as A, ProjectorCallFailure as C,
    ProjectorFailure as F, ProjectorWorkerFailure as W, SvStatus,
    ffi::{SvProjectorCompletion, SvProjectorFailureObservation},
};

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
    let release = matches!(failure, C::Failure(F::Host(_) | F::HostPanicked | F::AbiContract(_)));
    match (raw.slot, failure) {
        (1, C::Failure(source)) => Ok(Some(W::Run(source))),
        (2, C::Failure(source)) if release => Ok(Some(W::FrameRelease(source))),
        (3, C::Failure(source)) if release => Ok(Some(W::StateRelease(source))),
        (4, C::Failure(source)) if release => Ok(Some(W::ReplyRelease(source))),
        (5, C::ServiceJoinPanicked) => Ok(Some(W::ServiceJoinPanicked)),
        (6, C::Failure(source)) if release => Ok(Some(W::Notification(source))),
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
        for (index, cause) in slots.into_iter().enumerate() {
            if !cause.is_success() {
                decoded[index] = decode_observation(SvProjectorFailureObservation { slot: index as u32 + 1, reserved: 0, cause })?;
            }
        }
        if decoded[4].is_some() != (self.failed_service_joins != 0) {
            return Err(A::CallbackCount { expected: u64::from(decoded[4].is_some()), actual: self.failed_service_joins });
        }
        if let Some(prior) = observed
            && !decoded.contains(&Some(prior)) { return Err(A::InvalidHandle); }
        if self.joined == 0 && [1, 2, 3, 4, 6].into_iter().all(|slot| decoded[slot].is_none()) {
            return Err(A::Flag { actual: self.joined });
        }
        Ok(ProjectorCompletion {
            joined: self.joined == 1,
            run_failure: match decoded[0] { Some(W::Run(f)) => Some(f), _ => None },
            frame_release_failure: match decoded[1] { Some(W::FrameRelease(f)) => Some(f), _ => None },
            state_release_failure: match decoded[2] { Some(W::StateRelease(f)) => Some(f), _ => None },
            reply_release_failure: match decoded[3] { Some(W::ReplyRelease(f)) => Some(f), _ => None },
            notification_failure: match decoded[5] { Some(W::Notification(f)) => Some(f), _ => None },
            worker_join_panicked: decoded[6].is_some(), failed_service_joins: self.failed_service_joins,
        })
    }
}

impl From<&W> for SvProjectorFailureObservation {
    fn from(source: &W) -> Self {
        let (slot, cause) = match source {
            W::Run(f) => (1, f.into()), W::FrameRelease(f) => (2, f.into()),
            W::StateRelease(f) => (3, f.into()), W::ReplyRelease(f) => (4, f.into()),
            W::ServiceJoinPanicked => (5, SvStatus::from(&C::ServiceJoinPanicked)),
            W::Notification(f) => (6, f.into()),
            W::WorkerJoinPanicked => (7, SvStatus::from(&C::WorkerJoinPanicked)),
        };
        Self { slot, reserved: 0, cause }
    }
}
