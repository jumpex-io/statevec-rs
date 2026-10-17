use super::io::ScopeFailureSlot;
use super::{borrow::call, ffi::*, io::{ReleaseDisposition, Held, ReplySlot, ScopeData}, ProjectorAbiError as A, ProjectorError as F, ProjectorOperation as Op, ControlReply};
use crate::{ConsumeOffset, ProjectorControlRejection as Rejection, ProjectorStatus as Status};
use std::{cell::Cell, marker::PhantomData};

#[derive(Debug, Clone, Copy)]
pub struct RaftEntryToken { pub(super) raw: SvCut, pub(super) position: SvPosition, pub(super) projector_id: crate::ProjectorId, pub(super) mode: crate::HistoryMode }
#[derive(Debug, Clone, Copy)]
pub struct DurableOffsetReport { pub(super) raw: SvDurableOffsetReport }
#[derive(Debug, Clone, Copy)]
pub struct SnapshotToken { pub(super) raw: SvRebuildCut }
/// Local refusal to declare a durable history offset from a latest-state token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotContinuousHistory;
impl std::fmt::Display for NotContinuousHistory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("latest-state entry tokens do not define durable history offsets")
    }
}
impl std::error::Error for NotContinuousHistory {}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlDecision { Accepted, Rejected(Rejection) }
impl RaftEntryToken {
    /// Latest-state Raft entries never define a durable consume offset.
    pub fn offset(&self) -> Option<ConsumeOffset> {
        if self.mode != crate::HistoryMode::Continuous { return None; }
        let cluster_id = super::inputs::cluster_id(self.position.cluster);
        Some(ConsumeOffset { cluster_id, projector_id: self.projector_id.as_str().to_owned(), raft_term: self.position.raft_term, raft_index: self.position.raft_index })
    }
    /// Assertion after the application durably saved effects and this offset.
    pub fn declare_durable(self) -> Result<DurableOffsetReport, NotContinuousHistory> {
        if self.mode != crate::HistoryMode::Continuous { return Err(NotContinuousHistory); }
        Ok(DurableOffsetReport { raw: SvDurableOffsetReport { cut: self.raw } })
    }
    /// Compares the four offset fields only, not model/frontier equality.
    pub fn is_at(&self, offset: &ConsumeOffset) -> bool { self.offset().as_ref() == Some(offset) }
}

#[must_use]
pub enum Offer<'io, T: ReplyValue> { Queued(ReplyHandle<'io, T>), OwnerQueueFull, Closed, ReplyCapacityExhausted }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplyPoll<T> { Pending, Ready(T), Disconnected }
mod sealed {
    pub trait Sealed {}
    impl Sealed for super::ControlDecision {}
    impl Sealed for crate::ProjectorStatus {}
}
#[doc(hidden)]
pub trait ReplyValue: sealed::Sealed + Copy {
    fn decode(reply: ControlReply) -> Result<ReplyPoll<Self>, A>;
}
impl ReplyValue for ControlDecision {
    fn decode(reply: ControlReply) -> Result<ReplyPoll<Self>, A> {
        match reply {
            ControlReply::Pending => Ok(ReplyPoll::Pending), ControlReply::Accepted => Ok(ReplyPoll::Ready(Self::Accepted)),
            ControlReply::Rejected(cause) => Ok(ReplyPoll::Ready(Self::Rejected(cause))), ControlReply::Disconnected => Ok(ReplyPoll::Disconnected),
            ControlReply::Status(_) => Err(A::ReplyKindMismatch),
        }
    }
}
impl ReplyValue for Status {
    fn decode(reply: ControlReply) -> Result<ReplyPoll<Self>, A> {
        match reply {
            ControlReply::Pending => Ok(ReplyPoll::Pending), ControlReply::Status(value) => Ok(ReplyPoll::Ready(value)),
            ControlReply::Disconnected => Ok(ReplyPoll::Disconnected), _ => Err(A::ReplyKindMismatch),
        }
    }
}

