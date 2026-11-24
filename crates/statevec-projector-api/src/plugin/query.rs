//! Engine lookups by unique key or system id. The host answers between applied
//! entries, so a result is one complete engine state at its reported position.
//! That position may be ahead of the entries this projector has consumed.
use super::io::{Held, ReleaseDisposition, ReplySlot, ScopeData, ScopeFailureSlot};
use super::{borrow::{bytes, call}, ffi::*, ProjectorAbiError as A, ProjectorError as F, ProjectorOperation as Op};
use std::{cell::Cell, marker::PhantomData};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineQueryKey<'a> {
    /// Encoded unique-key bytes, as the schema's unique-key encoder produces them.
    UniqueKey { kind: u16, uk_id: u8, key: &'a [u8] },
    /// Found only when the live record has this kind.
    SysId { kind: u16, sys_id: u64 },
}

/// The whole query was refused; nothing partial is returned. The first four
/// come from the engine lookup, the rest from the node before any lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum EngineQueryRejection {
    /// A key names a record kind the engine's schema does not define.
    UnknownKind,
    /// A key names a unique key its record kind does not define.
    UnknownUniqueKey,
    /// The answer would exceed `MAX_ENGINE_QUERY_RECORD_BYTES`.
    ReplyTooLarge,
    /// No live engine can serve the query now.
    EngineUnavailable,
    /// The node has stopped after a failure.
    OwnerFatal,
    /// The node is shutting down.
    OwnerDraining,
    /// No Projector that serves engine queries is attached to the node.
    Unsupported,
    /// The node's initial replay has not reached its recorded commit.
    RecoveryPending,
    /// A snapshot installation is in progress.
    SnapshotInstalling,
    /// This Projector's latest-state rebuild has not been acknowledged.
    ProjectorRebuilding,
    /// The node's own bound on held queries; only a host defect reaches it.
    Busy,
    /// The node has applied no entry yet, so it has no state to read.
    NotYetVisible,
    /// The host rejected the query with a cause this SDK does not know.
    Unrecognized { observed: u32 },
}

#[must_use]
pub enum QueryOffer<'io> {
    Queued(EngineQueryHandle<'io>),
    /// The host table has no engine query callbacks.
    Unsupported,
    /// The host holds `MAX_ENGINE_QUERIES_IN_FLIGHT` queries, or its owner
    /// input is full. A dropped handle's query counts until it is answered.
    OwnerQueueFull,
    Closed,
    /// All four of this scope's query handles are live.
    CapacityExhausted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EngineQueryPosition { pub raft_index: u64, pub raft_term: u64, pub frontier: crate::AppliedTxProgress }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EngineRecordView<'h> { pub kind: u16, pub sys_id: u64, pub revision: u64, pub record: &'h [u8] }

pub enum EngineQueryPoll<'h> { Pending, Ready(EngineQueryResult<'h>), Rejected(EngineQueryRejection), Disconnected }

#[derive(Clone, Copy)]
struct Expected { kind: u16, sys_id: Option<u64> }
#[derive(Clone, Copy)]
enum Terminal { Ready(EngineQueryPosition, statevec_model::SchemaIdentity), Rejected(EngineQueryRejection), Disconnected }

/// Move-only scoped borrow, like a control reply. Ready record bytes stay
/// readable until Drop releases the host result.
#[must_use]
pub struct EngineQueryHandle<'io> {
    host: &'io ScopeData, table: SvEngineQueryHost, cookie: u64, expected: Vec<Expected>, terminal: Option<Terminal>,
    _borrow: PhantomData<&'io mut &'io ()>, _single: PhantomData<Cell<()>>,
}

pub struct EngineQueryResult<'h> { host: &'h ScopeData, table: SvEngineQueryHost, cookie: u64, expected: &'h [Expected], position: EngineQueryPosition, schema: statevec_model::SchemaIdentity }

impl<'io> super::ProjectorControlPort<'io> {
    pub fn engine_query_supported(&self) -> bool { self.host.query.is_some() }

