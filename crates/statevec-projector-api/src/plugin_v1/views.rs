use super::{borrow::*, ffi::*, io::ScopeData, ProjectorAbiFailure as A, ProjectorFailure as F, ProjectorOperation as Op};
use crate::{ProjectionPosition, ProjectionContext, TransactionResult, Digest32};
use statevec_model::{RecordKey, SchemaIdentity, Version};

pub(super) fn validate_frame(info: &SvFrameInfo) -> Result<(), A> {
    position(info.position)?;
    if info.reserved != 0 { return Err(A::Reserved { actual: info.reserved.into() }); }
    if info.context > 1 { return Err(A::Flag { actual: info.context }); }
    if info.transaction_count > usize::MAX as u64 { return Err(A::Length { actual: info.transaction_count, maximum: usize::MAX as u64 }); }
    let b = &info.cut.bytes;
    let p = info.position;
    if &b[..8] != b"SVCUT002" || b[40..72] != p.cluster || b[72..104] != p.genesis || b[104..136] != p.profile
        || b[136..140] != p.manifest.to_le_bytes() || b[140..144] != [0; 4]
        || b[144..152] != p.raft_index.to_le_bytes() || b[152..160] != p.raft_term.to_le_bytes()
        || b[160..192] != p.applied_batch_identity || b[192..200] != p.applied_tx_seq.to_le_bytes() || b[200..232] != p.tx_chain_hash {
        return Err(A::InvalidHandle);
    }
    Ok(())
}

