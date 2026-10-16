//! Mechanical custody of one exact control request. The caller owns scheduling,
//! persistence/model installation, readiness and shutdown disposition.
use super::{ControlDecision, DurableOffsetReport, Offer, ProjectorControlPort,
    ProjectorFailure, RebuildCut, ReplyHandle, ReplyPoll};
use crate::ProjectorCheckpointRejection;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlProgress {
    Pending,
    Accepted,
    Rejected(ProjectorCheckpointRejection),
    Closed,
}

trait Request: Copy {
    fn offer<'io>(&self, control: &ProjectorControlPort<'io>)
        -> Result<Offer<'io, ControlDecision>, ProjectorFailure>;
}
impl Request for DurableOffsetReport {
    fn offer<'io>(&self, control: &ProjectorControlPort<'io>)
        -> Result<Offer<'io, ControlDecision>, ProjectorFailure> {
        control.report_durable_offset(self)
    }
}
impl Request for RebuildCut {
    fn offer<'io>(&self, control: &ProjectorControlPort<'io>)
        -> Result<Offer<'io, ControlDecision>, ProjectorFailure> {
        control.offer_rebuild(self)
    }
}

enum Pending<'io, T> {
    Unqueued(T),
    Awaiting { value: T, reply: ReplyHandle<'io, ControlDecision> },
    Accepted,
    Rejected { value: T, cause: ProjectorCheckpointRejection },
    Closed(T),
    Failed { value: T, cause: ProjectorFailure },
}
impl<'io, T: Request> Pending<'io, T> {
    fn value(&self) -> Option<T> {
        match self {
            Self::Unqueued(value) | Self::Closed(value) | Self::Awaiting { value, .. }
            | Self::Rejected { value, .. } | Self::Failed { value, .. } => Some(*value),
            Self::Accepted => None,
        }
    }
    fn progress(&mut self, control: &ProjectorControlPort<'io>) -> Result<ControlProgress, ProjectorFailure> {
        if let Self::Awaiting { value, reply } = self {
            let value = *value;
            let next = match reply.poll() {
                Ok(ReplyPoll::Pending) => return Ok(ControlProgress::Pending),
                Ok(ReplyPoll::Disconnected) => Self::Unqueued(value),
                Ok(ReplyPoll::Ready(ControlDecision::Accepted)) => Self::Accepted,
                Ok(ReplyPoll::Ready(ControlDecision::Rejected(cause))) => Self::Rejected { value, cause },
                Err(cause) => Self::Failed { value, cause },
            };
            // Drop the old reply before another offer, returning its physical
            // slot. A release failure remains the scope's independent cause.
            *self = next;
        }
        if let Self::Unqueued(value) = self {
            let value = *value;
            match value.offer(control) {
                Ok(Offer::Queued(reply)) => *self = Self::Awaiting { value, reply },
                Ok(Offer::OwnerQueueFull | Offer::ReplyCapacityExhausted) => (),
                Ok(Offer::Closed) => *self = Self::Closed(value),
                Err(cause) => *self = Self::Failed { value, cause },
            }
        }
        match self {
            Self::Unqueued(_) | Self::Awaiting { .. } => Ok(ControlProgress::Pending),
            Self::Accepted => Ok(ControlProgress::Accepted),
            Self::Rejected { cause, .. } => Ok(ControlProgress::Rejected(*cause)),
            Self::Closed(_) => Ok(ControlProgress::Closed),
            Self::Failed { cause, .. } => Err(*cause),
        }
    }
}

/// One caller-driven durability report. Persist effects and offset before
/// constructing it. Drop releases reply custody, not an already queued request.
#[must_use]
pub struct PendingDurableOffset<'io>(Pending<'io, DurableOffsetReport>);
impl<'io> PendingDurableOffset<'io> {
    pub fn new(report: DurableOffsetReport) -> Self { Self(Pending::Unqueued(report)) }
    /// None only after owner acceptance; terminal failures retain the declaration.
    pub fn report(&self) -> Option<DurableOffsetReport> { self.0.value() }
    /// At most one poll and one offer. Disconnection permits exact retry; queue
    /// refusal waits for a later call. Other failures/rejections are terminal.
    pub fn progress(&mut self, control: &ProjectorControlPort<'io>) -> Result<ControlProgress, ProjectorFailure> {
        self.0.progress(control)
    }
}

/// One caller-driven confirmation, constructed only after installing the model
/// associated with this EOF-derived cut. It never claims a durable offset.
#[must_use]
pub struct PendingRebuildConfirmation<'io>(Pending<'io, RebuildCut>);
impl<'io> PendingRebuildConfirmation<'io> {
    pub fn new(cut: RebuildCut) -> Self { Self(Pending::Unqueued(cut)) }
    pub fn cut(&self) -> Option<RebuildCut> { self.0.value() }
    pub fn progress(&mut self, control: &ProjectorControlPort<'io>) -> Result<ControlProgress, ProjectorFailure> {
        self.0.progress(control)
    }
}
