//! Mechanical state-loan protocol, not manufactured native completion evidence.
//! Real EOF/cut provenance is covered by the loaded host/engine assembly.
use super::*;
use super::super::RebuildSourceFailure as R;

#[derive(Clone, Copy)]
enum Fault { None, EarlyEnd, BadInfo, Read, Release, ReleaseRetained, ReleaseMalformed,
    FinishRetained, FinishMalformed, FinishConsumedError, IncompleteConsumed }
#[repr(C)]
struct StateFixture {
    // Existing frame-fixture control/notify callbacks can borrow this prefix.
    base: Fixture,
    info: SvStateInfo,
    reads: AtomicU64,
    finishes: AtomicU64,
    fault: Fault,
}
impl StateFixture {
    fn new(fault: Fault) -> Self {
        Self { base: Fixture::new(), reads: AtomicU64::new(0), finishes: AtomicU64::new(0), fault,
            info: SvStateInfo { position: SvPosition { manifest: 1, raft_index: 4, raft_term: 2, ..Default::default() }, record_count: 1 } }
    }
    fn table(&self) -> SvProjectorHostV2 {
        let mut table = self.base.table();
        table.context = (self as *const Self).cast_mut().cast();
        table.h.provided_caps = PROJECTOR_BASE_CAPABILITIES | LATEST_STATE_CAPABILITY;
        table.h.required_caps = table.h.provided_caps;
        table.receive = Some(state_receive); table.state_info = Some(state_info);
        table.state_next = Some(state_next); table.finish_state = Some(finish);
        table.release_state = Some(release);
        table
    }
    fn start(&self) -> SvProjectorStart {
        SvProjectorStart { identity: SvProjectionIdentity { manifest: 1, ..Default::default() }, peer_id: 1,
            projector_id: SvProjectorId::encode("state").unwrap(), recovery_mode: 2, initialization: 1,
            rebuild_target: self.info.position.raft_index, ..Default::default() }
    }
    fn run(&self, run: impl for<'io> FnOnce(ProjectorIo<'io>) -> Result<(), ProjectorFailure>) -> SvProjectorCompletion {
        unsafe { super::super::host::with_host(&self.table(), &self.start(), run) }.unwrap()
    }
}
unsafe fn state<'a>(raw: *const c_void) -> &'a StateFixture { unsafe { &*raw.cast::<StateFixture>() } }
unsafe extern "C" fn state_receive(raw: *mut c_void, _: u64, tag: *mut u32, cookie: *mut u64) -> SvStatus {
    let f = unsafe { state(raw) };
    let pair = if f.base.stopped.load(Ordering::SeqCst) { (3, 0) }
        else if f.base.delivered.swap(true, Ordering::SeqCst) { (1, 0) } else { (5, 101) };
    unsafe { tag.write(pair.0); cookie.write(pair.1); }
    SvStatus::SUCCESS
}
unsafe extern "C" fn state_info(raw: *mut c_void, _: u64, value: *mut SvStateInfo) -> SvStatus {
    let f = unsafe { state(raw) };
    let mut info = f.info;
    if matches!(f.fault, Fault::BadInfo) { info.position.reserved = 1; }
    unsafe { value.write(info); }
    SvStatus::SUCCESS
}
unsafe extern "C" fn state_next(raw: *mut c_void, _: u64, tag: *mut u32, value: *mut SvStateRecord) -> SvStatus {
    let f = unsafe { state(raw) };
    unsafe { tag.write(0); value.write(std::mem::zeroed()); }
    if matches!(f.fault, Fault::Read) { return (&ProjectorFailure::RebuildSource(R::InvalidRecord)).into(); }
    let n = f.reads.fetch_add(1, Ordering::SeqCst);
    let t = if f.base.stopped.load(Ordering::SeqCst) { 2 }
        else if f.info.record_count == 0 || matches!(f.fault, Fault::EarlyEnd) || n > 1 { 1 }
        else if n == 0 { 3 } else { 0 };
    if t == 0 {
        unsafe { value.write(SvStateRecord { key: SvKey { kind: 7, sys_id: 9, reserved16: 0, reserved32: 0 }, revision: 3, record: SvBytes::borrowed(b"record") }); }
    }
    unsafe { tag.write(t); }
    SvStatus::SUCCESS
}
fn cut(p: SvPosition) -> SvRebuildCut {
    let mut cut = SvRebuildCut::default();
    let b = &mut cut.bytes;
    b[..8].copy_from_slice(b"SVRBLD02"); b[40..72].copy_from_slice(&p.cluster);
    b[72..104].copy_from_slice(&p.genesis); b[104..136].copy_from_slice(&p.profile);
    b[136..140].copy_from_slice(&p.manifest.to_le_bytes()); b[144..152].copy_from_slice(&p.raft_index.to_le_bytes());
    b[152..160].copy_from_slice(&p.raft_term.to_le_bytes()); b[160..192].copy_from_slice(&p.applied_batch_identity);
    b[192..200].copy_from_slice(&p.applied_tx_seq.to_le_bytes()); b[200..232].copy_from_slice(&p.tx_chain_hash);
    cut
}
unsafe extern "C" fn finish(raw: *mut c_void, cookie: *mut u64, value: *mut SvRebuildCut) -> SvStatus {
    let f = unsafe { state(raw) };
    f.finishes.fetch_add(1, Ordering::SeqCst);
    unsafe { value.write(SvRebuildCut::default()); }
    if !matches!(f.fault, Fault::FinishRetained) { unsafe { cookie.write(0); } }
    if matches!(f.fault, Fault::FinishConsumedError) { return (&ProjectorFailure::HostPanicked).into(); }
    if matches!(f.fault, Fault::IncompleteConsumed) { return (&ProjectorFailure::RebuildSource(R::Incomplete)).into(); }
    let mut out = cut(f.info.position);
    if matches!(f.fault, Fault::FinishMalformed) { out.bytes[144] ^= 1; }
    unsafe { value.write(out); }
    SvStatus::SUCCESS
}
unsafe extern "C" fn release(raw: *mut c_void, cookie: *mut u64) -> SvStatus {
    let f = unsafe { state(raw) };
    f.base.frame_releases.fetch_add(1, Ordering::SeqCst);
    if !matches!(f.fault, Fault::ReleaseRetained) { unsafe { cookie.write(0); } }
    if matches!(f.fault, Fault::ReleaseMalformed) { return SvStatus { tag: u32::MAX, cause: 0, detail: [0; 3] }; }
    if matches!(f.fault, Fault::Release | Fault::ReleaseRetained) { return (&ProjectorFailure::HostPanicked).into(); }
    if f.base.stopped.load(Ordering::SeqCst) { SvStatus::SUCCESS }
    else { (&ProjectorFailure::RebuildSource(R::RebuildAbandoned)).into() }
}
fn complete(loan: &mut StateLoan<'_>) {
    for _ in 0..8 {
        match loan.next_record().unwrap() {
            StateRead::End => return,
            StateRead::Record(record) => assert_eq!((record.key.kind, record.key.sys_id, record.revision, record.bytes), (7, 9, 3, &b"record"[..])),
            StateRead::Progress => (),
            other => panic!("complete mechanical source: {other:?}"),
        }
    }
    panic!("bounded fixture EOF");
}

