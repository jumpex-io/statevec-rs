//! Lending state views and exact handle disposition. The host owns traversal
//! integrity; this layer checks its wire evidence and never manufactures a cut.
use super::{borrow::*, ffi::*, io::{Held, ScopeData}, ProjectorAbiFailure as A,
    ProjectorFailure as F, ProjectorOperation as Op, RebuildSourceFailure as R};
use std::{marker::PhantomData, rc::Rc, sync::atomic::Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProjectionStateInfo {
    pub position: crate::ProjectionPosition,
    pub record_count: u64,
}
pub struct ProjectionStateRecord<'a> {
    pub key: statevec_model::RecordKey,
    pub revision: u64,
    pub bytes: &'a [u8],
}
impl std::fmt::Debug for ProjectionStateRecord<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProjectionStateRecord").field("key", &self.key)
            .field("revision", &self.revision).field("length", &self.bytes.len()).finish()
    }
}
#[derive(Debug)]
pub enum StateRead<'a> { Record(ProjectionStateRecord<'a>), End, Stop, Progress }
enum ReadState { Reading(u64), End, Failed }

/// One source-worker loan. Borrowed bytes cannot survive another read, finish
/// or drop; this loan cannot move to another thread.
///
/// ```compile_fail,E0499
/// use statevec_projector_api::plugin_v1::{StateLoan, StateRead};
/// fn overlap(loan: &mut StateLoan<'_>) {
///     let StateRead::Record(record) = loan.next_record().unwrap() else { return };
///     let _ = loan.next_record();
///     let _ = record.bytes.len();
/// }
/// ```
/// ```compile_fail,E0277
/// use statevec_projector_api::plugin_v1::StateLoan;
/// fn escape(loan: StateLoan<'_>) {
///     std::thread::scope(|scope| { scope.spawn(move || drop(loan)); });
/// }
/// ```
#[must_use]
pub struct StateLoan<'a> {
    host: &'a ScopeData,
    cookie: u64,
    info: SvStateInfo,
    read: ReadState,
    _local: PhantomData<Rc<()>>,
}
impl<'a> StateLoan<'a> {
    pub(super) fn acquire(host: &'a ScopeData, cookie: u64) -> Result<Self, F> {
        let crate::ProjectorInitialization::Rebuild { target } = host.initialization else {
            return Err(A::InvalidHandle.into());
        };
        let mut info = SvStateInfo::default();
        // SAFETY: sole worker retains the nonzero cookie and exact output.
        call(unsafe { host.table.state_info.unwrap()(host.table.context, cookie, &mut info) }, Op::StateInfo)?;
        let p = state_position(info.position)?;
        let base = host.startup;
        if p.raft_index != target || p.cluster_identity != base.identity.cluster_identity
            || p.genesis_identity != base.identity.genesis_identity
            || p.execution_profile_identity != base.identity.execution_profile_identity
            || p.manifest != base.identity.manifest
            || p.raft_index == base.raft_index && (p.raft_term != base.raft_term || p.frontier != base.frontier) {
            return Err(A::InvalidHandle.into());
        }
        Ok(Self { host, cookie, info, read: ReadState::Reading(0), _local: PhantomData })
    }
    pub fn info(&self) -> ProjectionStateInfo {
        ProjectionStateInfo { position: state_position(self.info.position).expect("validated state info"), record_count: self.info.record_count }
    }
    fn live(&self) -> Result<(), F> {
        let held = self.host.state.lock().unwrap_or_else(|p| p.into_inner());
        if self.cookie == 0 || held.cookie != self.cookie || held.attempted { return Err(A::InvalidHandle.into()); }
        Ok(())
    }
    pub fn next_record(&mut self) -> Result<StateRead<'_>, F> {
        let host = self.host;
        self.live()?;
        let count = match self.read {
            ReadState::Reading(count) => count,
            ReadState::End => return Ok(StateRead::End),
            ReadState::Failed => return Err(F::RebuildSource(R::Unavailable)),
        };
        self.read = ReadState::Failed;
        let mut tag = 0;
        let mut raw: SvStateRecord = unsafe { std::mem::zeroed() };
        // SAFETY: &mut loan prevents another read/release while returned bytes
        // are borrowed. Output storage is complete and disjoint from the host.
        let result = call(unsafe { self.host.table.state_next.unwrap()(self.host.table.context, self.cookie, &mut tag, &mut raw) }, Op::StateNext)
            .and_then(|()| self.decode_record(tag, raw, count));
        if let Err(cause) = &result { host.record(1, *cause); }
        result
    }
    fn decode_record(&mut self, tag: u32, raw: SvStateRecord, count: u64) -> Result<StateRead<'_>, F> {
        if raw.key.reserved16 != 0 || raw.key.reserved32 != 0 { return Err(A::Reserved { actual: 1 }.into()); }
        if tag != 0 && (raw.key.kind != 0 || raw.key.sys_id != 0 || raw.revision != 0 || !raw.record.ptr.is_null() || raw.record.len != 0) {
            return Err(A::Reserved { actual: 1 }.into());
        }
        match tag {
            0 => {
                if raw.key.sys_id == 0 { return Err(A::InvalidHandle.into()); }
                let next = count.checked_add(1).filter(|next| *next <= self.info.record_count)
                    .ok_or(A::CallbackCount { expected: self.info.record_count, actual: count.saturating_add(1) })?;
                let bytes = unsafe { bytes(raw.record)? };
                self.read = ReadState::Reading(next);
                Ok(StateRead::Record(ProjectionStateRecord { key: statevec_model::RecordKey { kind: raw.key.kind, sys_id: raw.key.sys_id }, revision: raw.revision, bytes }))
            }
            1 if count == self.info.record_count => { self.read = ReadState::End; Ok(StateRead::End) }
            1 => Err(A::CallbackCount { expected: self.info.record_count, actual: count }.into()),
            2 => Ok(StateRead::Stop), // Stop is not EOF; finish remains forbidden.
            3 => { self.read = ReadState::Reading(count); Ok(StateRead::Progress) }
            actual => Err(A::Flag { actual }.into()),
        }
    }
    pub fn finish(&mut self) -> Result<super::RebuildCut, F> {
        self.live()?;
        if !matches!(self.read, ReadState::End) { return Err(F::RebuildSource(R::Incomplete)); }
        let mut returned = self.cookie;
        let mut raw = SvRebuildCut::default();
        let status = unsafe { self.host.table.finish_state.unwrap()(self.host.table.context, &mut returned, &mut raw) };
        let canonical = super::decode_call(status, Op::FinishState).is_ok();
        let result = call(status, Op::FinishState);
        let valid = canonical && match &result {
            Ok(()) => returned == 0 && valid_cut(&raw, self.info.position),
            Err(F::RebuildSource(R::Incomplete)) => returned == self.cookie && raw == SvRebuildCut::default(),
            Err(_) => (returned == 0 || returned == self.cookie) && raw == SvRebuildCut::default(),
        };
        if !valid {
            // Unknown transfer: never retry a possibly consumed handle or claim
            // retirement solely because a malformed callback wrote zero.
            self.host.state.lock().unwrap_or_else(|p| p.into_inner()).attempted = true;
            self.host.record(3, A::InvalidHandle.into());
            return Err(result.err().unwrap_or_else(|| A::InvalidHandle.into()));
        }
        if returned == 0 {
            *self.host.state.lock().unwrap_or_else(|p| p.into_inner()) = Held::default();
            self.cookie = 0;
        }
        match result {
            Ok(()) => Ok(super::RebuildCut { raw }),
            Err(cause) => {
                if cause != F::RebuildSource(R::Incomplete) { self.host.record(1, cause); }
                Err(cause)
            }
        }
    }
}
impl Drop for StateLoan<'_> { fn drop(&mut self) { self.host.release_state(); } }

