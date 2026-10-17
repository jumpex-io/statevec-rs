//! Generated table and real SDK worker tests; no native source is attested here.
use super::*;
use ffi::*;
use std::{ffi::c_void, num::NonZeroUsize, sync::{Arc, Mutex}, time::{Duration, Instant}};

#[derive(Clone, Copy, Debug)]
enum Case { Normal, RunError, RunPanic, ServicePanic, WaitForStop, RecoveryPanic, InvalidRecovery, PrepareHostError, DropPanic }
type Log = Arc<Mutex<Vec<(&'static str, std::thread::ThreadId)>>>;
struct Module { log: Log, case: Case }
struct Config { log: Log, case: Case }
struct Prepared { log: Log, case: Case }
fn note(log: &Log, event: &'static str) { log.lock().unwrap().push((event, std::thread::current().id())); }
struct PanicPayload;
impl Drop for PanicPayload { fn drop(&mut self) { panic!("payload must be retained"); } }
impl ProjectorModule for Module {
    const SUPPORTED_HISTORY_MODES: SupportedHistoryModes = SupportedHistoryModes::Continuous;
    type Configured = Config;
    fn configure(&self, input: ProjectorConfig<'_>) -> Result<Config, ConfigureFailure> {
        assert_eq!(input.bytes, b"{}");
        assert!(input.base_dir.is_absolute());
        note(&self.log, "configure");
        Ok(Config { log: self.log.clone(), case: self.case })
    }
}
impl ConfiguredProjector for Config {
    type Prepared = Prepared;
    fn prepare(&self, input: ProjectorPrepare<'_>) -> Result<Prepared, ProjectorError> {
        assert_eq!(input.projector_id, "history");
        assert_eq!(input.cluster_id, "00".repeat(32));
        assert_eq!(input.schema_idl, b"{}");
        assert!(input.checkpoint_policy.is_some());
        note(&self.log, "prepare");
        if matches!(self.case, Case::PrepareHostError) { return Err(ProjectorError::HostPanicked); }
        Ok(Prepared { log: self.log.clone(), case: self.case })
    }
}
impl Drop for Config { fn drop(&mut self) { note(&self.log, "configured_drop"); } }
impl Drop for Prepared {
    fn drop(&mut self) {
        note(&self.log, "prepared_drop");
        if matches!(self.case, Case::DropPanic) { std::panic::panic_any(PanicPayload); }
    }
}
impl PreparedProjector for Prepared {
    fn start_request(&self) -> crate::v1::StartRequest {
        match self.case {
            Case::RecoveryPanic => std::panic::panic_any(PanicPayload),
            Case::InvalidRecovery | Case::DropPanic => crate::v1::StartRequest::LatestState,
            _ => crate::v1::StartRequest::Continuous { offset: None },
        }
    }
    fn delivery_limits(&self) -> DeliveryLimits {
        DeliveryLimits { frame_capacity: NonZeroUsize::new(1).unwrap(), maximum_accounted_bytes: NonZeroUsize::new(4096).unwrap() }
    }
    fn run(self, io: ProjectorIo<'_>) -> Result<(), ProjectorError> {
        assert_eq!(io.startup_position().peer_id, 1);
        assert_eq!(io.start_instruction(), crate::StartInstruction::DeliverEntries);
        note(&self.log, "run");
        let parts = io.split();
        match self.case {
            Case::RunError => Err(ProjectorError::Application { code: 42 }),
            Case::RunPanic => std::panic::panic_any(PanicPayload),
            Case::ServicePanic => {
                parts.services.spawn(|_| std::panic::panic_any(PanicPayload))?;
                Ok(())
            }
            Case::WaitForStop => {
                let log = self.log.clone();
                parts.services.spawn(move |stop| {
                    while !stop.is_stopping()? { std::thread::park_timeout(Duration::from_millis(1)); }
                    note(&log, "service_return");
                    Ok(())
                })?;
                while !parts.stop.is_stopping()? { std::thread::park_timeout(Duration::from_millis(1)); }
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

unsafe extern "C" fn unused_configure(_: SvBytes, _: SvBytes, _: *mut *mut c_void) -> SvStatus { unreachable!() }
static TABLE: __export::ProjectorExportTable = __export::ProjectorExportTable::new::<Module>(unused_configure);
fn table() -> SvProjectorV2 {
    let mut discovery = SvDiscovery { table: std::ptr::null(), table_bytes: 0 };
    assert_eq!(unsafe { TABLE.discover(std::mem::size_of::<SvDiscovery>() as u64, &mut discovery) }, 0);
    assert_eq!(discovery.table_bytes, 112);
    unsafe { *discovery.table.cast::<SvProjectorV2>() }
}
fn input() -> SvProjectorPrepare {
    SvProjectorPrepare {
        identity: SvProjectionIdentity { manifest: 1, ..Default::default() }, peer_id: 1,
        projector_id: SvProjectorId::encode("history").unwrap(), schema_idl: SvBytes::borrowed(b"{}"),
        learner_only: 1, external_retention: 1, checkpoint_interval_indices: 1, min_capture_residence_ms: 1,
        recovery_mode: 1, reserved: 0,
    }
}
fn startup() -> SvProjectorStart {
    let input = input();
    SvProjectorStart { identity: input.identity, peer_id: input.peer_id, projector_id: input.projector_id, recovery_mode: 1, ..Default::default() }
}

#[test_case::test_case(0, 1, 1, 19; "zero_peer")]
#[test_case::test_case(1, 0, 1, 20; "missing_interval")]
#[test_case::test_case(1, 1, 0, 20; "missing_residence")]
fn prepare_reports_the_actual_context_cause_before_plugin_code(peer: u64, interval: u64, residence: u64, cause: u32) {
    let (mut config, log) = configured(Case::Normal);
    let mut input = input();
    input.peer_id = peer;
    input.checkpoint_interval_indices = interval;
    input.min_capture_residence_ms = residence;
    let mut prepared = std::ptr::dangling_mut::<c_void>();
    let mut recovery = SvRecovery::default();
    let mut limits = SvDeliveryLimits::default();
    let status = unsafe { table().prepare.unwrap()(config, &input, &mut prepared, &mut recovery, &mut limits) };
    assert_eq!(status, SvStatus { tag: 9, cause, detail: [0; 3] });
    assert!(prepared.is_null());
    assert!(!log.lock().unwrap().iter().any(|(event, _)| *event == "prepare"));
    assert_eq!(unsafe { table().destroy_configured.unwrap()(&mut config) }, SvStatus::SUCCESS);
}
fn configured(case: Case) -> (*mut c_void, Log) {
    let log = Arc::new(Mutex::new(Vec::new()));
    let module = Module { log: log.clone(), case };
    let mut raw = std::ptr::null_mut();
    assert_eq!(unsafe { __export::configure(&module, SvBytes::borrowed(b"{}"), SvBytes::borrowed(b"/tmp"), &mut raw) }, SvStatus::SUCCESS);
    assert!(!raw.is_null());
    (raw, log)
}
fn prepared(config: *mut c_void) -> *mut c_void {
    let (mut output, mut recovery, mut limits) = (std::ptr::null_mut(), SvRecovery::default(), SvDeliveryLimits::default());
    assert_eq!(unsafe { table().prepare.unwrap()(config, &input(), &mut output, &mut recovery, &mut limits) }, SvStatus::SUCCESS);
    assert!(!output.is_null());
    assert_eq!(recovery.decode().unwrap(), crate::v1::StartRequest::Continuous { offset: None });
    assert_eq!(limits.decode().unwrap().frame_capacity.get(), 1);
    output
}
struct Running<'a> { worker: *mut c_void, _host: &'a io_tests::Fixture }
impl Drop for Running<'_> {
    fn drop(&mut self) {
        if self.worker.is_null() { return; }
        unsafe { table().request_stop.unwrap()(self.worker); }
        let mut report = SvProjectorCompletion::EMPTY;
        let status = unsafe { table().join.unwrap()(&mut self.worker, &mut report) };
        // A test failure must not release borrowed stack data or loaded code
        // without a confirmed join. Nextest isolates this physical test case.
        if !status.is_success() || report.joined != 1 { std::process::abort(); }
    }
}

#[test]
fn explicit_stop_failure_remains_separate_from_an_existing_run_error() {
    unsafe extern "C" fn fail_stop(_: *mut c_void) -> SvStatus { (&ProjectorError::HostPanicked).into() }
    let fixture = io_tests::Fixture::new();
    let host = SvProjectorHostV2 { stop_receive: Some(fail_stop), ..fixture.table() };
    let (mut config, _) = configured(Case::RunError);
    let mut prepared = prepared(config);
    let mut running = Running { worker: std::ptr::null_mut(), _host: &fixture };
    assert_eq!(unsafe { table().start.unwrap()(&mut prepared, &host, &startup(), &mut running.worker) }, SvStatus::SUCCESS);
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut ready = 0;
    let mut observed = SvProjectorFailureObservation::NONE;
    while ready == 0 && Instant::now() < deadline {
        assert_eq!(unsafe { table().poll_worker.unwrap()(running.worker, &mut ready, &mut observed) }, SvStatus::SUCCESS);
        std::thread::yield_now();
    }
    assert_eq!(ready, 1);
    let stopped = unsafe { table().request_stop.unwrap()(running.worker) };
    let mut report = SvProjectorCompletion::EMPTY;
    assert_eq!(unsafe { table().join.unwrap()(&mut running.worker, &mut report) }, SvStatus::SUCCESS);
    assert_eq!(unsafe { table().destroy_configured.unwrap()(&mut config) }, SvStatus::SUCCESS);
    assert_eq!(report.run_failure, (&ProjectorError::Application { code: 42 }).into());
    assert_eq!(report.joined, 1);
    assert_eq!(stopped, (&ProjectorError::HostPanicked).into(), "explicit stop is a separate returned cause, not a second run error");
    assert_eq!(decode_call(stopped, ProjectorOperation::RequestStop), Ok(Some(ProjectorCallError::Operation(ProjectorError::HostPanicked))));
    assert!(report.decode(decode_observation(observed).unwrap()).is_ok());
}

#[test_case::test_case(None; "delivered")]
#[test_case::test_case(Some(ProjectorError::HostPanicked); "notification_failure_survives_join")]
fn finished_notification_observes_ready_before_the_physical_join(failure: Option<ProjectorError>) {
    use std::sync::mpsc;
    struct Notification {
        entered: mpsc::Sender<()>,
        proceed: Mutex<mpsc::Receiver<()>>,
        result: SvStatus,
    }
    unsafe extern "C" fn stop(_: *mut c_void) -> SvStatus { SvStatus::SUCCESS }
    unsafe extern "C" fn notify(raw: *mut c_void, kind: u32, _: *const SvProjectorFailureObservation) -> SvStatus {
        let notification = unsafe { &*raw.cast::<Notification>() };
        if kind == 2 {
            let _ = notification.entered.send(());
            let _ = notification.proceed.lock().unwrap().recv_timeout(Duration::from_secs(2));
        }
        notification.result
    }
    let (entered, arrived) = mpsc::channel();
    let (release, proceed) = mpsc::channel();
    let notification = Notification { entered, proceed: Mutex::new(proceed), result: failure.as_ref().map(SvStatus::from).unwrap_or(SvStatus::SUCCESS) };
    let fixture = io_tests::Fixture::new();
    let host = SvProjectorHostV2 { context: (&notification as *const Notification).cast_mut().cast(),
        stop_receive: Some(stop), notify: Some(notify), ..fixture.table() };
    let (mut config, _) = configured(Case::Normal);
    let mut prepared = prepared(config);
    let mut running = Running { worker: std::ptr::null_mut(), _host: &fixture };
    assert_eq!(unsafe { table().start.unwrap()(&mut prepared, &host, &startup(), &mut running.worker) }, SvStatus::SUCCESS);

    // Hold the real worker in Finished. The host thread polls independently;
    // the callback does not reenter the plugin, and readiness is not a join.
    let arrival = arrived.recv_timeout(Duration::from_secs(2));
    let (mut ready, mut observed) = (0, SvProjectorFailureObservation::NONE);
    let polled = unsafe { table().poll_worker.unwrap()(running.worker, &mut ready, &mut observed) };
    let stopped = unsafe { table().request_stop.unwrap()(running.worker) };
    let released = release.send(());
    let mut report = SvProjectorCompletion::EMPTY;
    let joined = unsafe { table().join.unwrap()(&mut running.worker, &mut report) };
    let destroyed = unsafe { table().destroy_configured.unwrap()(&mut config) };

    assert!(arrival.is_ok() && released.is_ok(), "the notification cut must be reached and released");
    assert_eq!((polled, ready, observed), (SvStatus::SUCCESS, 1, SvProjectorFailureObservation::NONE), "Finished must wake an already-ready observer");
    assert_eq!((stopped, joined, destroyed), (SvStatus::SUCCESS, SvStatus::SUCCESS, SvStatus::SUCCESS));
    assert!(running.worker.is_null());
    assert_eq!(report.joined, 1, "only actual join confirms retirement");
    assert_eq!(report.decode(None).unwrap().notification_failure, failure);
    assert_eq!(report.run_failure, SvStatus::SUCCESS);
}

#[test_case::test_case(Case::Normal; "normal")]
#[test_case::test_case(Case::RunError; "immediate_run_error_is_accepted")]
#[test_case::test_case(Case::RunPanic; "panicking_payload_is_not_dropped")]
#[test_case::test_case(Case::ServicePanic; "scoped_service_panic")]
#[test_case::test_case(Case::WaitForStop; "stop_then_join_every_borrower")]
fn configured_table_transfers_once_and_returns_physical_completion(case: Case) {
    let fixture = io_tests::Fixture::new();
    let (mut config, log) = configured(case);
    let mut prepared = prepared(config);
    let mut running = Running { worker: std::ptr::null_mut(), _host: &fixture };
    assert_eq!(unsafe { table().start.unwrap()(&mut prepared, &fixture.table(), &startup(), &mut running.worker) }, SvStatus::SUCCESS);
    assert!(prepared.is_null());
    assert!(!running.worker.is_null());
    let deadline = Instant::now() + Duration::from_secs(2);
    while !log.lock().unwrap().iter().any(|(event, _)| *event == "run") && Instant::now() < deadline { std::thread::yield_now(); }
    let mut ready = 99;
    let mut observed = SvProjectorFailureObservation::NONE;
    assert_eq!(unsafe { table().poll_worker.unwrap()(running.worker, &mut ready, &mut observed) }, SvStatus::SUCCESS);
    if matches!(case, Case::WaitForStop) { assert_eq!((ready, observed), (0, SvProjectorFailureObservation::NONE)); }
    assert_eq!(unsafe { table().request_stop.unwrap()(running.worker) }, SvStatus::SUCCESS);
    let mut report = SvProjectorCompletion::EMPTY;
    assert_eq!(unsafe { table().join.unwrap()(&mut running.worker, &mut report) }, SvStatus::SUCCESS);
    assert!(running.worker.is_null());
    assert_eq!(report.joined, 1);
    let decoded = report.decode(decode_observation(observed).unwrap()).unwrap();
    match case {
        Case::RunError => assert_eq!(decoded.run_failure, Some(ProjectorError::Application { code: 42 })),
        Case::RunPanic => assert_eq!(decoded.run_failure, Some(ProjectorError::PluginPanicked)),
        Case::ServicePanic => assert_eq!(decoded.failed_service_joins, 1),
        Case::WaitForStop => assert!(log.lock().unwrap().iter().any(|(event, _)| *event == "service_return")),
        _ => assert_eq!(decoded.run_failure, None),
    }
    assert_eq!(unsafe { table().destroy_configured.unwrap()(&mut config) }, SvStatus::SUCCESS);
    assert!(config.is_null());
    let log = log.lock().unwrap();
    assert_eq!(log.iter().filter(|(event, _)| *event == "prepared_drop").count(), 1);
    assert_eq!(log.iter().filter(|(event, _)| *event == "configured_drop").count(), 1);
    assert_eq!(log.last().unwrap().0, "configured_drop");
    assert_ne!(log.iter().find(|(event, _)| *event == "prepared_drop").unwrap().1, std::thread::current().id());
}

#[test_case::test_case(Case::RecoveryPanic; "recovery_panic")]
#[test_case::test_case(Case::InvalidRecovery; "native_only_recovery")]
#[test_case::test_case(Case::PrepareHostError; "author_illegal_prepare_error")]
#[test_case::test_case(Case::DropPanic; "partial_prepared_drop_panics")]
fn prepare_refusals_transfer_no_handle_or_metadata(case: Case) {
    let (mut config, log) = configured(case);
    let mut out = std::ptr::dangling_mut::<c_void>();
    let mut recovery = SvRecovery { tag: 99, ..Default::default() };
    let mut limits = SvDeliveryLimits { frame_capacity: 99, maximum_accounted_bytes: 99 };
    let result = unsafe { table().prepare.unwrap()(config, &input(), &mut out, &mut recovery, &mut limits) };
    assert!(!result.is_success());
    assert!(decode_call(result, ProjectorOperation::Prepare).unwrap().is_some());
    assert!(out.is_null());
    assert_eq!(recovery, SvRecovery::default());
    assert_eq!(limits, SvDeliveryLimits::default());
    assert_eq!(unsafe { table().destroy_configured.unwrap()(&mut config) }, SvStatus::SUCCESS);
    assert_eq!(log.lock().unwrap().iter().filter(|(event, _)| *event == "prepared_drop").count(), usize::from(!matches!(case, Case::PrepareHostError)));
}

#[test]
fn old_host_major_refuses_before_spawn_and_preserves_prepared() {
    let fixture = io_tests::Fixture::new();
    let (mut config, log) = configured(Case::Normal);
    let mut prepared = prepared(config);
    let original = prepared;
    let mut host = fixture.table();
    host.h.abi_major = 1; // Independent old header; current size/caps/callbacks.
    let mut worker = std::ptr::dangling_mut::<c_void>();
    let result = unsafe { table().start.unwrap()(&mut prepared, &host, &startup(), &mut worker) };
    assert_eq!(result, SvStatus { tag: 9, cause: 4, detail: [2, 1, 0] });
    assert_eq!(prepared, original);
    assert!(worker.is_null());
    assert!(!log.lock().unwrap().iter().any(|(event, _)| *event == "run"));
    assert_eq!(unsafe { table().destroy_prepared.unwrap()(&mut prepared) }, SvStatus::SUCCESS);
    assert_eq!(unsafe { table().destroy_configured.unwrap()(&mut config) }, SvStatus::SUCCESS);
}

#[test_case::test_case(false; "wrong_start_identity")]
#[test_case::test_case(true; "short_host_header")]
fn start_refusal_preserves_identical_prepared_custody(short: bool) {
    let fixture = io_tests::Fixture::new();
    let (mut config, log) = configured(Case::Normal);
    let mut prepared = prepared(config);
    let original = prepared;
    let mut start = startup();
    if !short { start.peer_id = 2; }
    let mut header = fixture.table().h;
    header.table_bytes = std::mem::size_of::<SvHeader>() as u64;
    let full = fixture.table();
    let host = if short { (&header as *const SvHeader).cast() } else { &full };
    let mut worker = std::ptr::dangling_mut::<c_void>();
    let result = unsafe { table().start.unwrap()(&mut prepared, host, &start, &mut worker) };
    assert!(!result.is_success());
    assert_eq!(prepared, original);
    assert!(worker.is_null());
    assert_eq!(unsafe { table().destroy_prepared.unwrap()(&mut prepared) }, SvStatus::SUCCESS);
    assert!(prepared.is_null());
    assert_eq!(unsafe { table().destroy_configured.unwrap()(&mut config) }, SvStatus::SUCCESS);
    let log = log.lock().unwrap();
    assert!(!log.iter().any(|(event, _)| *event == "run"));
    assert!(log.iter().all(|(_, thread)| *thread == std::thread::current().id()));
}

#[cfg(all(target_os = "linux", any(target_arch = "x86_64", target_arch = "aarch64")))]
#[test]
fn physical_worker_spawn_refusal_returns_prepared_to_the_bootstrap_thread() {
    const CHILD: &str = "STATEVEC_PROJECTOR_SPAWN_REFUSAL_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let current = std::thread::current();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", current.name().unwrap(), "--nocapture"])
            .env(CHILD, "1").stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while child.try_wait().unwrap().is_none() && Instant::now() < deadline { std::thread::sleep(Duration::from_millis(5)); }
        if child.try_wait().unwrap().is_none() { child.kill().unwrap(); }
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"), "child must execute the exact regression");
        return;
    }
    let fixture = io_tests::Fixture::new();
    let (mut config, log) = configured(Case::Normal);
    let mut prepared = prepared(config);
    let original = prepared;
    let ins = |code: u32, jt, jf, k| libc::sock_filter { code: code as u16, jt, jf, k };
    let arch = if cfg!(target_arch = "x86_64") { 0xc000_003e } else { 0xc000_00b7 };
    let mut instructions = [
        ins(libc::BPF_LD | libc::BPF_W | libc::BPF_ABS, 0, 0, std::mem::offset_of!(libc::seccomp_data, arch) as u32),
        ins(libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K, 1, 0, arch),
        ins(libc::BPF_RET | libc::BPF_K, 0, 0, libc::SECCOMP_RET_KILL_PROCESS),
        ins(libc::BPF_LD | libc::BPF_W | libc::BPF_ABS, 0, 0, 0),
        ins(libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K, 1, 0, libc::SYS_clone as u32),
        ins(libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K, 0, 1, libc::SYS_clone3 as u32),
        ins(libc::BPF_RET | libc::BPF_K, 0, 0, libc::SECCOMP_RET_ERRNO | libc::EPERM as u32),
        ins(libc::BPF_RET | libc::BPF_K, 0, 0, libc::SECCOMP_RET_ALLOW),
    ];
    let program = libc::sock_fprog { len: instructions.len() as u16, filter: instructions.as_mut_ptr() };
    assert_eq!(unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) }, 0);
    assert_eq!(unsafe { libc::prctl(libc::PR_SET_SECCOMP, libc::SECCOMP_MODE_FILTER, &program) }, 0);
    let mut worker = std::ptr::null_mut();
    let result = unsafe { table().start.unwrap()(&mut prepared, &fixture.table(), &startup(), &mut worker) };
    assert!(matches!(decode_call(result, ProjectorOperation::Start).unwrap(),
        Some(ProjectorCallError::Operation(ProjectorError::Io { stage: ProjectorIoStage::WorkerSpawn, os_code: Some(libc::EPERM), .. }))));
    assert_eq!(prepared, original);
    assert!(worker.is_null());
    assert_eq!(unsafe { table().destroy_prepared.unwrap()(&mut prepared) }, SvStatus::SUCCESS);
    assert_eq!(unsafe { table().destroy_configured.unwrap()(&mut config) }, SvStatus::SUCCESS);
    let log = log.lock().unwrap();
    assert_eq!(log.iter().map(|(event, _)| *event).collect::<Vec<_>>(), ["configure", "prepare", "prepared_drop", "configured_drop"]);
    assert!(log.iter().all(|(_, thread)| *thread == std::thread::current().id()));
}
