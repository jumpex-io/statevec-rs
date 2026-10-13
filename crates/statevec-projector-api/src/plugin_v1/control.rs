use super::{borrow::call, ffi::*, io::{Held, ScopeData}, ProjectorAbiFailure as A, ProjectorFailure as F, ProjectorOperation as Op, ControlReply};
use crate::{ConsumeOffset, ProjectorCheckpointRejection as Rejection, ProjectorCheckpointStatus as Status};
use std::{cell::Cell, marker::PhantomData};

#[derive(Debug, Clone, Copy)]
pub struct CheckpointCut { pub(super) raw: SvCut, pub(super) position: SvPosition, pub(super) projector_id: crate::ProjectorId }
#[derive(Debug, Clone, Copy)]
pub struct OffsetCheckpoint { pub(super) raw: SvOffsetCheckpoint }
#[derive(Debug, Clone, Copy)]
pub struct RebuildCut { pub(super) raw: SvRebuildCut }
impl CheckpointCut {
    pub fn offset(&self) -> ConsumeOffset {
        let mut cluster_id = String::with_capacity(64);
        use std::fmt::Write;
        for byte in self.position.cluster { write!(&mut cluster_id, "{byte:02x}").expect("writing a String"); }
        ConsumeOffset { cluster_id, projector_id: self.projector_id.as_str().to_owned(), raft_term: self.position.raft_term, raft_index: self.position.raft_index }
    }
    /// Assertion after the application durably saved effects and this offset.
    pub fn checkpointed(self) -> OffsetCheckpoint { OffsetCheckpoint { raw: SvOffsetCheckpoint { cut: self.raw } } }
}

#[must_use]
pub enum Offer<'io, T: ReplyValue> { Queued(ReplyHandle<'io, T>), OwnerQueueFull, Closed, ReplyCapacityExhausted }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplyPoll<T> { Pending, Ready(T), Disconnected }
mod sealed {
    pub trait Sealed {}
    impl Sealed for Result<(), crate::ProjectorCheckpointRejection> {}
    impl Sealed for crate::ProjectorCheckpointStatus {}
}
#[doc(hidden)]
pub trait ReplyValue: sealed::Sealed + Copy {
    fn decode(reply: ControlReply) -> Result<ReplyPoll<Self>, A>;
}
impl ReplyValue for Result<(), Rejection> {
    fn decode(reply: ControlReply) -> Result<ReplyPoll<Self>, A> {
        match reply {
            ControlReply::Pending => Ok(ReplyPoll::Pending), ControlReply::Accepted => Ok(ReplyPoll::Ready(Ok(()))),
            ControlReply::Rejected(cause) => Ok(ReplyPoll::Ready(Err(cause))), ControlReply::Disconnected => Ok(ReplyPoll::Disconnected),
            ControlReply::Status(_) => Err(A::InvalidHandle),
        }
    }
}
impl ReplyValue for Status {
    fn decode(reply: ControlReply) -> Result<ReplyPoll<Self>, A> {
        match reply {
            ControlReply::Pending => Ok(ReplyPoll::Pending), ControlReply::Status(value) => Ok(ReplyPoll::Ready(value)),
            ControlReply::Disconnected => Ok(ReplyPoll::Disconnected), _ => Err(A::InvalidHandle),
        }
    }
}

