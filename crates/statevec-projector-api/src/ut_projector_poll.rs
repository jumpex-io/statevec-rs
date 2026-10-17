//! Measure the generated ABI observer while its real worker is joining a held
//! user service. Setup/other threads are outside the zero-allocation scope.
use super::*;
use ffi::*;
use std::{alloc::{GlobalAlloc, Layout, System}, cell::Cell, ffi::c_void,
    num::NonZeroUsize, sync::{Arc, Mutex, mpsc, atomic::{AtomicUsize, Ordering}}, time::Duration};

thread_local! { static ALLOCATIONS: Cell<Option<usize>> = const { Cell::new(None) }; }
struct Counting;
#[global_allocator]
static ALLOCATOR: Counting = Counting;
fn allocated() { let _ = ALLOCATIONS.try_with(|count| {
    if let Some(value) = count.get() { count.set(Some(value + 1)); }
}); }
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 { allocated(); unsafe { System.alloc(layout) } }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 { allocated(); unsafe { System.alloc_zeroed(layout) } }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 { allocated(); unsafe { System.realloc(ptr, layout, size) } }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) { unsafe { System.dealloc(ptr, layout) } }
}
fn allocations(action: impl FnOnce()) -> usize {
    struct Reset;
    impl Drop for Reset { fn drop(&mut self) { ALLOCATIONS.with(|count| count.set(None)); } }
    ALLOCATIONS.with(|count| assert!(count.replace(Some(0)).is_none()));
    let _reset = Reset;
    action();
    ALLOCATIONS.with(|count| count.get().unwrap())
}

struct Held {
    entered: mpsc::SyncSender<()>, release: Mutex<mpsc::Receiver<()>>, author_calls: AtomicUsize,
}
struct Module(Arc<Held>);
struct Value(Arc<Held>);
impl ProjectorModule for Module {
    const SUPPORTED_HISTORY_MODES: SupportedHistoryModes = SupportedHistoryModes::Continuous;
    type Configured = Value;
    fn configure(&self, _: ProjectorConfig<'_>) -> Result<Value, ConfigureError> { Ok(Value(self.0.clone())) }
}
impl ConfiguredProjector for Value {
    type Prepared = Value;
    fn prepare(&self, _: ProjectorPrepare<'_>) -> Result<Value, ProjectorError> { Ok(Value(self.0.clone())) }
}
impl PreparedProjector for Value {
    fn start_request(&self) -> crate::v1::StartRequest { crate::v1::StartRequest::Continuous { offset: None } }
    fn delivery_limits(&self) -> DeliveryLimits { DeliveryLimits {
        frame_capacity: NonZeroUsize::new(1).unwrap(), maximum_accounted_bytes: NonZeroUsize::new(4096).unwrap(),
    } }
    fn run(self, io: ProjectorIo<'_>) -> Result<(), ProjectorError> {
        self.0.author_calls.fetch_add(1, Ordering::Relaxed);
        io.split().services.spawn(move |_| {
            self.0.author_calls.fetch_add(1, Ordering::Relaxed);
            let receiver = self.0.release.lock().unwrap();
            self.0.entered.send(()).unwrap();
            receiver.recv_timeout(Duration::from_secs(5)).expect("test must explicitly release held service");
            Ok(())
        })?;
        Ok(())
    }
}
unsafe extern "C" fn unused(_: SvBytes, _: SvBytes, _: *mut *mut c_void) -> SvStatus { unreachable!() }
static EXPORT: __export::ProjectorExportTable = __export::ProjectorExportTable::new::<Module>(unused);

#[test]
fn generated_poll_allocates_nothing_and_cannot_join_or_call_a_held_service() {
    let (entered, arrival) = mpsc::sync_channel(1);
    let (release, receiver) = mpsc::channel();
    let held = Arc::new(Held { entered, release: Mutex::new(receiver), author_calls: AtomicUsize::new(0) });
    let host_fixture = io_tests::Fixture::new();
    let host = host_fixture.table();
    let input = SvProjectorPrepare {
        identity: SvProjectionIdentity { manifest: 1, ..Default::default() }, peer_id: 1,
        projector_id: SvProjectorId::encode("history").unwrap(), schema_idl: SvBytes::borrowed(b"{}"),
        learner_only: 1, external_retention: 1, checkpoint_interval_indices: 1, min_capture_residence_ms: 1,
        recovery_mode: 1, reserved: 0,
    };
    let start = SvProjectorStart { identity: input.identity, peer_id: 1, projector_id: input.projector_id, recovery_mode: 1, ..Default::default() };
    let mut discovery = SvDiscovery { table: std::ptr::null(), table_bytes: 0 };
    assert_eq!(unsafe { EXPORT.discover(std::mem::size_of::<SvDiscovery>() as u64, &mut discovery) }, 0);
    let table = unsafe { *discovery.table.cast::<SvProjectorV2>() };
    let (mut config, mut prepared, mut worker) = (std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut());
    let (mut recovery, mut limits) = (SvRecovery::default(), SvDeliveryLimits::default());
    assert_eq!(unsafe { __export::configure(&Module(held.clone()), SvBytes::borrowed(b"{}"), SvBytes::borrowed(b"/tmp"), &mut config) }, SvStatus::SUCCESS);
    assert_eq!(unsafe { table.prepare.unwrap()(config, &input, &mut prepared, &mut recovery, &mut limits) }, SvStatus::SUCCESS);
    assert_eq!(unsafe { table.start.unwrap()(&mut prepared, &host, &start, &mut worker) }, SvStatus::SUCCESS);

    // Release before join even if an assertion fails; the borrowed host lives
    // through both guards. No sleep or timing threshold establishes this cut.
    struct Join { worker: *mut c_void, config: *mut c_void, table: SvProjectorV2 }
    impl Drop for Join { fn drop(&mut self) { unsafe {
        self.table.request_stop.unwrap()(self.worker);
        let mut out = SvProjectorCompletion::EMPTY;
        if self.table.join.unwrap()(&mut self.worker, &mut out) != SvStatus::SUCCESS || out.joined != 1 { std::process::abort(); }
        self.table.destroy_configured.unwrap()(&mut self.config);
    } } }
    struct Release(Option<mpsc::Sender<()>>);
    impl Drop for Release { fn drop(&mut self) { if let Some(send) = self.0.take() { let _ = send.send(()); } } }
    let joined = Join { worker, config, table };
    let released = Release(Some(release));
    arrival.recv_timeout(Duration::from_secs(2)).expect("real scoped service must be held");
    assert_eq!(held.author_calls.load(Ordering::Relaxed), 2);
    assert_eq!(allocations(|| drop(std::hint::black_box(Box::new([0u8; 32])))), 1, "counter control");

    let mut ready = 99;
    let mut failure = SvProjectorFailureObservation::NONE;
    let count = allocations(|| {
        for _ in 0..1024 { // fixed observation workload, not a latency budget
            assert_eq!(unsafe { table.poll_worker.unwrap()(worker, &mut ready, &mut failure) }, SvStatus::SUCCESS);
            assert_eq!((ready, failure), (0, SvProjectorFailureObservation::NONE));
        }
    });
    assert_eq!(count, 0, "generated poll must allocate nothing while the user service is held");
    assert_eq!(held.author_calls.load(Ordering::Relaxed), 2, "poll must not invoke author code");
    drop(released);
    drop(joined);
}
