//! Mechanical engine-query borrower tests. The fixture host answers from a
//! fixed table; real owner scheduling is tested by the host assembly.
use super::*;
use std::sync::Mutex;

const RECORD: [u8; 4] = [1, 2, 3, 4];
// (tag, kind, uk_id, sys_id, key bytes) as the host received them.
type OfferedKey = (u32, u16, u8, u64, Vec<u8>);

#[repr(C)]
struct QueryFixture {
    base: Fixture,
    info: Mutex<SvEngineQueryInfo>,
    offered: Mutex<Vec<OfferedKey>>,
    record_kind: u16,
    query_releases: AtomicU64,
}
impl QueryFixture {
    fn new(info: SvEngineQueryInfo) -> Self {
        Self { base: Fixture::new(), info: Mutex::new(info), offered: Mutex::new(Vec::new()), record_kind: 3, query_releases: AtomicU64::new(0) }
    }
    fn table(&self) -> SvProjectorHostQuery {
        let mut base = self.base.table();
        base.context = (self as *const Self).cast_mut().cast();
        base.h.provided_caps |= ENGINE_QUERY_CAPABILITY;
        base.h.table_bytes = std::mem::size_of::<SvProjectorHostQuery>() as u64;
        SvProjectorHostQuery { base, query: SvEngineQueryHost {
            offer_query: Some(offer_query), poll_query: Some(poll_query), query_record: Some(query_record), release_query: Some(release_query),
        } }
    }
}
fn rejected(cause: u32) -> SvEngineQueryInfo { SvEngineQueryInfo { tag: query_reply_tag::REJECTED, cause, ..Default::default() } }
fn ready(count: u64) -> SvEngineQueryInfo {
    SvEngineQueryInfo { tag: query_reply_tag::READY, raft_index: 5, raft_term: 2, applied_tx_seq: 9, tx_chain_hash: [7; 32], count, schema: wire_schema(4), ..Default::default() }
}
unsafe fn query_fixture<'a>(raw: *const c_void) -> &'a QueryFixture { unsafe { &*raw.cast::<QueryFixture>() } }
unsafe extern "C" fn offer_query(raw: *mut c_void, query: *const SvEngineQuery, tag: *mut u32, cookie: *mut u64) -> SvStatus {
    let f = unsafe { query_fixture(raw) };
    let query = unsafe { *query };
    let keys = unsafe { std::slice::from_raw_parts(query.keys, query.count as usize) };
    let mut offered = f.offered.lock().unwrap();
    for key in keys {
        let bytes = if key.key.len == 0 { Vec::new() } else { unsafe { std::slice::from_raw_parts(key.key.ptr, key.key.len as usize) }.to_vec() };
        offered.push((key.tag, key.kind, key.uk_id, key.sys_id, bytes));
    }
    unsafe { tag.write(offer_tag::QUEUED); cookie.write(77); }
    SvStatus::SUCCESS
}
unsafe extern "C" fn poll_query(raw: *mut c_void, cookie: u64, out: *mut SvEngineQueryInfo) -> SvStatus {
    if cookie != 77 { return (&ProjectorAbiError::InvalidHandle).into(); }
    unsafe { out.write(*query_fixture(raw).info.lock().unwrap()); }
    SvStatus::SUCCESS
}
// Even indices are found with sys_id 100 + index; odd indices are absent.
unsafe extern "C" fn query_record(raw: *mut c_void, cookie: u64, index: u64, out: *mut SvEngineQueryRecord) -> SvStatus {
    if cookie != 77 { return (&ProjectorAbiError::InvalidHandle).into(); }
    let kind = unsafe { query_fixture(raw) }.record_kind;
    let found = index.is_multiple_of(2);
    unsafe { out.write(SvEngineQueryRecord {
        found: u32::from(found), reserved: 0,
        key: SvKey { kind: if found { kind } else { 0 }, reserved16: 0, reserved32: 0, sys_id: if found { 100 + index } else { 0 } },
        revision: if found { 4 } else { 0 }, record: if found { SvBytes::borrowed(&RECORD) } else { SvBytes::EMPTY },
    }); }
    SvStatus::SUCCESS
}
unsafe extern "C" fn release_query(raw: *mut c_void, cookie: *mut u64) -> SvStatus {
    unsafe { query_fixture(raw) }.query_releases.fetch_add(1, Ordering::SeqCst);
    unsafe { cookie.write(0); }
    SvStatus::SUCCESS
}
unsafe fn with_query_host(table: &SvProjectorHostQuery, run: impl for<'io> FnOnce(ProjectorIo<'io>) -> Result<(), ProjectorError>) -> Result<SvProjectorCompletion, ProjectorAbiError> {
    let start = SvProjectorStart { identity: SvProjectionIdentity { manifest: 1, ..Default::default() }, peer_id: 1,
        projector_id: SvProjectorId::encode("history").unwrap(), recovery_mode: 1, ..Default::default() };
    unsafe { super::super::host::with_query_host(table, &start, run) }
}
const KEY: [u8; 32] = [9; 32];
const KEYS: [EngineQueryKey<'static>; 2] = [
    EngineQueryKey::UniqueKey { kind: 3, uk_id: 0, key: &KEY },
    EngineQueryKey::SysId { kind: 3, sys_id: 55 },
];

