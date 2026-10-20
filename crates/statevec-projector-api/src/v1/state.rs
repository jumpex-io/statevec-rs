//! Lending state views and exact handle disposition. The host owns traversal
//! integrity; this layer checks its wire metadata and never manufactures a token.
use super::io::ScopeFailureSlot;
use super::{borrow::*, ffi::*, io::{ReleaseDisposition, Held, ScopeData}, ProjectorAbiError as A,
    ProjectorError as F, ProjectorOperation as Op, ProjectorSnapshotReadError as R};
use std::{marker::PhantomData, rc::Rc, sync::atomic::Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapshotInfo {
    pub position: crate::SourcePosition,
    pub record_count: u64,
}
pub struct SnapshotRecordView<'a> {
    pub key: statevec_model::RecordKey,
    pub revision: u64,
    pub bytes: &'a [u8],
}
impl std::fmt::Debug for SnapshotRecordView<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SnapshotRecordView").field("key", &self.key)
            .field("revision", &self.revision).field("length", &self.bytes.len()).finish()
    }
}
#[derive(Debug)]
pub enum SnapshotRead<'a> {
    Record(SnapshotRecordView<'a>), End, Stop,
    /// One bounded scan quantum with no record yet; not EOF or Stop.
    Progress,
}
enum ReadState { Reading(u64), End, Failed }