#[test_case::test_case(false; "record_and_progress")]
#[test_case::test_case(true; "empty_genesis")]
fn state_loan_requires_real_end_and_consumes_finish_once(empty: bool) {
    let mut f = StateFixture::new(Fault::None);
    if empty { f.info.record_count = 0; f.info.position.raft_index = 0; f.info.position.raft_term = 0; }
    let c = f.run(|io| {
        let mut source = io.split().source;
        let Delivery::State(mut loan) = source.try_next()? else { panic!("state") };
        assert_eq!(loan.info().record_count, u64::from(!empty));
        assert!(matches!(loan.finish(), Err(ProjectorFailure::RebuildSource(R::Incomplete))));
        assert_eq!(f.finishes.load(Ordering::SeqCst), 0, "local incomplete never calls host finish");
        complete(&mut loan);
        let _cut = loan.finish()?;
        assert!(matches!(loan.finish(), Err(ProjectorFailure::AbiContract(ProjectorAbiFailure::InvalidHandle))));
        assert!(matches!(loan.next_record(), Err(ProjectorFailure::AbiContract(ProjectorAbiFailure::InvalidHandle))));
        Ok(())
    });
    assert_eq!(c.joined, 1); assert_eq!(c.run_failure, SvStatus::SUCCESS); assert_eq!(c.state_release_failure, SvStatus::SUCCESS);
    assert_eq!(f.finishes.load(Ordering::SeqCst), 1); assert_eq!(f.base.frame_releases.load(Ordering::SeqCst), 0);
}