#[test]
fn ready_query_reads_each_key_in_order_and_releases_on_drop() {
    let f = QueryFixture::new(ready(2));
    let completion = unsafe { with_query_host(&f.table(), |io| {
        let control = io.split().control;
        assert!(control.engine_query_supported());
        let QueryOffer::Queued(mut handle) = control.offer_engine_query(&KEYS)? else { panic!("fixture queues every query") };
        let EngineQueryPoll::Ready(result) = handle.poll()? else { panic!("fixture is ready") };
        assert_eq!(result.position(), EngineQueryPosition { raft_index: 5, raft_term: 2, frontier: crate::AppliedTxProgress { applied_tx_seq: 9, tx_chain_hash: [7; 32] } });
        assert_eq!(result.len(), 2);
        assert_eq!(result.schema_identity(), super::super::borrow::schema(wire_schema(4)));
        assert_eq!(result.record(0)?, Some(EngineRecordView { kind: 3, sys_id: 100, revision: 4, record: &RECORD }));
        assert_eq!(result.record(1)?, None);
        assert_eq!(result.record(2), Err(ProjectorAbiError::Length { actual: 2, maximum: 2 }.into()));
        drop(handle);
        assert_eq!(f.query_releases.load(Ordering::SeqCst), 1);
        Ok(())
    }) }.unwrap();
    assert_eq!(completion.run_failure, SvStatus::SUCCESS);
    assert_eq!(completion.joined, 1);
    assert_eq!(*f.offered.lock().unwrap(), vec![
        (query_key_tag::UNIQUE_KEY, 3, 0, 0, KEY.to_vec()),
        (query_key_tag::SYS_ID, 3, 0, 55, Vec::new()),
    ]);
}

#[test]
fn forgotten_query_handle_is_released_when_the_scope_ends() {
    let f = QueryFixture::new(SvEngineQueryInfo::default());
    let completion = unsafe { with_query_host(&f.table(), |io| {
        let control = io.split().control;
        let QueryOffer::Queued(mut handle) = control.offer_engine_query(&KEYS)? else { panic!("fixture queues every query") };
        assert!(matches!(handle.poll()?, EngineQueryPoll::Pending));
        std::mem::forget(handle);
        Ok(())
    }) }.unwrap();
    assert_eq!(f.query_releases.load(Ordering::SeqCst), 1);
    assert_eq!(completion.joined, 1);
}

#[test_case::test_case(rejected(1), EngineQueryRejection::UnknownKind; "unknown_kind")]
#[test_case::test_case(rejected(2), EngineQueryRejection::UnknownUniqueKey; "unknown_unique_key")]
#[test_case::test_case(rejected(3), EngineQueryRejection::ReplyTooLarge; "too_large")]
#[test_case::test_case(rejected(4), EngineQueryRejection::EngineUnavailable; "engine_unavailable")]
#[test_case::test_case(rejected(5), EngineQueryRejection::OwnerFatal; "owner_fatal")]
#[test_case::test_case(rejected(6), EngineQueryRejection::OwnerDraining; "owner_draining")]
#[test_case::test_case(rejected(7), EngineQueryRejection::Unsupported; "unsupported")]
#[test_case::test_case(rejected(8), EngineQueryRejection::RecoveryPending; "recovery_pending")]
#[test_case::test_case(rejected(9), EngineQueryRejection::SnapshotInstalling; "snapshot_installing")]
#[test_case::test_case(rejected(10), EngineQueryRejection::ProjectorRebuilding; "rebuilding")]
#[test_case::test_case(rejected(11), EngineQueryRejection::Busy; "busy")]
#[test_case::test_case(rejected(12), EngineQueryRejection::NotYetVisible; "not_yet_visible")]
fn rejection_is_a_terminal_answer(info: SvEngineQueryInfo, expected: EngineQueryRejection) {
    let f = QueryFixture::new(info);
    let completion = unsafe { with_query_host(&f.table(), |io| {
        let control = io.split().control;
        let QueryOffer::Queued(mut handle) = control.offer_engine_query(&KEYS)? else { panic!("fixture queues every query") };
        assert!(matches!(handle.poll()?, EngineQueryPoll::Rejected(cause) if cause == expected));
        Ok(())
    }) }.unwrap();
    assert_eq!(completion.run_failure, SvStatus::SUCCESS);
}