fn valid_cut(cut: &SvRebuildCut, p: SvPosition) -> bool {
    let b = &cut.bytes;
    &b[..8] == b"SVRBLD02" && b[40..72] == p.cluster && b[72..104] == p.genesis && b[104..136] == p.profile
        && b[136..140] == p.manifest.to_le_bytes() && b[140..144] == [0; 4]
        && b[144..152] == p.raft_index.to_le_bytes() && b[152..160] == p.raft_term.to_le_bytes()
        && b[160..192] == p.applied_batch_identity && b[192..200] == p.applied_tx_seq.to_le_bytes()
        && b[200..232] == p.tx_chain_hash
}

impl ScopeData {
    pub(super) fn release_state(&self) {
        let cookie = {
            let mut held = self.state.lock().unwrap_or_else(|p| p.into_inner());
            if held.cookie == 0 || held.attempted { return; }
            held.attempted = true;
            held.cookie
        };
        let mut returned = cookie;
        let status = unsafe { self.table.release_state.unwrap()(self.table.context, &mut returned) };
        let canonical = super::decode_call(status, Op::ReleaseState).is_ok();
        let result = call(status, Op::ReleaseState);
        if canonical && returned == 0 { *self.state.lock().unwrap_or_else(|p| p.into_inner()) = Held::default(); }
        match result {
            Err(F::RebuildSource(R::RebuildAbandoned)) if returned == 0 => {
                self.abandoned.store(true, Ordering::Release);
                if let Err(cause) = self.stop() { self.record(3, cause); }
            }
            Err(cause) => self.record(3, cause),
            Ok(()) if returned != 0 => self.record(3, A::InvalidHandle.into()),
            Ok(()) => (),
        }
    }
}
