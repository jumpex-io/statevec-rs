//! Mechanical ABI borrower/cleanup tests only. These tables never attest a
//! native cut; real committed-frame provenance is tested by the host assembly.
use super::*;
use ffi::*;
use std::{ffi::c_void, sync::atomic::{AtomicBool, AtomicU64, Ordering}};
struct Fixture {
    stopped: AtomicBool, delivered: AtomicBool, offers: AtomicU64, releases: AtomicU64,
    frame_releases: AtomicU64, retain_frame: bool, fail_frame: bool, fail_reply: bool,
    receives: AtomicU64, fail_stop: bool, info_failure: Option<ProjectorFailure>, bad_frame: bool,
}
impl Fixture {
    fn new() -> Self { Self { stopped: AtomicBool::new(false), delivered: AtomicBool::new(false), offers: AtomicU64::new(0), releases: AtomicU64::new(0), frame_releases: AtomicU64::new(0), retain_frame: false, fail_frame: false, fail_reply: false, receives: AtomicU64::new(0), fail_stop: false, info_failure: None, bad_frame: false } }
    fn table(&self) -> SvProjectorHostV1 {
        SvProjectorHostV1 {
            h: SvHeader { abi_major: 1, kind: PROJECTOR_HOST_KIND, table_bytes: std::mem::size_of::<SvProjectorHostV1>() as u64, provided_caps: 0x3f, required_caps: 0x3f }, context: (self as *const Self).cast_mut().cast(),
            receive: Some(receive), stop_receive: Some(stop), frame_info: Some(info), transaction: Some(transaction), event: Some(event), change: Some(change), release_frame: Some(release_frame),
            offer_checkpoint: Some(checkpoint), offer_status: Some(offer), poll_reply: Some(poll), release_reply: Some(release_reply), stopping: Some(stopping), notify: Some(notify),
            state_info: Some(state_info), state_next: Some(state_next), finish_state: Some(finish_state), release_state: Some(release_frame), offer_rebuild: Some(rebuild),
        }
    }
}
unsafe fn fixture<'a>(raw: *const c_void) -> &'a Fixture { unsafe { &*raw.cast::<Fixture>() } }
unsafe extern "C" fn receive(raw: *mut c_void, _: u64, tag: *mut u32, cookie: *mut u64) -> SvStatus {
    let f = unsafe { fixture(raw) };
    f.receives.fetch_add(1, Ordering::SeqCst);
    let (t, c) = if f.stopped.load(Ordering::SeqCst) { (3, 0) } else if f.delivered.swap(true, Ordering::SeqCst) { (1, 0) } else { (0, 1) };
    unsafe { tag.write(t); cookie.write(c); } SvStatus::SUCCESS
}
unsafe extern "C" fn stop(raw: *mut c_void) -> SvStatus {
    let f = unsafe { fixture(raw) };
    if f.fail_stop { return (&ProjectorFailure::HostPanicked).into(); }
    f.stopped.store(true, Ordering::SeqCst); SvStatus::SUCCESS
}
unsafe extern "C" fn stopping(raw: *const c_void, out: *mut u32) -> SvStatus { unsafe { out.write(u32::from(fixture(raw).stopped.load(Ordering::SeqCst))); } SvStatus::SUCCESS }
unsafe extern "C" fn notify(_: *mut c_void, kind: u32, observation: *const SvProjectorFailureObservation) -> SvStatus {
    if observation.is_null() { return (&ProjectorAbiFailure::NullPointer).into(); }
    let value = unsafe { *observation };
    if kind == 2 && (value.slot != 0 || value.reserved != 0 || value.cause != SvStatus::SUCCESS) {
        return (&ProjectorAbiFailure::InvalidHandle).into();
    }
    SvStatus::SUCCESS
}
unsafe extern "C" fn info(raw: *mut c_void, _: u64, out: *mut SvFrameInfo) -> SvStatus {
    let f = unsafe { fixture(raw) };
    if let Some(cause) = f.info_failure { return (&cause).into(); }
    let mut frame = SvFrameInfo::default();
    frame.reserved = u32::from(f.bad_frame);
    frame.position.manifest = 1; frame.position.raft_index = 1; frame.position.raft_term = 1; frame.peer_id = 1;
    frame.cut.bytes[..8].copy_from_slice(b"SVCUT002");
    frame.cut.bytes[136..140].copy_from_slice(&1u32.to_le_bytes());
    frame.cut.bytes[144..152].copy_from_slice(&1u64.to_le_bytes()); frame.cut.bytes[152..160].copy_from_slice(&1u64.to_le_bytes());
    unsafe { out.write(frame); } SvStatus::SUCCESS
}
unsafe extern "C" fn release_frame(raw: *mut c_void, cookie: *mut u64) -> SvStatus {
    let f = unsafe { fixture(raw) }; f.frame_releases.fetch_add(1, Ordering::SeqCst);
    if !f.retain_frame { unsafe { cookie.write(0); } }
    if f.fail_frame { (&ProjectorFailure::HostPanicked).into() } else { SvStatus::SUCCESS }
}
unsafe extern "C" fn offer(raw: *mut c_void, tag: *mut u32, cookie: *mut u64) -> SvStatus {
    let c = unsafe { fixture(raw) }.offers.fetch_add(1, Ordering::SeqCst) + 10;
    unsafe { tag.write(0); cookie.write(c); } SvStatus::SUCCESS
}
unsafe extern "C" fn checkpoint(raw: *mut c_void, _: *const SvOffsetCheckpoint, tag: *mut u32, cookie: *mut u64) -> SvStatus { unsafe { offer(raw, tag, cookie) } }
unsafe extern "C" fn rebuild(raw: *mut c_void, _: *const SvRebuildCut, tag: *mut u32, cookie: *mut u64) -> SvStatus { unsafe { offer(raw, tag, cookie) } }
unsafe extern "C" fn poll(_: *mut c_void, _: u64, value: *mut SvControlReply) -> SvStatus {
    unsafe { value.write(ControlReply::Status(crate::ProjectorCheckpointStatus::Active { durable_raft_index: 0, waiting_for_projector: None }).into()); } SvStatus::SUCCESS
}
unsafe extern "C" fn release_reply(raw: *mut c_void, cookie: *mut u64) -> SvStatus {
    let f = unsafe { fixture(raw) }; f.releases.fetch_add(1, Ordering::SeqCst);
    let fail = f.fail_reply && unsafe { *cookie } == 10;
    unsafe { cookie.write(0); }
    if fail { (&ProjectorFailure::HostPanicked).into() } else { SvStatus::SUCCESS }
}
macro_rules! unused { ($name:ident($($ty:ty),*)) => { unsafe extern "C" fn $name($(_: $ty),*) -> SvStatus { (&ProjectorAbiFailure::InvalidHandle).into() } }; }
unused!(transaction(*mut c_void, u64, u64, *mut SvTransactionInfo));
unused!(event(*mut c_void, u64, u64, u64, *mut SvEvent));
unused!(change(*mut c_void, u64, u64, u64, *mut SvRecordChange));
unused!(state_info(*mut c_void, u64, *mut SvStateInfo));
unused!(state_next(*mut c_void, u64, *mut u32, *mut SvStateRecord));
unused!(finish_state(*mut c_void, *mut u64, *mut SvRebuildCut));