#[test_case::test_case(13; "first_unassigned")]
#[test_case::test_case(u32::MAX; "reserved")]
fn unassigned_query_rejection_keeps_the_run_usable(cause: u32) {
    let fixture = QueryFixture::new(rejected(cause));
    let completion = unsafe {
        with_query_host(&fixture.table(), |io| {
            let control = io.split().control;
            let QueryOffer::Queued(mut rejected) = control.offer_engine_query(&KEYS)? else {
                panic!("fixture queues the rejected query");
            };
            assert!(matches!(
                rejected.poll()?,
                EngineQueryPoll::Rejected(EngineQueryRejection::Unrecognized { observed }) if observed == cause
            ));
            drop(rejected);
            assert_eq!(fixture.query_releases.load(Ordering::SeqCst), 1);

            *fixture.info.lock().unwrap() = ready(2);
            let QueryOffer::Queued(mut following) = control.offer_engine_query(&KEYS)? else {
                panic!("scope queues another query after the rejection");
            };
            let EngineQueryPoll::Ready(result) = following.poll()? else {
                panic!("the following query has a ready answer");
            };
            assert_eq!(result.position().raft_index, 5);
            Ok(())
        })
    }
    .unwrap();
    assert_eq!(completion.run_failure, SvStatus::SUCCESS);
    assert_eq!(completion.reply_release_failure, SvStatus::SUCCESS);
    assert_eq!(completion.joined, 1);
    assert_eq!(fixture.query_releases.load(Ordering::SeqCst), 2);
}

#[test]
fn a_disconnected_answer_is_terminal_without_a_cause() {
    let f = QueryFixture::new(SvEngineQueryInfo { tag: query_reply_tag::DISCONNECTED, ..Default::default() });
    let completion = unsafe { with_query_host(&f.table(), |io| {
        let control = io.split().control;
        let QueryOffer::Queued(mut handle) = control.offer_engine_query(&KEYS)? else { panic!("fixture queues every query") };
        assert!(matches!(handle.poll()?, EngineQueryPoll::Disconnected));
        Ok(())
    }) }.unwrap();
    assert_eq!(completion.run_failure, SvStatus::SUCCESS);
}

#[test_case::test_case(ready(3), ProjectorAbiError::CallbackCount { expected: 2, actual: 3 }; "count_mismatch")]
#[test_case::test_case(SvEngineQueryInfo { raft_index: 0, ..ready(2) }, ProjectorAbiError::InvalidPosition; "zero_index")]
#[test_case::test_case(SvEngineQueryInfo { cause: 1, ..ready(2) }, ProjectorAbiError::Reserved { actual: 1 }; "ready_cause")]
#[test_case::test_case(SvEngineQueryInfo { raft_term: 1, ..Default::default() }, ProjectorAbiError::Reserved { actual: 1 }; "pending_position")]
#[test_case::test_case(SvEngineQueryInfo { cause: 13, ..Default::default() }, ProjectorAbiError::Reserved { actual: 13 }; "pending_cause")]
#[test_case::test_case(SvEngineQueryInfo { tag: query_reply_tag::DISCONNECTED, cause: 13, ..Default::default() }, ProjectorAbiError::Reserved { actual: 13 }; "disconnected_cause")]
#[test_case::test_case(SvEngineQueryInfo { tag: 9, ..Default::default() }, ProjectorAbiError::Flag { actual: 9 }; "unknown_tag")]
#[test_case::test_case(rejected(0), ProjectorAbiError::Flag { actual: 0 }; "zero_rejection_cause")]
#[test_case::test_case(SvEngineQueryInfo { raft_index: 1, ..rejected(13) }, ProjectorAbiError::Reserved { actual: 1 }; "unknown_rejection_with_position")]
#[test_case::test_case(SvEngineQueryInfo { schema: wire_schema(4), ..rejected(13) }, ProjectorAbiError::Reserved { actual: 1 }; "unknown_rejection_with_schema")]
#[test_case::test_case(SvEngineQueryInfo { reserved_schema: [1; 6], ..rejected(13) }, ProjectorAbiError::Reserved { actual: 1 }; "unknown_rejection_with_reserved")]
fn malformed_host_answer_fails_the_scope(info: SvEngineQueryInfo, cause: ProjectorAbiError) {
    let f = QueryFixture::new(info);
    let completion = unsafe { with_query_host(&f.table(), |io| {
        let control = io.split().control;
        let QueryOffer::Queued(mut handle) = control.offer_engine_query(&KEYS)? else { panic!("fixture queues every query") };
        assert!(matches!(handle.poll(), Err(actual) if actual == cause.into()));
        Ok(())
    }) }.unwrap();
    assert_eq!(completion.run_failure, (&ProjectorError::from(cause)).into());
    assert!(f.base.stopped.load(Ordering::SeqCst));
}