    /// Queue one batch of lookups, answered in key order. Too many keys or an
    /// oversized key is a local error and does not affect the scope.
    pub fn offer_engine_query(&self, keys: &[EngineQueryKey<'_>]) -> Result<QueryOffer<'io>, F> {
        let Some(table) = self.host.query else { return Ok(QueryOffer::Unsupported) };
        self.host.acquisition_allowed()?;
        if keys.is_empty() || keys.len() as u64 > MAX_ENGINE_QUERY_KEYS {
            return Err(A::Length { actual: keys.len() as u64, maximum: MAX_ENGINE_QUERY_KEYS }.into());
        }
        let mut raw = Vec::with_capacity(keys.len());
        let mut expected = Vec::with_capacity(keys.len());
        for key in keys {
            raw.push(match *key {
                EngineQueryKey::UniqueKey { kind, uk_id, key } => {
                    if key.len() as u64 > MAX_ENGINE_QUERY_KEY_BYTES {
                        return Err(A::Length { actual: key.len() as u64, maximum: MAX_ENGINE_QUERY_KEY_BYTES }.into());
                    }
                    expected.push(Expected { kind, sys_id: None });
                    SvEngineQueryKey { tag: query_key_tag::UNIQUE_KEY, kind, uk_id, reserved: 0, sys_id: 0, key: SvBytes::borrowed(key) }
                }
                EngineQueryKey::SysId { kind, sys_id } => {
                    expected.push(Expected { kind, sys_id: Some(sys_id) });
                    SvEngineQueryKey { tag: query_key_tag::SYS_ID, kind, uk_id: 0, reserved: 0, sys_id, key: SvBytes::EMPTY }
                }
            });
        }
        let query = SvEngineQuery { keys: raw.as_ptr(), count: raw.len() as u64 };
        self.host.latch_contract_failure(self.offer_query(table, &query, expected))
    }

    fn offer_query(&self, table: SvEngineQueryHost, query: &SvEngineQuery, expected: Vec<Expected>) -> Result<QueryOffer<'io>, F> {
        let index = {
            let mut slots = self.host.queries.lock().unwrap_or_else(|e| e.into_inner());
            let Some(index) = slots.iter().position(|slot| matches!(slot, ReplySlot::Free)) else { return Ok(QueryOffer::CapacityExhausted); };
            slots[index] = ReplySlot::Offering;
            index
        };
        let (mut tag, mut cookie) = (0, 0);
        // SAFETY: live scoped table; the keys and their bytes outlive this call.
        let result = call(unsafe { table.offer_query.unwrap()(self.host.table.context, query, &mut tag, &mut cookie) }, Op::OfferEngineQuery);
        let mut slots = self.host.queries.lock().unwrap_or_else(|e| e.into_inner());
        slots[index] = ReplySlot::Free;
        let untrusted = |mut slots: std::sync::MutexGuard<'_, [ReplySlot; 4]>| {
            // Retain unknown ownership; never guess that a returned cookie is a
            // live resource safe to destroy.
            if cookie != 0 { slots[index] = ReplySlot::Held(Held { cookie, release: ReleaseDisposition::UntrustedTransfer }); }
            drop(slots);
            self.host.record(ScopeFailureSlot::ReplyRelease, A::InvalidHandle.into());
        };
        if let Err(cause) = result {
            if cookie != 0 { untrusted(slots); }
            return Err(cause);
        }
        match tag {
            offer_tag::QUEUED if cookie != 0 && slots.iter().all(|slot| !matches!(slot, ReplySlot::Held(held) if held.cookie == cookie)) => {
                slots[index] = ReplySlot::Held(Held { cookie, release: ReleaseDisposition::Available });
                Ok(QueryOffer::Queued(EngineQueryHandle { host: self.host, table, cookie, expected, terminal: None, _borrow: PhantomData, _single: PhantomData }))
            }
            offer_tag::OWNER_QUEUE_FULL if cookie == 0 => Ok(QueryOffer::OwnerQueueFull),
            offer_tag::CLOSED if cookie == 0 => Ok(QueryOffer::Closed),
            offer_tag::REPLY_CAPACITY_EXHAUSTED if cookie == 0 => Ok(QueryOffer::CapacityExhausted),
            _ => { untrusted(slots); Err(A::InvalidHandle.into()) }
        }
    }
}

impl EngineQueryHandle<'_> {
    pub fn poll(&mut self) -> Result<EngineQueryPoll<'_>, F> {
        let host = self.host;
        let terminal = host.latch_contract_failure(self.read_info())?;
        Ok(match terminal {
            None => EngineQueryPoll::Pending,
            Some(Terminal::Ready(position, schema)) => EngineQueryPoll::Ready(EngineQueryResult {
                host: self.host, table: self.table, cookie: self.cookie, expected: &self.expected, position, schema,
            }),
            Some(Terminal::Rejected(cause)) => EngineQueryPoll::Rejected(cause),
            Some(Terminal::Disconnected) => EngineQueryPoll::Disconnected,
        })
    }
    fn read_info(&mut self) -> Result<Option<Terminal>, F> {
        if let Some(value) = self.terminal { return Ok(Some(value)); }
        let mut raw = SvEngineQueryInfo::default();
        // SAFETY: this handle retains the sole poll/release ownership of its cookie.
        call(unsafe { self.table.poll_query.unwrap()(self.host.table.context, self.cookie, &mut raw) }, Op::PollEngineQuery)?;
        let value = decode_info(raw, self.expected.len() as u64)?;
        self.terminal = value;
        Ok(value)
    }
}
impl Drop for EngineQueryHandle<'_> { fn drop(&mut self) { self.host.release_query(self.cookie); } }

