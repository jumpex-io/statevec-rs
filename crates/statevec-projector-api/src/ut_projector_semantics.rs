//! Raw-table protocol tests, not native provenance/durability evidence.
use super::*;
use crate::v1::control::RaftEntryToken;
use std::{collections::VecDeque, sync::Mutex};
use test_case::test_case;

#[test]
fn latest_declaration_is_local_and_does_not_poison_the_scope() {
    let fixture = Fixture::new();
    let mut table = fixture.table();
    table.h.provided_caps = 0x5f;
    table.h.required_caps = 0x5f;
    let startup = SvProjectorStart {
        identity: SvProjectionIdentity { manifest: 1, ..Default::default() }, peer_id: 1,
        projector_id: SvProjectorId::encode("latest").unwrap(), recovery_mode: 2,
        initialization: 1, rebuild_target: 1, ..Default::default()
    };
    let completion = unsafe { crate::v1::host::with_host(&table, &startup, |io| {
        let mut parts = io.split();
        let Delivery::RaftEntry(loan) = parts.source.try_next()? else { panic!("mechanical latest frame") };
        let cut = loan.token();
        assert!(cut.offset().is_none());
        assert!(matches!(cut.declare_durable(), Err(NotContinuousHistory)));
        assert_eq!(fixture.offers.load(Ordering::SeqCst), 0, "no host declaration call");
        assert!(fixture.calls.lock().unwrap().is_empty(), "no stop/notify failure latch");
        drop(loan);
        assert!(matches!(parts.source.try_next()?, Delivery::Empty), "scope remains usable");
        Ok(())
    }) }.unwrap();
    assert_eq!(completion.run_failure, SvStatus::SUCCESS);
    assert_eq!(completion.joined, 1);
}

#[test_case(ProjectorAbiError::BindingMismatch, 17; "binding")]
#[test_case(ProjectorAbiError::RelativeBaseDir, 18; "path")]
#[test_case(ProjectorAbiError::ZeroPeer, 19; "peer")]
#[test_case(ProjectorAbiError::PartialCheckpointPolicy, 20; "policy")]
#[test_case(ProjectorAbiError::ReplyKindMismatch, 21; "reply_kind")]
#[test_case(ProjectorAbiError::ChangeShape, 22; "change_shape")]
#[test_case(ProjectorAbiError::InvalidPosition, 23; "position")]
#[test_case(ProjectorAbiError::ZeroTerm, 24; "term")]
#[test_case(ProjectorAbiError::ZeroIndex, 25; "index")]
fn new_causes_use_independent_literal_codes(cause: ProjectorAbiError, code: u32) {
    let expected = SvStatus { tag: 9, cause: code, detail: [0; 3] };
    assert_eq!(SvStatus::from(&cause), expected);
    assert_eq!(decode_call(expected, ProjectorOperation::Prepare).unwrap(), Some(ProjectorCallError::Operation(ProjectorError::AbiContract(cause))));
    assert!(decode_call(SvStatus { detail: [1, 0, 0], ..expected }, ProjectorOperation::Prepare).is_err());
}

#[test]
fn shared_wire_names_preserve_independent_numeric_meanings() {
    assert_eq!(PROJECTOR_ABI_MAJOR, 2);
    assert_eq!(PROJECTOR_ENTRY_SYMBOL.to_bytes(), b"statevec_projector_plugin_entry_v2");
    assert_eq!([receive_tag::FRAME, receive_tag::EMPTY, receive_tag::DISCONNECTED, receive_tag::STOP, receive_tag::TIMED_OUT, receive_tag::STATE], [0,1,2,3,4,5]);
    assert_eq!([offer_tag::QUEUED, offer_tag::OWNER_QUEUE_FULL, offer_tag::CLOSED, offer_tag::REPLY_CAPACITY_EXHAUSTED], [0,1,2,3]);
    assert_eq!([state_tag::RECORD, state_tag::END, state_tag::STOP, state_tag::PROGRESS], [0,1,2,3]);
    assert_eq!([reply_tag::PENDING, reply_tag::ACCEPTED, reply_tag::REJECTED, reply_tag::STATUS, reply_tag::DISCONNECTED], [0,1,2,3,4]);
    assert_eq!([recovery_tag::HISTORY_FROM_GENESIS, recovery_tag::HISTORY_FROM_OFFSET, recovery_tag::LATEST_STATE], [0, 1, 2]);
    assert_eq!([recovery_mode::CONTINUOUS_HISTORY, recovery_mode::LATEST_STATE_ONLY], [1, 2]);
}

#[test]
fn local_mode_refusal_is_a_standard_error() {
    let error: &dyn std::error::Error = &NotContinuousHistory;
    assert!(error.source().is_none());
    assert_eq!(error.to_string(), "latest-state entry tokens do not define durable history offsets");
}