#[test_case::test_case(Fault::None, 1; "abandonment_defers_to_model_error")]
#[test_case::test_case(Fault::Release, 1; "consumed_release_failure")]
#[test_case::test_case(Fault::ReleaseRetained, 0; "unconfirmed_release_failure")]
#[test_case::test_case(Fault::ReleaseMalformed, 0; "malformed_status_cannot_confirm_retirement")]
fn state_drop_keeps_explicit_run_and_release_slots(fault: Fault, joined: u32) {
    let f = StateFixture::new(fault);
    let c = f.run(|io| {
        let mut source = io.split().source;
        let Delivery::State(loan) = source.try_next()? else { panic!("state") };
        drop(loan);
        assert!(!f.base.calls.lock().unwrap().iter().any(|(kind, slot)| (*kind, *slot) == (1, 1)), "abandonment cannot publish run before model returns");
        if matches!(fault, Fault::None) { assert!(matches!(source.try_next()?, Delivery::Stop)); }
        Err(ProjectorFailure::Application { code: 71 })
    });
    assert_eq!(c.run_failure, (&ProjectorFailure::Application { code: 71 }).into());
    assert_eq!(c.joined, joined);
    assert_eq!(c.state_release_failure == SvStatus::SUCCESS, matches!(fault, Fault::None));
    assert_eq!(f.base.frame_releases.load(Ordering::SeqCst), 1, "no retry during final cleanup");
}

#[test_case::test_case(false, false; "ordinary_abandonment")]
#[test_case::test_case(true, false; "forgotten_loan")]
#[test_case::test_case(false, true; "external_stop")]
fn unfinished_state_cannot_be_silent_success(forgotten: bool, stopping: bool) {
    let f = StateFixture::new(Fault::None);
    let c = f.run(|io| {
        let mut source = io.split().source;
        let Delivery::State(mut loan) = source.try_next()? else { panic!("state") };
        if stopping {
            f.base.stopped.store(true, Ordering::SeqCst);
            assert!(matches!(loan.next_record()?, StateRead::Stop));
            assert!(loan.finish().is_err());
        }
        if forgotten { std::mem::forget(loan); } else { drop(loan); }
        Ok(())
    });
    assert_eq!(c.joined, 1); assert_eq!(c.state_release_failure, SvStatus::SUCCESS);
    assert_eq!(c.run_failure, if stopping { SvStatus::SUCCESS } else { (&ProjectorFailure::RebuildSource(R::RebuildAbandoned)).into() });
    assert_eq!(f.base.frame_releases.load(Ordering::SeqCst), 1);
}

#[test]
fn state_abandonment_waits_for_scoped_service_result() {
    let f = StateFixture::new(Fault::None);
    let c = f.run(|io| {
        let ProjectorIoParts { mut source, services, .. } = io.split();
        services.spawn(|stop| {
            while !stop.is_stopping()? { std::thread::yield_now(); }
            Err(ProjectorFailure::Application { code: 72 })
        })?;
        let Delivery::State(loan) = source.try_next()? else { panic!("state") };
        std::mem::forget(loan);
        assert!(matches!(source.try_next(), Err(ProjectorFailure::AbiContract(ProjectorAbiFailure::BorrowConflict))));
        Ok(())
    });
    assert_eq!(c.run_failure, (&ProjectorFailure::Application { code: 72 }).into());
    assert_eq!(c.state_release_failure, SvStatus::SUCCESS);
    assert_eq!(c.service_join_failure, SvStatus::SUCCESS);
    assert_eq!(c.joined, 1);
    assert_eq!(f.base.frame_releases.load(Ordering::SeqCst), 1);
}

#[test]
fn state_abandonment_does_not_replace_reply_cleanup_failure() {
    let mut f = StateFixture::new(Fault::None);
    f.base.fail_reply = true;
    let c = f.run(|io| {
        let ProjectorIoParts { mut source, control, .. } = io.split();
        let Offer::Queued(reply) = control.offer_status()? else { panic!("held reply") };
        let Delivery::State(loan) = source.try_next()? else { panic!("held state") };

        drop(loan); // Provisional abandonment, before an independent cleanup fails.
        drop(reply);
        Ok(())
    });

    assert_eq!(c.run_failure, SvStatus::SUCCESS, "abandonment must not invent a run failure");
    assert_eq!(c.state_release_failure, SvStatus::SUCCESS);
    assert_eq!(c.reply_release_failure, (&ProjectorFailure::HostPanicked).into());
    assert_eq!(c.joined, 1);
    assert!(c.decode(Some(ProjectorWorkerFailure::ReplyRelease(ProjectorFailure::HostPanicked))).is_ok());
    assert_eq!(f.base.releases.load(Ordering::SeqCst), 1);
    assert_eq!(f.base.frame_releases.load(Ordering::SeqCst), 1);
    assert!(!f.base.calls.lock().unwrap().contains(&(1, 1)), "no abandonment Run notification");
}

