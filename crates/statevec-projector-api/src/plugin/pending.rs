//! Mechanical ownership of one exact control request. The caller owns scheduling,
//! persistence/model installation, readiness and shutdown disposition.
use super::{ControlDecision, DurableOffsetReport, Offer, ProjectorControlPort,
    ProjectorError, SnapshotToken, ReplyHandle, ReplyPoll};
use crate::ProjectorControlRejection;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlProgress {
    Pending,
    Accepted,
    Rejected(ProjectorControlRejection),
    Closed,
}

trait Request: Copy {
    fn offer<'io>(&self, control: &ProjectorControlPort<'io>)
        -> Result<Offer<'io, ControlDecision>, ProjectorError>;
}
impl Request for DurableOffsetReport {
    fn offer<'io>(&self, control: &ProjectorControlPort<'io>)
        -> Result<Offer<'io, ControlDecision>, ProjectorError> {
        control.report_durable_offset(self)
    }
}
impl Request for SnapshotToken {
    fn offer<'io>(&self, control: &ProjectorControlPort<'io>)
        -> Result<Offer<'io, ControlDecision>, ProjectorError> {
        control.offer_snapshot_confirmation(self)
    }
}

enum Pending<'io, T> {
    Unqueued(T),
    Awaiting { value: T, reply: ReplyHandle<'io, ControlDecision> },
    Accepted,
    Rejected { value: T, cause: ProjectorControlRejection },
    Closed(T),
    Failed { value: T, cause: ProjectorError },
}
impl<'io, T: Request> Pending<'io, T> {
    fn value(&self) -> Option<T> {
        match self {
            Self::Unqueued(value) | Self::Closed(value) | Self::Awaiting { value, .. }
            | Self::Rejected { value, .. } | Self::Failed { value, .. } => Some(*value),
            Self::Accepted => None,
        }
    }
    fn progress(&mut self, control: &ProjectorControlPort<'io>) -> Result<ControlProgress, ProjectorError> {
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
/// constructing it. Drop releases reply ownership, not an already queued request.
#[must_use]
pub struct PendingDurableOffset<'io>(Pending<'io, DurableOffsetReport>);
impl<'io> PendingDurableOffset<'io> {
    pub fn new(report: DurableOffsetReport) -> Self { Self(Pending::Unqueued(report)) }
    /// None only after owner acceptance; terminal failures retain the declaration.
    pub fn report(&self) -> Option<DurableOffsetReport> { self.0.value() }
    /// At most one poll and one offer. Disconnection permits exact retry; queue
    /// refusal waits for a later call. Other failures/rejections are terminal.
    pub fn progress(&mut self, control: &ProjectorControlPort<'io>) -> Result<ControlProgress, ProjectorError> {
        self.0.progress(control)
    }
}

/// One caller-driven confirmation, constructed only after installing the model
/// associated with this EOF-derived token. It never claims a durable offset.
#[must_use]
pub struct PendingSnapshotConfirmation<'io>(Pending<'io, SnapshotToken>);
impl<'io> PendingSnapshotConfirmation<'io> {
    pub fn new(cut: SnapshotToken) -> Self { Self(Pending::Unqueued(cut)) }
    pub fn token(&self) -> Option<SnapshotToken> { self.0.value() }
    pub fn progress(&mut self, control: &ProjectorControlPort<'io>) -> Result<ControlProgress, ProjectorError> {
        self.0.progress(control)
    }
}
