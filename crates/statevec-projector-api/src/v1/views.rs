use super::{borrow::*, ffi::*, io::ScopeData, ProjectorAbiError as A, ProjectorError as F, ProjectorOperation as Op};
use crate::{SourcePosition, ProjectionContext, TxResult, Digest32};
use statevec_model::{RecordKey, SchemaIdentity, Version};

pub(super) fn validate_frame(info: &SvFrameInfo) -> Result<(), A> {
    frame_position(info.position)?;
    if info.reserved != 0 { return Err(A::Reserved { actual: info.reserved.into() }); }
    if info.context > 1 { return Err(A::Flag { actual: info.context }); }
    if info.transaction_count > usize::MAX as u64 { return Err(A::Length { actual: info.transaction_count, maximum: usize::MAX as u64 }); }
    if !cut_binds(&info.cut.bytes, b"SVCUT002", info.position) {
        return Err(A::BindingMismatch);
    }
    Ok(())
}

pub struct RaftEntryView<'a> { host: &'a ScopeData, cookie: u64, info: SvFrameInfo, _loan: std::marker::PhantomData<&'a super::RaftEntryDelivery<'a>> }
impl super::RaftEntryDelivery<'_> {
    pub fn view(&self) -> RaftEntryView<'_> { RaftEntryView { host: self.host, cookie: self.cookie, info: self.info, _loan: std::marker::PhantomData } }
    pub fn token(&self) -> super::RaftEntryToken {
        let mode = match self.host.initialization {
            crate::StartInstruction::DeliverEntries => crate::HistoryMode::Continuous,
            crate::StartInstruction::Rebuild { .. } => crate::HistoryMode::LatestState,
        };
        super::RaftEntryToken { raw: self.info.cut, position: self.info.position, projector_id: self.host.projector_id, mode }
    }
}
impl RaftEntryView<'_> {
    pub fn position(&self) -> SourcePosition { frame_position(self.info.position).expect("validated loan") }
    pub fn peer_id(&self) -> u64 { self.info.peer_id }
    pub fn context(&self) -> ProjectionContext { if self.info.context == 0 { ProjectionContext::Live } else { ProjectionContext::Replay } }
    pub fn tx_count(&self) -> usize { self.info.transaction_count as usize }
    pub fn tx(&self, index: usize) -> Result<TxView<'_>, F> {
        self.host.latch_contract_failure(self.read_transaction(index))
    }
    fn read_transaction(&self, index: usize) -> Result<TxView<'_>, F> {
        if index >= self.tx_count() { return Err(A::Length { actual: index as u64, maximum: self.info.transaction_count }.into()); }
        let mut raw = std::mem::MaybeUninit::<SvTransactionInfo>::zeroed();
        // SAFETY: host fills the complete DTO on Success, payloads remain live
        // under this RaftEntryView borrow. No payload allocation/copy is performed.
        call(unsafe { self.host.table.transaction.unwrap()(self.host.table.context, self.cookie, index as u64, raw.as_mut_ptr()) }, Op::Tx)?;
        let raw = unsafe { raw.assume_init() };
        if raw.reserved != 0 || raw.reserved_profile != [0; 3] || raw.reserved_schema != [0; 6] { return Err(A::Reserved { actual: 1 }.into()); }
        if raw.ordinal as usize != index { return Err(A::CallbackCount { expected: index as u64, actual: raw.ordinal.into() }.into()); }
        let expected = match raw.result_digest_profile { 0x01 | 0x03 | 0x05 => 16, 0x04 | 0x06 => 32, actual => return Err(A::Flag { actual: actual.into() }.into()) };
        if raw.result_digest.len != expected { return Err(A::Length { actual: raw.result_digest.len, maximum: expected }.into()); }
        if raw.event_count > usize::MAX as u64 || raw.change_count > usize::MAX as u64 { return Err(A::Length { actual: raw.event_count.max(raw.change_count), maximum: usize::MAX as u64 }.into()); }
        unsafe { bytes(raw.result_digest)?; }
        Ok(TxView { host: self.host, cookie: self.cookie, index: index as u64, raw, _loan: std::marker::PhantomData })
    }
}
pub struct TxView<'a> { host: &'a ScopeData, cookie: u64, index: u64, raw: SvTransactionInfo, _loan: std::marker::PhantomData<&'a super::RaftEntryDelivery<'a>> }
impl TxView<'_> {
    pub fn result(&self) -> TxResult<'_> {
        TxResult { tx_seq: self.raw.tx_seq, ordinal: self.raw.ordinal, tx_id: Digest32::new(self.raw.tx_id), tx_chain_hash: Digest32::new(self.raw.tx_chain_hash),
            result_digest_profile: self.raw.result_digest_profile, result_digest: unsafe { bytes(self.raw.result_digest).expect("validated loan span") }, system_code: self.raw.sys_result_code, business_code: self.raw.biz_code }
    }
    pub fn schema_identity(&self) -> SchemaIdentity {
        let s = self.raw.schema;
        SchemaIdentity { schema_version: Version::new(s.version_main, s.version_minor), types_schema_fingerprint: s.types, record_schema_fingerprint: s.records, command_schema_fingerprint: s.commands, event_schema_fingerprint: s.events }
    }
    pub fn ref_tx_time_ns(&self) -> u64 { self.raw.ref_tx_time_ns }
    pub fn event_count(&self) -> usize { self.raw.event_count as usize }
    pub fn record_delta_count(&self) -> usize { self.raw.change_count as usize }
    pub fn event(&self, index: usize) -> Result<EventView<'_>, F> {
        self.host.latch_contract_failure(self.read_event(index))
    }
    fn read_event(&self, index: usize) -> Result<EventView<'_>, F> {
        if index >= self.event_count() { return Err(A::Length { actual: index as u64, maximum: self.raw.event_count }.into()); }
        let mut raw = std::mem::MaybeUninit::<SvEvent>::zeroed();
        call(unsafe { self.host.table.event.unwrap()(self.host.table.context, self.cookie, self.index, index as u64, raw.as_mut_ptr()) }, Op::Event)?;
        let raw = unsafe { raw.assume_init() };
        if raw.reserved != 0 { return Err(A::Reserved { actual: raw.reserved.into() }.into()); }
        Ok(EventView { event_seq: raw.event_seq, event_kind: raw.kind, payload: unsafe { bytes(raw.payload)? } })
    }
    pub fn record_delta(&self, index: usize) -> Result<RecordDeltaView<'_>, F> {
        self.host.latch_contract_failure(self.read_change(index))
    }
    fn read_change(&self, index: usize) -> Result<RecordDeltaView<'_>, F> {
        if index >= self.record_delta_count() { return Err(A::Length { actual: index as u64, maximum: self.raw.change_count }.into()); }
        let mut raw = std::mem::MaybeUninit::<SvRecordChange>::zeroed();
        call(unsafe { self.host.table.change.unwrap()(self.host.table.context, self.cookie, self.index, index as u64, raw.as_mut_ptr()) }, Op::RecordDelta)?;
        let raw = unsafe { raw.assume_init() };
        if raw.reserved != 0 || raw.key.reserved16 != 0 || raw.key.reserved32 != 0 { return Err(A::Reserved { actual: 1 }.into()); }
        let key = RecordKey { kind: raw.key.kind, sys_id: raw.key.sys_id };
        let before = unsafe { bytes(raw.before)? };
        let after = unsafe { bytes(raw.after)? };
        match raw.operation {
            0 if before.is_empty() && raw.before.ptr.is_null() => Ok(RecordDeltaView::Insert { key, revision: raw.revision, after }),
            1 => Ok(RecordDeltaView::Update { key, revision: raw.revision, before, after }),
            2 if after.is_empty() && raw.after.ptr.is_null() => Ok(RecordDeltaView::Delete { key, revision: raw.revision, before }),
            0 | 2 => Err(A::ChangeShape.into()),
            actual => Err(A::Flag { actual }.into()),
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventView<'a> { pub event_seq: u32, pub event_kind: u16, pub payload: &'a [u8] }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordDeltaView<'a> {
    Insert { key: RecordKey, revision: u64, after: &'a [u8] },
    Update { key: RecordKey, revision: u64, before: &'a [u8], after: &'a [u8] },
    Delete { key: RecordKey, revision: u64, before: &'a [u8] },
}