#[test]
fn recovery_codec_preserves_literal_tags_and_inactive_fields() {
    use crate::v1::StartRequest;
    let offset = crate::ConsumeOffset { cluster_id: "01".repeat(32), projector_id: "history".into(), raft_term: 2, raft_index: 3 };
    for (value, tag) in [(StartRequest::Continuous { offset: None }, 0),
        (StartRequest::Continuous { offset: Some(offset.clone()) }, 1), (StartRequest::LatestState, 2)] {
        let encoded = SvRecovery::encode(&value).unwrap();
        assert_eq!(encoded.tag, tag);
        assert_eq!(encoded.decode().unwrap(), value);
        if tag != 1 {
            assert_eq!(encoded.offset, SvConsumeOffset::default());
            assert!(SvRecovery { offset: SvConsumeOffset::encode(&offset).unwrap(), ..encoded }.decode().is_err());
        }
    }
    assert!(SvRecovery { tag: 3, ..Default::default() }.decode().is_err());
}

#[test]
fn local_decoders_preserve_distinct_path_offset_and_reply_causes() {
    let relative = SvBytes::borrowed(b"relative/path");
    assert_eq!(unsafe { crate::v1::inputs::base_dir(relative) }, Err(ProjectorAbiError::RelativeBaseDir));
    let mut offset = SvConsumeOffset { cluster: [0; 32], projector_id: SvProjectorId::encode("history").unwrap(), raft_term: 0, raft_index: 1 };
    assert_eq!(offset.decode(), Err(ProjectorAbiError::ZeroTerm));
    offset.raft_term = 1;
    offset.raft_index = 0;
    assert_eq!(offset.decode(), Err(ProjectorAbiError::ZeroIndex));
    assert_eq!(<ControlDecision as crate::v1::control::ReplyValue>::decode(ControlReply::Status(crate::ProjectorStatus::Disabled)), Err(ProjectorAbiError::ReplyKindMismatch));
}

#[derive(Clone, Copy)]
enum OfferAction { Full, Capacity, Queued, Closed, Fail }
#[repr(C)]
struct ReportFixture {
    // C layout guarantees the first field's address. Existing source/cleanup
    // callbacks see Fixture; only the three test control callbacks see this tail.
    base: Fixture,
    actions: Mutex<VecDeque<OfferAction>>,
    replies: Mutex<VecDeque<ControlReply>>,
    offered: Mutex<Vec<[u8; 264]>>,
    order: Mutex<Vec<&'static str>>,
}
impl ReportFixture {
    fn new(actions: &[OfferAction], replies: &[ControlReply]) -> Self {
        Self { base: Fixture::new(), actions: Mutex::new(actions.iter().copied().collect()),
            replies: Mutex::new(replies.iter().copied().collect()), offered: Mutex::new(Vec::new()), order: Mutex::new(Vec::new()) }
    }
    fn table(&self) -> SvProjectorHostV2 {
        let mut table = self.base.table();
        table.context = (self as *const Self).cast_mut().cast();
        table.report_durable_offset = Some(offer_report);
        table.offer_rebuild = Some(offer_rebuild);
        table.poll_reply = Some(poll_report);
        table.release_reply = Some(release_report);
        table
    }
}
unsafe fn script<'a>(raw: *mut c_void) -> &'a ReportFixture { unsafe { &*raw.cast::<ReportFixture>() } }
unsafe fn offer_bytes(raw: *mut c_void, bytes: [u8; 264], tag: *mut u32, cookie: *mut u64) -> SvStatus {
    let f = unsafe { script(raw) };
    f.order.lock().unwrap().push("offer");
    f.offered.lock().unwrap().push(bytes);
    let action = f.actions.lock().unwrap().pop_front();
    let (t, c) = match action {
        Some(OfferAction::Full) => (1, 0), Some(OfferAction::Capacity) => (3, 0),
        Some(OfferAction::Queued) => (0, f.base.offers.fetch_add(1, Ordering::SeqCst) + 10),
        Some(OfferAction::Closed) => (2, 0),
        Some(OfferAction::Fail) | None => return (&ProjectorError::HostPanicked).into(),
    };
    unsafe { tag.write(t); cookie.write(c); } SvStatus::SUCCESS
}
unsafe extern "C" fn offer_report(raw: *mut c_void, value: *const SvDurableOffsetReport, tag: *mut u32, cookie: *mut u64) -> SvStatus {
    unsafe { offer_bytes(raw, (*value).cut.bytes, tag, cookie) }
}
unsafe extern "C" fn offer_rebuild(raw: *mut c_void, value: *const SvRebuildCut, tag: *mut u32, cookie: *mut u64) -> SvStatus {
    unsafe { offer_bytes(raw, (*value).bytes, tag, cookie) }
}
unsafe extern "C" fn poll_report(raw: *mut c_void, _: u64, value: *mut SvControlReply) -> SvStatus {
    let f = unsafe { script(raw) };
    f.order.lock().unwrap().push("poll");
    let Some(reply) = f.replies.lock().unwrap().pop_front() else { return (&ProjectorError::HostPanicked).into(); };
    unsafe { value.write(reply.into()); } SvStatus::SUCCESS
}
unsafe extern "C" fn release_report(raw: *mut c_void, cookie: *mut u64) -> SvStatus {
    unsafe { script(raw) }.order.lock().unwrap().push("release");
    unsafe { release_reply(raw, cookie) }
}
fn declaration() -> DurableOffsetReport {
    RaftEntryToken { raw: SvCut { bytes: [93; 264] }, position: SvPosition::default(),
        projector_id: crate::ProjectorId::new("history").unwrap(), mode: crate::HistoryMode::Continuous }
        .declare_durable().unwrap()
}