/// One source-worker borrow. Borrowed bytes cannot survive another read, finish
/// or drop; this borrow cannot move to another thread.
///
/// ```compile_fail,E0499
/// use statevec_projector_api::v1::{SnapshotReader, SnapshotRead};
/// fn overlap(borrow: &mut SnapshotReader<'_>) {
///     let SnapshotRead::Record(record) = borrow.next_record().unwrap() else { return };
///     let _ = borrow.next_record();
///     let _ = record.bytes.len();
/// }
/// ```
/// ```compile_fail,E0277
/// use statevec_projector_api::v1::SnapshotReader;
/// fn escape(borrow: SnapshotReader<'_>) {
///     std::thread::scope(|scope| { scope.spawn(move || drop(borrow)); });
/// }
/// ```
#[must_use]
pub struct SnapshotReader<'a> {
    host: &'a ScopeData,
    cookie: u64,
    info: SvStateInfo,
    read: ReadState,
    _local: PhantomData<Rc<()>>,
}
impl<'a> SnapshotReader<'a> {
    pub(super) fn acquire(host: &'a ScopeData, cookie: u64) -> Result<Self, F> {
        let crate::StartInstruction::Rebuild { target } = host.initialization else {
            return Err(A::BindingMismatch.into());
        };
        let mut info = SvStateInfo::default();
        // SAFETY: sole worker retains the nonzero cookie and exact output.
        call(unsafe { host.table.state_info.unwrap()(host.table.context, cookie, &mut info) }, Op::SnapshotInfo)?;
        let p = position_allowing_genesis(info.position)?;
        let base = host.startup;
        if p.raft_index != target || p.cluster_identity != base.identity.cluster_identity
            || p.genesis_identity != base.identity.genesis_identity
            || p.execution_lineage_identity != base.identity.execution_lineage_identity
            || p.manifest != base.identity.manifest
            || p.raft_index == base.raft_index && (p.raft_term != base.raft_term || p.frontier != base.frontier) {
            return Err(A::BindingMismatch.into());
        }
        Ok(Self { host, cookie, info, read: ReadState::Reading(0), _local: PhantomData })
    }
    pub fn info(&self) -> SnapshotInfo {
        SnapshotInfo { position: position_allowing_genesis(self.info.position).expect("validated state info"), record_count: self.info.record_count }
    }
    fn live(&self) -> Result<(), F> {
        let held = self.host.state.lock().unwrap_or_else(|p| p.into_inner());
        if self.cookie == 0 || held.cookie != self.cookie || held.release != ReleaseDisposition::Available { return Err(A::InvalidHandle.into()); }
        Ok(())
    }
    pub fn next_record(&mut self) -> Result<SnapshotRead<'_>, F> {
        let host = self.host;
        self.live()?;
        let count = match self.read {
            ReadState::Reading(count) => count,
            ReadState::End => return Ok(SnapshotRead::End),
            ReadState::Failed => return Err(F::SnapshotRead(R::Unavailable)),
        };
        self.read = ReadState::Failed;
        let mut tag = 0;
        let mut raw: SvStateRecord = unsafe { std::mem::zeroed() };
        // SAFETY: &mut borrow prevents another read/release while returned bytes
        // are borrowed. Output storage is complete and disjoint from the host.
        let result = call(unsafe { self.host.table.state_next.unwrap()(self.host.table.context, self.cookie, &mut tag, &mut raw) }, Op::NextSnapshotRecord)
            .and_then(|()| self.decode_record(tag, raw, count));
        if let Err(cause) = &result { host.record(ScopeFailureSlot::Run, *cause); }
        result
    }
    fn decode_record(&mut self, tag: u32, raw: SvStateRecord, count: u64) -> Result<SnapshotRead<'_>, F> {
        if raw.key.reserved16 != 0 || raw.key.reserved32 != 0 { return Err(A::Reserved { actual: 1 }.into()); }
        if tag != state_tag::RECORD && (raw.key.kind != 0 || raw.key.sys_id != 0 || raw.revision != 0 || !raw.record.ptr.is_null() || raw.record.len != 0) {
            return Err(A::Reserved { actual: 1 }.into());
        }
        match tag {
            state_tag::RECORD => {
                if raw.key.sys_id == 0 { return Err(A::InvalidHandle.into()); }
                let next = count.checked_add(1).filter(|next| *next <= self.info.record_count)
                    .ok_or(A::CallbackCount { expected: self.info.record_count, actual: count.saturating_add(1) })?;
                let bytes = unsafe { bytes(raw.record)? };
                self.read = ReadState::Reading(next);
                Ok(SnapshotRead::Record(SnapshotRecordView { key: statevec_model::RecordKey { kind: raw.key.kind, sys_id: raw.key.sys_id }, revision: raw.revision, bytes }))
            }
            state_tag::END if count == self.info.record_count => { self.read = ReadState::End; Ok(SnapshotRead::End) }
            state_tag::END => Err(A::CallbackCount { expected: self.info.record_count, actual: count }.into()),
            state_tag::STOP => Ok(SnapshotRead::Stop), // Stop is not EOF; finish remains forbidden.
            state_tag::PROGRESS => { self.read = ReadState::Reading(count); Ok(SnapshotRead::Progress) }
            actual => Err(A::Flag { actual }.into()),
        }
    }
    pub fn finish(&mut self) -> Result<super::SnapshotToken, F> {
        self.live()?;
        if !matches!(self.read, ReadState::End) { return Err(F::SnapshotRead(R::Incomplete)); }
        let mut returned = self.cookie;
        let mut raw = SvRebuildCut::default();
        let status = unsafe { self.host.table.finish_state.unwrap()(self.host.table.context, &mut returned, &mut raw) };
        let canonical = super::decode_call(status, Op::FinishSnapshot).is_ok();
        let result = call(status, Op::FinishSnapshot);
        let valid = canonical && match &result {
            Ok(()) => returned == 0 && cut_binds(&raw.bytes, b"SVRBLD02", self.info.position),
            Err(F::SnapshotRead(R::Incomplete)) => returned == self.cookie && raw == SvRebuildCut::default(),
            Err(_) => (returned == 0 || returned == self.cookie) && raw == SvRebuildCut::default(),
        };
        if !valid {
            // Unknown transfer: never retry a possibly consumed handle or claim
            // retirement solely because a malformed callback wrote zero.
            self.host.state.lock().unwrap_or_else(|p| p.into_inner()).release = ReleaseDisposition::UntrustedTransfer;
            self.host.record(ScopeFailureSlot::StateRelease, A::InvalidHandle.into());
            return Err(result.err().unwrap_or_else(|| A::InvalidHandle.into()));
        }
        if returned == 0 {
            *self.host.state.lock().unwrap_or_else(|p| p.into_inner()) = Held::default();
            self.cookie = 0;
        }
        match result {
            Ok(()) => Ok(super::SnapshotToken { raw }),
            Err(cause) => {
                if cause != F::SnapshotRead(R::Incomplete) { self.host.record(ScopeFailureSlot::Run, cause); }
                Err(cause)
            }
        }
    }
}
impl Drop for SnapshotReader<'_> { fn drop(&mut self) { self.host.release_state(); } }

impl ScopeData {
    pub(super) fn release_state(&self) {
        let cookie = {
            let mut held = self.state.lock().unwrap_or_else(|p| p.into_inner());
            if held.cookie == 0 || held.release != ReleaseDisposition::Available { return; }
            held.release = ReleaseDisposition::Attempted;
            held.cookie
        };
        let mut returned = cookie;
        let status = unsafe { self.table.release_state.unwrap()(self.table.context, &mut returned) };
        let canonical = super::decode_call(status, Op::ReleaseSnapshot).is_ok();
        let result = call(status, Op::ReleaseSnapshot);
        if canonical && returned == 0 { *self.state.lock().unwrap_or_else(|p| p.into_inner()) = Held::default(); }
        match result {
            Err(F::SnapshotRead(R::RebuildAbandoned)) if returned == 0 => {
                self.abandoned.store(true, Ordering::Release);
                if let Err(cause) = self.stop() { self.record(ScopeFailureSlot::StateRelease, cause); }
            }
            Err(cause) => self.record(ScopeFailureSlot::StateRelease, cause),
            Ok(()) if returned != 0 => self.record(ScopeFailureSlot::StateRelease, A::InvalidHandle.into()),
            Ok(()) => (),
        }
    }
}