/// Move-only, invariant scoped borrow; Send but not Sync. A reply cannot be
/// moved into an unscoped thread or survive the enclosing with_host invocation.
/// Ready caches its answer but retains a reply slot until Drop. Drop releases
/// ownership, not the queued request. Disconnected is an unknown outcome: retry
/// the identical durability declaration, or discard a status observation.
///
/// ```compile_fail,E0521
/// use statevec_projector_api::v1::{ProjectorIo, Offer};
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
        host.latch_contract_failure(self.read_reply())
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
        let index = {
            let mut slots = self.host.replies.lock().unwrap_or_else(|e| e.into_inner());
            let Some(index) = slots.iter().position(|slot| matches!(slot, ReplySlot::Free)) else { return Ok(Offer::ReplyCapacityExhausted); };
            // Count the exact in-progress physical call against the same four
            // slots; never hold this lock across foreign code.
            slots[index] = ReplySlot::Offering;
            index
        };
        let (mut tag, mut cookie) = (0, 0);
        let result = call(invoke(&mut tag, &mut cookie), op);
        let mut slots = self.host.replies.lock().unwrap_or_else(|e| e.into_inner());
        debug_assert!(matches!(slots[index], ReplySlot::Offering));
        slots[index] = ReplySlot::Free;
        if let Err(cause) = result {
            if cookie != 0 {
                // Invalid transfer: retain unknown ownership, never guess that a
                // returned cookie is a live resource safe to destroy.
                slots[index] = ReplySlot::Held(Held { cookie, release: ReleaseDisposition::UntrustedTransfer });
                drop(slots);
                self.host.record(ScopeFailureSlot::ReplyRelease, A::InvalidHandle.into());
            }
            return Err(cause);
        }
        match tag {
            offer_tag::QUEUED if cookie != 0 && slots.iter().all(|slot| !matches!(slot, ReplySlot::Held(held) if held.cookie == cookie)) => {
                slots[index] = ReplySlot::Held(Held { cookie, release: ReleaseDisposition::Available });
                Ok(Offer::Queued(ReplyHandle { host: self.host, cookie, terminal: None, _borrow: PhantomData, _single: PhantomData }))
            }
            offer_tag::OWNER_QUEUE_FULL if cookie == 0 => Ok(Offer::OwnerQueueFull), offer_tag::CLOSED if cookie == 0 => Ok(Offer::Closed),
            offer_tag::REPLY_CAPACITY_EXHAUSTED if cookie == 0 => Ok(Offer::ReplyCapacityExhausted),
            _ => {
                if cookie != 0 { slots[index] = ReplySlot::Held(Held { cookie, release: ReleaseDisposition::UntrustedTransfer }); }
                drop(slots);
                self.host.record(ScopeFailureSlot::ReplyRelease, A::InvalidHandle.into());
                Err(A::InvalidHandle.into())
            }
        }
    }
    /// Report a prefix whose business effects and offset are already durable.
    /// This enqueues a host request; inspect its reply to confirm acceptance.
    pub fn report_durable_offset(&self, value: &DurableOffsetReport) -> Result<Offer<'io, ControlDecision>, F> {
        self.host.acquisition_allowed()?;
        self.host.latch_contract_failure(self.offer(Op::ReportDurableOffset, |tag, handle| unsafe { self.host.table.report_durable_offset.unwrap()(self.host.table.context, &value.raw, tag, handle) }))
    }
    pub fn offer_status(&self) -> Result<Offer<'io, Status>, F> {
        self.host.acquisition_allowed()?;
        self.host.latch_contract_failure(self.offer(Op::OfferStatus, |tag, handle| unsafe { self.host.table.offer_status.unwrap()(self.host.table.context, tag, handle) }))
    }
    pub fn offer_snapshot_confirmation(&self, value: &SnapshotToken) -> Result<Offer<'io, ControlDecision>, F> {
        self.host.acquisition_allowed()?;
        self.host.latch_contract_failure(self.offer(Op::OfferSnapshotConfirmation, |tag, handle| unsafe { self.host.table.offer_rebuild.unwrap()(self.host.table.context, &value.raw, tag, handle) }))
    }
}