#[test_case::test_case(false; "consumed_error_confirms_no_borrow")]
#[test_case::test_case(true; "unconfirmed_custody_is_not_retried")]
fn release_failure_does_not_erase_the_later_explicit_application_error(retain: bool) {
    let f = Fixture { fail_frame: true, retain_frame: retain, ..Fixture::new() };
    let completion = unsafe { with_host(&f.table(), "history", |io| {
        let mut source = io.split().source;
        let Delivery::Frame(_loan) = source.try_next()? else { panic!("one mechanical test loan") };
        Err(ProjectorFailure::Application { code: 42 })
    }) }.unwrap();
    assert_eq!(completion.run_failure, (&ProjectorFailure::Application { code: 42 }).into());
    assert_eq!(completion.frame_release_failure, (&ProjectorFailure::HostPanicked).into());
    assert_eq!(completion.joined, u32::from(!retain));
    assert_eq!(f.frame_releases.load(Ordering::SeqCst), 1, "never retry a release with uncertain custody");
    assert!(completion.decode(Some(ProjectorWorkerFailure::FrameRelease(ProjectorFailure::HostPanicked))).is_ok());
    assert!(f.stopped.load(Ordering::SeqCst));
}

#[test_case::test_case(false; "caught_host_panic")]
#[test_case::test_case(true; "malformed_success_dto")]
fn ignored_callback_failure_cannot_be_returned_as_success(bad_frame: bool) {
    let cause = if bad_frame { ProjectorAbiFailure::Reserved { actual: 1 }.into() } else { ProjectorFailure::HostPanicked };
    let f = Fixture { info_failure: if bad_frame { None } else { Some(cause) }, bad_frame, ..Fixture::new() };
    let completion = unsafe { with_host(&f.table(), "history", |io| {
        let mut parts = io.split();
        assert!(matches!(parts.source.try_next(), Err(actual) if actual == cause));
        assert!(matches!(parts.source.try_next(), Err(actual) if actual == cause));
        assert!(matches!(parts.control.offer_status(), Err(actual) if actual == cause));
        Ok(())
    }) }.unwrap();
    assert_eq!(completion.run_failure, (&cause).into());
    assert_eq!(completion.joined, 1);
    assert_eq!(f.receives.load(Ordering::SeqCst), 1);
    assert_eq!(f.offers.load(Ordering::SeqCst), 0);
    assert_eq!(f.frame_releases.load(Ordering::SeqCst), 1);
    assert!(f.stopped.load(Ordering::SeqCst));
}