pub struct FrameView<'a> { host: &'a ScopeData, cookie: u64, info: SvFrameInfo, _loan: std::marker::PhantomData<&'a super::FrameLoan<'a>> }
impl super::FrameLoan<'_> {
    pub fn view(&self) -> FrameView<'_> { FrameView { host: self.host, cookie: self.cookie, info: self.info, _loan: std::marker::PhantomData } }
    pub fn checkpoint_cut(&self) -> super::CheckpointCut {
        super::CheckpointCut { raw: self.info.cut, position: self.info.position, projector_id: self.host.projector_id }
    }
}
impl FrameView<'_> {
    pub fn position(&self) -> ProjectionPosition { position(self.info.position).expect("validated loan") }
    pub fn peer_id(&self) -> u64 { self.info.peer_id }
    pub fn context(&self) -> ProjectionContext { if self.info.context == 0 { ProjectionContext::Live } else { ProjectionContext::Replay } }
    pub fn transaction_count(&self) -> usize { self.info.transaction_count as usize }
    pub fn transaction(&self, index: usize) -> Result<TransactionView<'_>, F> {
        self.host.checked(self.read_transaction(index))
    }
    fn read_transaction(&self, index: usize) -> Result<TransactionView<'_>, F> {
        if index >= self.transaction_count() { return Err(A::Length { actual: index as u64, maximum: self.info.transaction_count }.into()); }
        let mut raw = std::mem::MaybeUninit::<SvTransactionInfo>::zeroed();
        // SAFETY: host fills the complete DTO on Success, payloads remain live
        // under this FrameView borrow. No payload allocation/copy is performed.
        call(unsafe { self.host.table.transaction.unwrap()(self.host.table.context, self.cookie, index as u64, raw.as_mut_ptr()) }, Op::Transaction)?;
        let raw = unsafe { raw.assume_init() };
        if raw.reserved != 0 || raw.reserved_profile != [0; 3] || raw.reserved_schema != [0; 6] { return Err(A::Reserved { actual: 1 }.into()); }
        if raw.ordinal as usize != index { return Err(A::CallbackCount { expected: index as u64, actual: raw.ordinal.into() }.into()); }
        let expected = match raw.result_digest_profile { 0x01 | 0x03 | 0x05 => 16, 0x04 | 0x06 => 32, actual => return Err(A::Flag { actual: actual.into() }.into()) };
        if raw.result_digest.len != expected { return Err(A::Length { actual: raw.result_digest.len, maximum: expected }.into()); }
        if raw.event_count > usize::MAX as u64 || raw.change_count > usize::MAX as u64 { return Err(A::Length { actual: raw.event_count.max(raw.change_count), maximum: usize::MAX as u64 }.into()); }
        unsafe { bytes(raw.result_digest)?; }
        Ok(TransactionView { host: self.host, cookie: self.cookie, index: index as u64, raw, _loan: std::marker::PhantomData })
    }
}
pub struct TransactionView<'a> { host: &'a ScopeData, cookie: u64, index: u64, raw: SvTransactionInfo, _loan: std::marker::PhantomData<&'a super::FrameLoan<'a>> }
impl TransactionView<'_> {
    pub fn result(&self) -> TransactionResult<'_> {
        TransactionResult { tx_seq: self.raw.tx_seq, ordinal: self.raw.ordinal, tx_id: Digest32::new(self.raw.tx_id), tx_chain_hash: Digest32::new(self.raw.tx_chain_hash),
            result_digest_profile: self.raw.result_digest_profile, result_digest: unsafe { bytes(self.raw.result_digest).expect("validated loan span") }, sys_result_code: self.raw.sys_result_code, biz_code: self.raw.biz_code }
    }
    pub fn schema_identity(&self) -> SchemaIdentity {
        let s = self.raw.schema;
        SchemaIdentity { schema_version: Version::new(s.version_main, s.version_minor), types_schema_fingerprint: s.types, record_schema_fingerprint: s.records, command_schema_fingerprint: s.commands, event_schema_fingerprint: s.events }
    }
    pub fn ref_tx_time_ns(&self) -> u64 { self.raw.ref_tx_time_ns }
    pub fn event_count(&self) -> usize { self.raw.event_count as usize }
    pub fn change_count(&self) -> usize { self.raw.change_count as usize }
    pub fn event(&self, index: usize) -> Result<EventView<'_>, F> {
        self.host.checked(self.read_event(index))
    }
    fn read_event(&self, index: usize) -> Result<EventView<'_>, F> {
        if index >= self.event_count() { return Err(A::Length { actual: index as u64, maximum: self.raw.event_count }.into()); }
        let mut raw = std::mem::MaybeUninit::<SvEvent>::zeroed();
        call(unsafe { self.host.table.event.unwrap()(self.host.table.context, self.cookie, self.index, index as u64, raw.as_mut_ptr()) }, Op::Event)?;
        let raw = unsafe { raw.assume_init() };
        if raw.reserved != 0 { return Err(A::Reserved { actual: raw.reserved.into() }.into()); }
        Ok(EventView { event_seq: raw.event_seq, event_kind: raw.kind, payload: unsafe { bytes(raw.payload)? } })
    }
    pub fn change(&self, index: usize) -> Result<RecordChangeView<'_>, F> {
        self.host.checked(self.read_change(index))
    }
    fn read_change(&self, index: usize) -> Result<RecordChangeView<'_>, F> {
        if index >= self.change_count() { return Err(A::Length { actual: index as u64, maximum: self.raw.change_count }.into()); }
        let mut raw = std::mem::MaybeUninit::<SvRecordChange>::zeroed();
        call(unsafe { self.host.table.change.unwrap()(self.host.table.context, self.cookie, self.index, index as u64, raw.as_mut_ptr()) }, Op::Change)?;
        let raw = unsafe { raw.assume_init() };
        if raw.reserved != 0 || raw.key.reserved16 != 0 || raw.key.reserved32 != 0 { return Err(A::Reserved { actual: 1 }.into()); }
        let key = RecordKey { kind: raw.key.kind, sys_id: raw.key.sys_id };
        let before = unsafe { bytes(raw.before)? };
        let after = unsafe { bytes(raw.after)? };
        match raw.operation {
            0 if before.is_empty() && raw.before.ptr.is_null() => Ok(RecordChangeView::Insert { key, revision: raw.revision, after }),
            1 => Ok(RecordChangeView::Update { key, revision: raw.revision, before, after }),
            2 if after.is_empty() && raw.after.ptr.is_null() => Ok(RecordChangeView::Delete { key, revision: raw.revision, before }),
            0 | 2 => Err(A::Reserved { actual: 1 }.into()),
            actual => Err(A::Flag { actual }.into()),
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventView<'a> { pub event_seq: u32, pub event_kind: u16, pub payload: &'a [u8] }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordChangeView<'a> {
    Insert { key: RecordKey, revision: u64, after: &'a [u8] },
    Update { key: RecordKey, revision: u64, before: &'a [u8], after: &'a [u8] },
    Delete { key: RecordKey, revision: u64, before: &'a [u8] },
}
