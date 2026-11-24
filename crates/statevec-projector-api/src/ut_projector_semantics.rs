//! Raw-table protocol tests, not native provenance/durability evidence.
use super::*;
use crate::plugin::control::RaftEntryToken;
use crate::{ProjectorControlRejection as Rejection, ProjectorStatus as Status};
use std::collections::VecDeque;
use std::sync::Mutex;
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
    let completion = unsafe { crate::plugin::host::with_host(&table, &startup, |io| {
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
    assert_eq!(PROJECTOR_ABI_MAJOR, 1);
    assert_eq!(PROJECTOR_ENTRY_SYMBOL.to_bytes(), b"statevec_projector_entry");
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
    use crate::plugin::StartRequest;
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
    assert_eq!(unsafe { crate::plugin::inputs::base_dir(relative) }, Err(ProjectorAbiError::RelativeBaseDir));
    let mut offset = SvConsumeOffset { cluster: [0; 32], projector_id: SvProjectorId::encode("history").unwrap(), raft_term: 0, raft_index: 1 };
    assert_eq!(offset.decode(), Err(ProjectorAbiError::ZeroTerm));
    offset.raft_term = 1;
    offset.raft_index = 0;
    assert_eq!(offset.decode(), Err(ProjectorAbiError::ZeroIndex));
    assert_eq!(<ControlDecision as crate::plugin::control::ReplyValue>::decode(ControlReply::Status(crate::ProjectorStatus::Disabled)), Err(ProjectorAbiError::ReplyKindMismatch));
}

#[test_case(0, [0, 0], Rejection::NotAttached; "not_attached")]
#[test_case(1, [0, 0], Rejection::CheckpointsDisabled; "checkpoints_disabled")]
#[test_case(2, [0, 0], Rejection::RestoreNotVerified; "restore_not_verified")]
#[test_case(3, [0, 0], Rejection::OwnerNotRunning; "owner_not_running")]
#[test_case(4, [0, 0], Rejection::IdentityMismatch; "identity_mismatch")]
#[test_case(5, [8, 12], Rejection::BeyondApplied { applied: 8, projector: 12 }; "beyond_applied")]
#[test_case(6, [0, 0], Rejection::OffsetRegressed; "offset_regressed")]
#[test_case(7, [0, 0], Rejection::OffsetConflict; "offset_conflict")]
#[test_case(8, [0, 0], Rejection::WrongHistoryMode; "wrong_history_mode")]
#[test_case(9, [0, 0], Rejection::ProjectorIdentityMismatch; "projector_identity_mismatch")]
#[test_case(10, [0, 0], Rejection::NotRebuilding; "not_rebuilding")]
#[test_case(11, [0, 0], Rejection::RebuildNotComplete; "rebuild_not_complete")]
#[test_case(12, [0, 0], Rejection::RebuildMismatch; "rebuild_mismatch")]
#[test_case(13, [8, 12], Rejection::Unrecognized { observed: 13, value0: 8, value1: 12 }; "first_unassigned")]
#[test_case(u32::MAX, [u64::MAX, 1 << 40], Rejection::Unrecognized {
    observed: u32::MAX, value0: u64::MAX, value1: 1 << 40,
}; "reserved")]
fn control_rejection_codec_preserves_wire_values(cause: u32, [value0, value1]: [u64; 2], expected: Rejection) {
    let wire = SvControlReply { tag: 2, cause, value0, value1 };
    assert_eq!(wire.decode(), Ok(ControlReply::Rejected(expected)));
    assert_eq!(SvControlReply::from(ControlReply::Rejected(expected)), wire);
}

#[test_case(0, [0, 0], Status::Disabled; "disabled")]
#[test_case(1, [0, 0], Status::ReplayOnly; "replay_only")]
#[test_case(2, [8, 0], Status::Recovering { projector_raft_index: 8 }; "recovering")]
#[test_case(3, [8, 0], Status::Active { durable_raft_index: 8, engine_checkpoint_blocked_at: None }; "active")]
#[test_case(4, [8, 12], Status::Active { durable_raft_index: 8, engine_checkpoint_blocked_at: Some(12) }; "blocked")]
#[test_case(5, [8, 0], Status::Rebuilding { target_raft_index: 8 }; "rebuilding")]
#[test_case(6, [8, 0], Status::LatestStateActive { baseline_raft_index: 8 }; "latest_active")]
#[test_case(7, [8, 12], Status::Unrecognized { observed: 7, value0: 8, value1: 12 }; "first_unassigned")]
#[test_case(u32::MAX, [1 << 40, u64::MAX], Status::Unrecognized {
    observed: u32::MAX, value0: 1 << 40, value1: u64::MAX,
}; "reserved")]
fn projector_status_codec_preserves_wire_values(cause: u32, [value0, value1]: [u64; 2], expected: Status) {
    let wire = SvControlReply { tag: 3, cause, value0, value1 };
    assert_eq!(wire.decode(), Ok(ControlReply::Status(expected)));
    assert_eq!(SvControlReply::from(ControlReply::Status(expected)), wire);
}