/// Move-only, invariant scoped borrow; Send but not Sync. A reply cannot be
/// moved into an unscoped thread or survive the enclosing with_host invocation.
///
/// ```compile_fail,E0521
/// use statevec_projector_api::plugin_v1::{ProjectorIo, Offer};
/// fn escape(io: ProjectorIo<'_>) {
///     let control = io.split().control;
///     if let Offer::Queued(mut reply) = control.offer_status().unwrap() {
///         std::thread::spawn(move || { let _ = reply.poll(); });
///     }
/// }
/// ```
#[must_use]
pub struct ReplyHandle<'io, T: ReplyValue> {
    host: &'io ScopeData, cookie: u64, terminal: Option<ReplyPoll<T>>,
    _borrow: PhantomData<&'io mut &'io ()>, _single: PhantomData<Cell<()>>,
}
impl<T: ReplyValue> ReplyHandle<'_, T> {
    pub fn poll(&mut self) -> Result<ReplyPoll<T>, F> {
        let host = self.host;
        host.checked(self.read_reply())
    }
    fn read_reply(&mut self) -> Result<ReplyPoll<T>, F> {
        if let Some(value) = self.terminal { return Ok(value); }
        let mut raw = SvControlReply::default();
        // SAFETY: this handle retains the sole poll/release ownership and its
        // context lifetime; it may move only among services joined by the scope.
        call(unsafe { self.host.table.poll_reply.unwrap()(self.host.table.context, self.cookie, &mut raw) }, Op::PollReply)?;
        let value = T::decode(raw.decode()?)?;
        if !matches!(value, ReplyPoll::Pending) { self.terminal = Some(value); }
        Ok(value)
    }
}
impl<T: ReplyValue> Drop for ReplyHandle<'_, T> { fn drop(&mut self) { self.host.release_reply(self.cookie); } }

impl<'io> super::ProjectorControlPort<'io> {
    fn offer<T: ReplyValue>(&self, op: Op, invoke: impl FnOnce(*mut u32, *mut u64) -> SvStatus) -> Result<Offer<'io, T>, F> {
        let mut slots = self.host.replies.lock().unwrap_or_else(|e| e.into_inner());
        let Some(index) = slots.iter().position(|slot| slot.cookie == 0) else { return Ok(Offer::ReplyCapacityExhausted); };
        let (mut tag, mut cookie) = (0, 0);
        let result = call(invoke(&mut tag, &mut cookie), op);
        if let Err(cause) = result {
            if cookie != 0 {
                // Invalid transfer: retain unknown custody, never guess that a
                // returned cookie is a live resource safe to destroy.
                slots[index] = Held { cookie, attempted: true };
                drop(slots);
                self.host.record(4, A::InvalidHandle.into());
            }
            return Err(cause);
        }
        match tag {
            0 if cookie != 0 && slots.iter().all(|slot| slot.cookie != cookie) => {
                slots[index] = Held { cookie, attempted: false };
                Ok(Offer::Queued(ReplyHandle { host: self.host, cookie, terminal: None, _borrow: PhantomData, _single: PhantomData }))
            }
            1 if cookie == 0 => Ok(Offer::OwnerQueueFull), 2 if cookie == 0 => Ok(Offer::Closed),
            3 if cookie == 0 => Ok(Offer::ReplyCapacityExhausted),
            _ => {
                if cookie != 0 { slots[index] = Held { cookie, attempted: true }; }
                drop(slots);
                self.host.record(4, A::InvalidHandle.into());
                Err(A::InvalidHandle.into())
            }
        }
    }
    pub fn offer_checkpoint(&self, value: &OffsetCheckpoint) -> Result<Offer<'io, Result<(), Rejection>>, F> {
        self.host.acquisition_allowed()?;
        self.host.checked(self.offer(Op::OfferCheckpoint, |tag, handle| unsafe { self.host.table.offer_checkpoint.unwrap()(self.host.table.context, &value.raw, tag, handle) }))
    }
    pub fn offer_status(&self) -> Result<Offer<'io, Status>, F> {
        self.host.acquisition_allowed()?;
        self.host.checked(self.offer(Op::OfferStatus, |tag, handle| unsafe { self.host.table.offer_status.unwrap()(self.host.table.context, tag, handle) }))
    }
    pub fn offer_rebuild(&self, value: &RebuildCut) -> Result<Offer<'io, Result<(), Rejection>>, F> {
        self.host.acquisition_allowed()?;
        self.host.checked(self.offer(Op::OfferRebuild, |tag, handle| unsafe { self.host.table.offer_rebuild.unwrap()(self.host.table.context, &value.raw, tag, handle) }))
    }
}