#[test]
fn state_abandonment_does_not_replace_service_join_failure() {
    let f = StateFixture::new(Fault::None);
    let c = f.run(|io| {
        let ProjectorIoParts { mut source, services, .. } = io.split();
        services.spawn(|stop| {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
            while !stop.is_stopping()? {
                if std::time::Instant::now() >= deadline { return Err(ProjectorFailure::Application { code: 9701 }); }
                std::thread::yield_now();
            }
            panic!("service fails after the unfinished state is released");
        })?;
        let Delivery::State(loan) = source.try_next()? else { panic!("held state") };
        drop(loan);
        Ok(())
    });

    assert_eq!(c.run_failure, SvStatus::SUCCESS, "service join failure outranks provisional abandonment");
    assert_eq!(c.state_release_failure, SvStatus::SUCCESS);
    assert_eq!(c.failed_service_joins, 1);
    assert_eq!(c.joined, 1);
    assert!(c.decode(Some(ProjectorWorkerFailure::ServiceJoinPanicked)).is_ok());
    assert!(!f.base.calls.lock().unwrap().contains(&(1, 1)), "no abandonment Run notification");
}

#[test_case::test_case(3; "source_before_target")]
#[test_case::test_case(5; "source_after_target")]
fn state_acquire_rejects_a_well_formed_source_at_the_wrong_target(source_index: u64) {
    let mut f = StateFixture::new(Fault::None);
    let start = f.start(); // Startup promised S=4; keep this independent of the returned source.
    assert_eq!(start.rebuild_target, 4);
    f.info.position.raft_index = source_index;
    let cause = ProjectorFailure::AbiContract(ProjectorAbiFailure::BindingMismatch);

    let c = unsafe { super::super::host::with_host(&f.table(), &start, |io| {
        let mut source = io.split().source;
        assert!(matches!(source.try_next(), Err(actual) if actual == cause), "well-formed metadata at a different S must refuse acquisition");
        Ok(()) // Handling the error cannot make the malformed host evidence successful.
    }) }.unwrap();

    assert_eq!(c.run_failure, (&cause).into());
    assert_eq!(c.state_release_failure, SvStatus::SUCCESS);
    assert_eq!(c.joined, 1);
    assert_eq!(f.reads.load(Ordering::SeqCst), 0, "do not read the wrong baseline");
    assert_eq!(f.finishes.load(Ordering::SeqCst), 0, "do not request a cut");
    assert_eq!(f.base.frame_releases.load(Ordering::SeqCst), 1, "return acquired custody exactly once");
}

#[test_case::test_case(Fault::BadInfo; "invalid_info")]
#[test_case::test_case(Fault::Read; "explicit_read_failure")]
#[test_case::test_case(Fault::EarlyEnd; "count_mismatch_at_eof")]
fn invalid_state_evidence_never_creates_a_cut(fault: Fault) {
    let f = StateFixture::new(fault);
    let c = f.run(|io| {
        let mut source = io.split().source;
        match source.try_next() {
            Err(_) if matches!(fault, Fault::BadInfo) => (),
            Ok(Delivery::State(mut loan)) => {
                assert!(loan.next_record().is_err());
                assert!(loan.finish().is_err());
            }
            _ => panic!("expected typed source refusal"),
        }
        Ok(()) // Cannot hide malformed evidence by handling the Result.
    });
    assert_ne!(c.run_failure, SvStatus::SUCCESS); assert_eq!(c.joined, 1);
    assert_eq!(c.state_release_failure, SvStatus::SUCCESS); assert_eq!(f.finishes.load(Ordering::SeqCst), 0);
}

#[test_case::test_case(Fault::FinishRetained, 0; "success_did_not_consume")]
#[test_case::test_case(Fault::FinishMalformed, 0; "bad_cut_with_zero_cookie")]
#[test_case::test_case(Fault::FinishConsumedError, 1; "known_error_after_claim")]
#[test_case::test_case(Fault::IncompleteConsumed, 0; "preclaim_incomplete_cannot_consume")]
fn finish_uses_status_and_cookie_as_one_disposition(fault: Fault, joined: u32) {
    let f = StateFixture::new(fault);
    let c = f.run(|io| {
        let mut source = io.split().source;
        let Delivery::State(mut loan) = source.try_next()? else { panic!("state") };
        complete(&mut loan);
        assert!(loan.finish().is_err());
        Ok(())
    });
    assert_eq!(c.joined, joined); assert_eq!(f.finishes.load(Ordering::SeqCst), 1);
    assert_eq!(f.base.frame_releases.load(Ordering::SeqCst), 0, "never retry unknown/consumed custody");
    assert_ne!(if joined == 1 { c.run_failure } else { c.state_release_failure }, SvStatus::SUCCESS);
}