#[test]
fn release_failure_revokes_new_acquisitions_even_if_stop_callback_refuses() {
    let f = Fixture { fail_frame: true, fail_stop: true, ..Fixture::new() };
    let completion = unsafe { with_host(&f.table(), "history", |io| {
        let mut parts = io.split();
        let Delivery::Frame(loan) = parts.source.try_next()? else { panic!("first frame") };
        drop(loan);
        assert!(matches!(parts.source.try_next(), Err(ProjectorFailure::HostPanicked)));
        assert!(matches!(parts.control.offer_status(), Err(ProjectorFailure::HostPanicked)));
        Err(ProjectorFailure::Application { code: 42 })
    }) }.unwrap();
    assert_eq!(completion.run_failure, (&ProjectorFailure::Application { code: 42 }).into());
    assert_eq!(completion.frame_release_failure, (&ProjectorFailure::HostPanicked).into());
    assert_eq!(f.receives.load(Ordering::SeqCst), 1);
    assert_eq!(f.offers.load(Ordering::SeqCst), 0);
    assert_eq!(completion.joined, 1);
}

#[test]
fn finished_notification_uses_canonical_zero_observation() {
    let fixture = Fixture::new();
    let completion = unsafe { with_host(&fixture.table(), "history", |_| Ok(())) }.unwrap();
    assert_eq!(completion.joined, 1);
    assert_eq!(completion.notification_failure, SvStatus::SUCCESS);
}

#[test]
fn forgotten_replies_remain_bounded_and_all_four_releases_run_after_one_fails() {
    let f = Fixture { fail_reply: true, ..Fixture::new() };
    let completion = unsafe { with_host(&f.table(), "history", |io| {
        let control = io.split().control;
        for _ in 0..4 {
            let Offer::Queued(reply) = control.offer_status()? else { panic!("four slots") };
            std::mem::forget(reply);
        }
        assert!(matches!(control.offer_status()?, Offer::ReplyCapacityExhausted));
        Ok(())
    }) }.unwrap();
    assert_eq!(f.offers.load(Ordering::SeqCst), 4);
    assert_eq!(f.releases.load(Ordering::SeqCst), 4);
    assert_eq!(completion.reply_release_failure, (&ProjectorFailure::HostPanicked).into());
    assert_eq!(completion.run_failure, SvStatus::SUCCESS);
    assert_eq!(completion.joined, 1);
}

#[test]
fn scoped_service_keeps_its_reply_borrow_until_actual_join() {
    let f = Fixture::new();
    let finished = std::sync::Arc::new(AtomicBool::new(false));
    let completion = unsafe { with_host(&f.table(), "history", |io| {
        let parts = io.split(); let control = parts.control.clone();
        let completed = finished.clone();
        parts.services.spawn(move |stop| {
            let Offer::Queued(mut reply) = control.offer_status()? else { panic!("one reply") };
            while !stop.is_stopping()? { std::thread::yield_now(); }
            assert!(matches!(reply.poll()?, ReplyPoll::Ready(_)));
            drop(reply); completed.store(true, Ordering::SeqCst); Ok(())
        })?;
        Ok(())
    }) }.unwrap();
    assert!(finished.load(Ordering::SeqCst));
    assert_eq!(f.releases.load(Ordering::SeqCst), 1);
    assert_eq!(completion.joined, 1);
    assert!(completion.decode(None).is_ok());
}

#[test_case::test_case(0, 1; "zero_frames")]
#[test_case::test_case(1, 0; "zero_bytes")]
fn zero_delivery_bounds_refuse(frames: u64, bytes: u64) {
    assert!(SvDeliveryLimits { frame_capacity: frames, maximum_accounted_bytes: bytes }.decode().is_err());
}
#[test_case::test_case(SvControlReply { tag: 3, cause: 2, value0: 0, value1: 0 }; "zero_recovering_offset")]
#[test_case::test_case(SvControlReply { tag: 3, cause: 4, value0: 9, value1: 9 }; "waiting_not_after_durable")]
#[test_case::test_case(SvControlReply { tag: 3, cause: 3, value0: 9, value1: 1 }; "unused_status_slot")]
#[test_case::test_case(SvControlReply { tag: 1, cause: 0, value0: 1, value1: 0 }; "unused_accepted_slot")]
fn noncanonical_control_replies_refuse(reply: SvControlReply) { assert!(reply.decode().is_err()); }

#[test]
fn recovery_and_readiness_values_round_trip_without_native_authority() {
    for recovery in [crate::ProjectorRecovery::Continuous { offset: None }, crate::ProjectorRecovery::LatestState] {
        assert_eq!(SvRecovery::encode(&recovery).unwrap().decode().unwrap(), recovery);
    }
    assert!(SvRecovery::encode(&crate::ProjectorRecovery::ReplayOnly).is_err());
    let mut inactive = SvRecovery::default(); inactive.offset.raft_index = 1;
    assert!(inactive.decode().is_err());
    for value in [crate::ProjectorCheckpointStatus::Recovering { projector_raft_index: 9 }, crate::ProjectorCheckpointStatus::Active { durable_raft_index: 9, waiting_for_projector: Some(10) }] {
        assert_eq!(SvControlReply::from(ControlReply::Status(value)).decode().unwrap(), ControlReply::Status(value));
    }
}