fn decode_info(raw: SvEngineQueryInfo, count: u64) -> Result<Option<Terminal>, A> {
    let SvEngineQueryInfo { tag, cause, raft_index, raft_term, applied_tx_seq, tx_chain_hash, count: actual, schema, reserved_schema } = raw;
    if reserved_schema != [0; 6] { return Err(A::Reserved { actual: 1 }); }
    if tag != query_reply_tag::READY && schema != SvSchemaIdentity::default() { return Err(A::Reserved { actual: 1 }); }
    let position_unused = raft_index | raft_term | applied_tx_seq | actual | tx_chain_hash.iter().fold(0, |value, byte| value | u64::from(*byte));
    match tag {
        query_reply_tag::PENDING | query_reply_tag::DISCONNECTED | query_reply_tag::REJECTED if position_unused != 0 => Err(A::Reserved { actual: position_unused }),
        query_reply_tag::PENDING | query_reply_tag::DISCONNECTED if cause != 0 => Err(A::Reserved { actual: cause.into() }),
        query_reply_tag::PENDING => Ok(None),
        query_reply_tag::DISCONNECTED => Ok(Some(Terminal::Disconnected)),
        query_reply_tag::REJECTED => Ok(Some(Terminal::Rejected(match cause {
            query_rejection::UNKNOWN_KIND => EngineQueryRejection::UnknownKind,
            query_rejection::UNKNOWN_UNIQUE_KEY => EngineQueryRejection::UnknownUniqueKey,
            query_rejection::REPLY_TOO_LARGE => EngineQueryRejection::ReplyTooLarge,
            query_rejection::ENGINE_UNAVAILABLE => EngineQueryRejection::EngineUnavailable,
            query_rejection::OWNER_FATAL => EngineQueryRejection::OwnerFatal,
            query_rejection::OWNER_DRAINING => EngineQueryRejection::OwnerDraining,
            query_rejection::UNSUPPORTED => EngineQueryRejection::Unsupported,
            query_rejection::RECOVERY_PENDING => EngineQueryRejection::RecoveryPending,
            query_rejection::SNAPSHOT_INSTALLING => EngineQueryRejection::SnapshotInstalling,
            query_rejection::PROJECTOR_REBUILDING => EngineQueryRejection::ProjectorRebuilding,
            query_rejection::BUSY => EngineQueryRejection::Busy,
            query_rejection::NOT_YET_VISIBLE => EngineQueryRejection::NotYetVisible,
            0 => return Err(A::Flag { actual: 0 }),
            observed => EngineQueryRejection::Unrecognized { observed },
        }))),
        query_reply_tag::READY => {
            if cause != 0 { return Err(A::Reserved { actual: cause.into() }); }
            if raft_index == 0 || raft_term == 0 { return Err(A::InvalidPosition); }
            if actual != count { return Err(A::CallbackCount { expected: count, actual }); }
            Ok(Some(Terminal::Ready(EngineQueryPosition {
                raft_index, raft_term, frontier: crate::AppliedTxProgress { applied_tx_seq, tx_chain_hash },
            }, super::borrow::schema(schema))))
        }
        actual => Err(A::Flag { actual }),
    }
}

impl<'h> EngineQueryResult<'h> {
    pub fn schema_identity(&self) -> statevec_model::SchemaIdentity { self.schema }
    pub fn position(&self) -> EngineQueryPosition { self.position }
    pub fn len(&self) -> usize { self.expected.len() }
    pub fn is_empty(&self) -> bool { self.expected.is_empty() }
    /// The record for the key at `index`, or None if no live record matches.
    pub fn record(&self, index: usize) -> Result<Option<EngineRecordView<'h>>, F> {
        let Some(expected) = self.expected.get(index).copied() else {
            return Err(A::Length { actual: index as u64, maximum: self.expected.len() as u64 }.into());
        };
        self.host.latch_contract_failure(self.read_record(index as u64, expected))
    }
    fn read_record(&self, index: u64, expected: Expected) -> Result<Option<EngineRecordView<'h>>, F> {
        let mut raw = SvEngineQueryRecord { found: 0, reserved: 0, key: SvKey { kind: 0, reserved16: 0, reserved32: 0, sys_id: 0 }, revision: 0, record: SvBytes::EMPTY };
        // SAFETY: the handle borrowed for 'h retains this cookie's host result.
        call(unsafe { self.table.query_record.unwrap()(self.host.table.context, self.cookie, index, &mut raw) }, Op::EngineQueryRecord)?;
        let SvEngineQueryRecord { found, reserved, key, revision, record } = raw;
        let reserved = u64::from(reserved) | u64::from(key.reserved16) | u64::from(key.reserved32);
        if reserved != 0 { return Err(A::Reserved { actual: reserved }.into()); }
        match found {
            0 => {
                let unused = u64::from(key.kind) | key.sys_id | revision | record.len;
                if unused != 0 { return Err(A::Reserved { actual: unused }.into()); }
                Ok(None)
            }
            1 => {
                if key.kind != expected.kind || key.sys_id == 0 || expected.sys_id.is_some_and(|sys_id| sys_id != key.sys_id) {
                    return Err(A::InvalidHandle.into());
                }
                // SAFETY: the host keeps a ready result readable until release,
                // which cannot happen while this result borrows its handle.
                let record = unsafe { bytes::<'h>(record) }?;
                Ok(Some(EngineRecordView { kind: key.kind, sys_id: key.sys_id, revision, record }))
            }
            actual => Err(A::Flag { actual }.into()),
        }
    }
}