#[derive(Clone, Copy)]
enum OfferAction { Full, Capacity, Queued, Closed, Fail }
#[repr(C)]
struct ReportFixture {
    // C layout guarantees the first field's address. Existing source/cleanup
    // callbacks see Fixture. The scripted control callbacks see this tail.
    base: Fixture,
    actions: Mutex<VecDeque<OfferAction>>,
    replies: Mutex<VecDeque<SvControlReply>>,
    offered: Mutex<Vec<[u8; 264]>>,
    order: Mutex<Vec<&'static str>>,
}
impl ReportFixture {
    fn new(actions: &[OfferAction], replies: &[SvControlReply]) -> Self {
        Self { base: Fixture::new(), actions: Mutex::new(actions.iter().copied().collect()),
            replies: Mutex::new(replies.iter().copied().collect()), offered: Mutex::new(Vec::new()), order: Mutex::new(Vec::new()) }
    }
    fn table(&self) -> SvProjectorHost {
        let mut table = self.base.table();
        table.context = (self as *const Self).cast_mut().cast();
        table.report_durable_offset = Some(offer_report);
        table.offer_rebuild = Some(offer_rebuild);
        table.offer_status = Some(offer_status);
        table.poll_reply = Some(poll_report);
        table.release_reply = Some(release_report);
        table
    }
}
unsafe fn script<'a>(raw: *mut c_void) -> &'a ReportFixture { unsafe { &*raw.cast::<ReportFixture>() } }
unsafe fn scripted_offer(
    raw: *mut c_void,
    bytes: Option<[u8; 264]>,
    tag: *mut u32,
    cookie: *mut u64,
) -> SvStatus {
    let f = unsafe { script(raw) };
    f.order.lock().unwrap().push("offer");
    if let Some(bytes) = bytes {
        f.offered.lock().unwrap().push(bytes);
    }
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
    unsafe { scripted_offer(raw, Some((*value).cut.bytes), tag, cookie) }
}
unsafe extern "C" fn offer_rebuild(raw: *mut c_void, value: *const SvRebuildCut, tag: *mut u32, cookie: *mut u64) -> SvStatus {
    unsafe { scripted_offer(raw, Some((*value).bytes), tag, cookie) }
}
unsafe extern "C" fn offer_status(raw: *mut c_void, tag: *mut u32, cookie: *mut u64) -> SvStatus {
    unsafe { scripted_offer(raw, None, tag, cookie) }
}
unsafe extern "C" fn poll_report(raw: *mut c_void, _: u64, value: *mut SvControlReply) -> SvStatus {
    let f = unsafe { script(raw) };
    f.order.lock().unwrap().push("poll");
    let Some(reply) = f.replies.lock().unwrap().pop_front() else { return (&ProjectorError::HostPanicked).into(); };
    unsafe { value.write(reply); } SvStatus::SUCCESS
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
        &[ControlReply::Pending.into(), ControlReply::Disconnected.into(), ControlReply::Accepted.into()]);
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
    let f = ReportFixture::new(&[action], &[ControlReply::Rejected(Rejection::OwnerNotRunning).into()]);
    let _completion = unsafe { with_host(&f.table(), "history", |io| {
        let control = io.split().control;
        let mut pending = PendingDurableOffset::new(declaration());
        let mut result = pending.progress(&control);
        if result == Ok(ControlProgress::Pending) { result = pending.progress(&control); }
        let expected = match action {
            OfferAction::Closed => Ok(ControlProgress::Closed), OfferAction::Fail => Err(ProjectorError::HostPanicked),
            _ => Ok(ControlProgress::Rejected(Rejection::OwnerNotRunning)),
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
    let f = ReportFixture::new(
        &[OfferAction::Queued, OfferAction::Queued],
        &[ControlReply::Disconnected.into(), ControlReply::Accepted.into()],
    );
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

const UNASSIGNED_REJECTION: SvControlReply = SvControlReply { tag: 2, cause: 13, value0: 8, value1: 12 };
const UNASSIGNED_STATUS: SvControlReply = SvControlReply { tag: 3, cause: 7, value0: 8, value1: 12 };

#[test]
fn unassigned_rejection_retains_exact_durable_offset_report() {
    let fixture = ReportFixture::new(&[OfferAction::Queued], &[UNASSIGNED_REJECTION]);
    let report = declaration();
    let completion = unsafe {
        with_host(&fixture.table(), "history", |io| {
            let control = io.split().control;
            let mut pending = PendingDurableOffset::new(report);
            assert_eq!(pending.progress(&control)?, ControlProgress::Pending);
            let expected = ControlProgress::Rejected(Rejection::Unrecognized { observed: 13, value0: 8, value1: 12 });
            assert_eq!(pending.progress(&control)?, expected);
            assert_eq!(pending.report().unwrap().raw, report.raw);
            assert_eq!(pending.progress(&control)?, expected);
            Ok(())
        })
    }
    .unwrap();
    assert_eq!(completion.run_failure, SvStatus::SUCCESS);
    assert_eq!(completion.reply_release_failure, SvStatus::SUCCESS);
    assert_eq!(completion.joined, 1);
    assert_eq!(*fixture.offered.lock().unwrap(), vec![report.raw.cut.bytes]);
    assert_eq!(&*fixture.order.lock().unwrap(), &["offer", "poll", "release"]);
}

#[test]
fn unassigned_rejection_retains_exact_snapshot_token() {
    let fixture = ReportFixture::new(&[OfferAction::Queued], &[UNASSIGNED_REJECTION]);
    let cut = SnapshotToken { raw: SvRebuildCut { bytes: [41; 264] } };
    let mut table = fixture.table();
    table.h.required_caps = PROJECTOR_BASE_CAPABILITIES | LATEST_STATE_CAPABILITY;
    table.h.provided_caps = table.h.required_caps;
    let startup = SvProjectorStart {
        identity: SvProjectionIdentity { manifest: 1, ..Default::default() },
        peer_id: 1,
        projector_id: SvProjectorId::encode("latest").unwrap(),
        recovery_mode: recovery_mode::LATEST_STATE_ONLY,
        initialization: 1,
        rebuild_target: 1,
        ..Default::default()
    };
    let completion = unsafe {
        crate::plugin::host::with_host(&table, &startup, |io| {
            let control = io.split().control;
            let mut pending = PendingSnapshotConfirmation::new(cut);
            assert_eq!(pending.progress(&control)?, ControlProgress::Pending);
            let expected = ControlProgress::Rejected(Rejection::Unrecognized { observed: 13, value0: 8, value1: 12 });
            assert_eq!(pending.progress(&control)?, expected);
            assert_eq!(pending.token().unwrap().raw, cut.raw);
            assert_eq!(pending.progress(&control)?, expected);
            Ok(())
        })
    }
    .unwrap();
    assert_eq!(completion.run_failure, SvStatus::SUCCESS);
    assert_eq!(completion.reply_release_failure, SvStatus::SUCCESS);
    assert_eq!(completion.joined, 1);
    assert_eq!(*fixture.offered.lock().unwrap(), vec![cut.raw.bytes]);
    assert_eq!(&*fixture.order.lock().unwrap(), &["offer", "poll", "release"]);
}

#[test]
fn unassigned_status_reaches_author_and_releases_control_slot() {
    let fixture = ReportFixture::new(&[OfferAction::Queued], &[UNASSIGNED_STATUS]);
    let completion = unsafe {
        with_host(&fixture.table(), "history", |io| {
            let control = io.split().control;
            let Offer::Queued(mut reply) = control.offer_status()? else {
                panic!("fixture queues the status request");
            };
            let expected = ReplyPoll::Ready(Status::Unrecognized { observed: 7, value0: 8, value1: 12 });
            assert_eq!(reply.poll()?, expected);
            assert_eq!(reply.poll()?, expected);
            drop(reply);
            assert_eq!(fixture.base.releases.load(Ordering::SeqCst), 1);
            Ok(())
        })
    }
    .unwrap();
    assert_eq!(completion.run_failure, SvStatus::SUCCESS);
    assert_eq!(completion.reply_release_failure, SvStatus::SUCCESS);
    assert_eq!(completion.joined, 1);
    assert_eq!(&*fixture.order.lock().unwrap(), &["offer", "poll", "release"]);
}

#[test_case(UNASSIGNED_STATUS; "status_for_control")]
#[test_case(UNASSIGNED_REJECTION; "rejection_for_status")]
fn unassigned_code_does_not_legalize_a_wrong_reply_kind(wire: SvControlReply) {
    let fixture = ReportFixture::new(&[OfferAction::Queued], &[wire]);
    let expected = ProjectorError::AbiContract(ProjectorAbiError::ReplyKindMismatch);
    let completion = unsafe {
        with_host(&fixture.table(), "history", |io| {
            let control = io.split().control;
            let error = if wire.tag == reply_tag::STATUS {
                let Offer::Queued(mut reply) = control.report_durable_offset(&declaration())? else {
                    panic!("fixture queues the control request");
                };
                reply.poll().unwrap_err()
            } else {
                let Offer::Queued(mut reply) = control.offer_status()? else {
                    panic!("fixture queues the status request");
                };
                reply.poll().unwrap_err()
            };
            assert_eq!(error, expected);
            Ok(())
        })
    }
    .unwrap();
    assert_eq!(completion.run_failure, (&expected).into());
    assert_eq!(completion.joined, 1);
    assert_eq!(&*fixture.order.lock().unwrap(), &["offer", "poll", "release"]);
}