#[test]
fn bounded_retry_retains_exact_report_and_releases_before_reoffer() {
    let f = ReportFixture::new(&[OfferAction::Full, OfferAction::Capacity, OfferAction::Queued, OfferAction::Queued],
        &[ControlReply::Pending, ControlReply::Disconnected, ControlReply::Accepted]);
    let completion = unsafe { with_host(&f.table(), "history", |io| {
        let control = io.split().control;
        let report = declaration();
        let mut pending = PendingDurableOffset::new(report);
        for _ in 0..3 { assert_eq!(pending.progress(&control)?, ControlProgress::Pending); }
        assert_eq!(&*f.order.lock().unwrap(), &["offer", "offer", "offer"]);
        assert_eq!(pending.progress(&control)?, ControlProgress::Pending);
        assert_eq!(pending.progress(&control)?, ControlProgress::Pending);
        assert_eq!(&*f.order.lock().unwrap(), &["offer", "offer", "offer", "poll", "poll", "release", "offer"]);
        assert_eq!(pending.report().unwrap().raw, report.raw);
        assert_eq!(pending.progress(&control)?, ControlProgress::Accepted);
        assert!(pending.report().is_none());
        assert_eq!(pending.progress(&control)?, ControlProgress::Accepted, "terminal result performs no callback");
        assert_eq!(*f.offered.lock().unwrap(), vec![report.raw.cut.bytes; 4]);
        Ok(())
    }) }.unwrap();
    assert_eq!(completion.run_failure, SvStatus::SUCCESS);
    assert_eq!(completion.joined, 1);
    assert_eq!(f.base.releases.load(Ordering::SeqCst), 2);
}

#[test_case(OfferAction::Closed; "closed")]
#[test_case(OfferAction::Fail; "call_failure")]
#[test_case(OfferAction::Queued; "owner_rejection")]
fn terminal_result_retains_report_without_retry(action: OfferAction) {
    let f = ReportFixture::new(&[action], &[ControlReply::Rejected(crate::ProjectorControlRejection::OwnerNotRunning)]);
    let _completion = unsafe { with_host(&f.table(), "history", |io| {
        let control = io.split().control;
        let mut pending = PendingDurableOffset::new(declaration());
        let mut result = pending.progress(&control);
        if result == Ok(ControlProgress::Pending) { result = pending.progress(&control); }
        let expected = match action {
            OfferAction::Closed => Ok(ControlProgress::Closed), OfferAction::Fail => Err(ProjectorError::HostPanicked),
            _ => Ok(ControlProgress::Rejected(crate::ProjectorControlRejection::OwnerNotRunning)),
        };
        assert_eq!(result, expected);
        assert_eq!(pending.progress(&control), expected);
        assert_eq!(pending.report().unwrap().raw, declaration().raw);
        assert_eq!(f.offered.lock().unwrap().len(), 1);
        Ok(())
    }) }.unwrap();
}

#[test]
fn rebuild_helper_retries_its_distinct_eof_cut() {
    let f = ReportFixture::new(&[OfferAction::Queued, OfferAction::Queued], &[ControlReply::Disconnected, ControlReply::Accepted]);
    let completion = unsafe { with_host(&f.table(), "history", |io| {
        let control = io.split().control;
        let cut = SnapshotToken { raw: SvRebuildCut { bytes: [41; 264] } };
        let mut pending = PendingSnapshotConfirmation::new(cut);
        assert_eq!(pending.progress(&control)?, ControlProgress::Pending);
        assert_eq!(pending.progress(&control)?, ControlProgress::Pending);
        assert_eq!(pending.token().unwrap().raw, cut.raw);
        assert_eq!(pending.progress(&control)?, ControlProgress::Accepted);
        assert!(pending.token().is_none());
        assert_eq!(*f.offered.lock().unwrap(), vec![cut.raw.bytes; 2]);
        Ok(())
    }) }.unwrap();
    assert_eq!(completion.run_failure, SvStatus::SUCCESS);
    assert_eq!(completion.joined, 1);
}