#[test]
fn record_of_another_kind_fails_the_scope() {
    let f = QueryFixture { record_kind: 4, ..QueryFixture::new(ready(2)) };
    let completion = unsafe { with_query_host(&f.table(), |io| {
        let control = io.split().control;
        let QueryOffer::Queued(mut handle) = control.offer_engine_query(&KEYS)? else { panic!("fixture queues every query") };
        let EngineQueryPoll::Ready(result) = handle.poll()? else { panic!("fixture is ready") };
        assert_eq!(result.record(0), Err(ProjectorAbiError::InvalidHandle.into()));
        Ok(())
    }) }.unwrap();
    assert_eq!(completion.run_failure, (&ProjectorError::from(ProjectorAbiError::InvalidHandle)).into());
}

#[test]
fn oversized_request_is_a_local_error() {
    let f = QueryFixture::new(ready(2));
    let long = vec![0; MAX_ENGINE_QUERY_KEY_BYTES as usize + 1];
    let completion = unsafe { with_query_host(&f.table(), |io| {
        let control = io.split().control;
        assert!(matches!(control.offer_engine_query(&[]), Err(ProjectorError::AbiContract(ProjectorAbiError::Length { actual: 0, .. }))));
        let key = [EngineQueryKey::UniqueKey { kind: 3, uk_id: 0, key: &long }];
        assert!(matches!(control.offer_engine_query(&key), Err(ProjectorError::AbiContract(ProjectorAbiError::Length { .. }))));
        let many = vec![KEYS[1]; MAX_ENGINE_QUERY_KEYS as usize + 1];
        assert!(matches!(control.offer_engine_query(&many), Err(ProjectorError::AbiContract(ProjectorAbiError::Length { .. }))));
        Ok(())
    }) }.unwrap();
    assert_eq!(completion.run_failure, SvStatus::SUCCESS);
    assert!(f.offered.lock().unwrap().is_empty());
}

#[test]
fn base_table_has_no_query_port() {
    let f = Fixture::new();
    let completion = unsafe { with_host(&f.table(), "history", |io| {
        let control = io.split().control;
        assert!(!control.engine_query_supported());
        assert!(matches!(control.offer_engine_query(&KEYS)?, QueryOffer::Unsupported));
        Ok(())
    }) }.unwrap();
    assert_eq!(completion.run_failure, SvStatus::SUCCESS);
}

#[test]
fn query_capability_and_table_size_must_agree() {
    let f = QueryFixture::new(ready(2));
    let mut table = f.table();
    table.base.h.table_bytes = std::mem::size_of::<SvProjectorHost>() as u64;
    let refused = unsafe { with_query_host(&table, |_| Ok(())) };
    assert!(matches!(refused, Err(ProjectorAbiError::TableSize { .. })));
    let mut base = f.base.table();
    base.h.provided_caps |= ENGINE_QUERY_CAPABILITY;
    let refused = unsafe { with_host(&base, "history", |_| Ok(())) };
    assert!(matches!(refused, Err(ProjectorAbiError::TableSize { .. })));
    let mut table = f.table();
    table.query.query_record = None;
    assert!(matches!(unsafe { with_query_host(&table, |_| Ok(())) }, Err(ProjectorAbiError::NullPointer)));
}

// Development-format literals; Phase 1 freezes them with the rest of the ABI.
#[test_case::test_case(std::mem::size_of::<SvEngineQueryKey>(), 32; "key")]
#[test_case::test_case(std::mem::size_of::<SvEngineQuery>(), 16; "query")]
#[test_case::test_case(std::mem::size_of::<SvEngineQueryInfo>(), 144; "info")]
#[test_case::test_case(std::mem::size_of::<SvEngineQueryRecord>(), 48; "record")]
#[test_case::test_case(std::mem::size_of::<SvEngineQueryHost>(), 32; "callbacks")]
#[test_case::test_case(std::mem::size_of::<SvProjectorHostQuery>(), 216; "table")]
#[test_case::test_case(std::mem::offset_of!(SvProjectorHostQuery, query), 184; "callbacks_after_v2")]
#[test_case::test_case(std::mem::offset_of!(SvEngineQueryInfo, tx_chain_hash), 32; "info_hash")]
#[test_case::test_case(std::mem::offset_of!(SvEngineQueryRecord, record), 32; "record_bytes")]
#[cfg(target_pointer_width = "64")]
fn query_table_extents(actual: usize, expected: usize) { assert_eq!(actual, expected); }
