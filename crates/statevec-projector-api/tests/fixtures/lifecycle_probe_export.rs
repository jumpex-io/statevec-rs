//! Mechanical handle wrappers around the generated table. They retain the
//! callback interposer for exactly the same joined/retained lifetime; completion
//! bytes and ready/observation values are never rewritten by the fixture.
use super::*;
use std::{alloc::{GlobalAlloc, Layout, System}, cell::Cell, mem::{size_of, ManuallyDrop}, ptr, sync::OnceLock};
thread_local! { static ALLOCS: Cell<Option<usize>> = const { Cell::new(None) }; }
struct Counting;
#[global_allocator]
static ALLOCATOR: Counting = Counting;
fn allocated() { let _ = ALLOCS.try_with(|value| { if let Some(n) = value.get() { value.set(Some(n + 1)); } }); }
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 { allocated(); unsafe { System.alloc(l) } }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 { allocated(); unsafe { System.alloc_zeroed(l) } }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 { allocated(); unsafe { System.realloc(p, l, n) } }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) { unsafe { System.dealloc(p, l) } }
}
fn count<T>(f: impl FnOnce() -> T) -> (T, usize) {
    struct Reset;
    impl Drop for Reset { fn drop(&mut self) { ALLOCS.with(|n| n.set(None)); } }
    ALLOCS.with(|n| assert!(n.replace(Some(0)).is_none()));
    let _reset = Reset;
    let value = f();
    (value, ALLOCS.with(|n| n.get().unwrap()))
}
struct Holding { inner: *mut c_void, harness: Arc<Harness> }
struct Worker { inner: *mut c_void, callbacks: Box<callbacks::Callbacks>, _table: Box<SvProjectorHostV1> }
unsafe extern "C" fn unused(_: SvBytes, _: SvBytes, _: *mut *mut c_void) -> SvStatus { unreachable!() }
static GENERATED: __export::ProjectorExportTable = __export::ProjectorExportTable::new::<Module>(unused);
fn generated() -> SvProjectorV1 {
    let mut discovery = SvDiscovery { table: ptr::null(), table_bytes: 0 };
    assert_eq!(unsafe { GENERATED.discover(size_of::<SvDiscovery>() as u64, &mut discovery) }, 0);
    unsafe { *discovery.table.cast::<SvProjectorV1>() }
}
fn caught(f: impl FnOnce() -> SvStatus) -> SvStatus {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(value) => value,
        Err(payload) => { std::mem::forget(payload); (&ProjectorFailure::PluginPanicked).into() }
    }
}
unsafe extern "C" fn configure(config: SvBytes, base: SvBytes, out: *mut *mut c_void) -> SvStatus {
    caught(|| {
        unsafe { *out = ptr::null_mut(); }
        let config: Config = serde_json::from_slice(unsafe { std::slice::from_raw_parts(config.ptr, config.len as usize) }).unwrap();
        let report = Mutex::new(std::fs::File::create(&config.report).unwrap());
        let harness = Arc::new(Harness { config, report, release_observed: AtomicBool::new(false), polls: AtomicUsize::new(0),
            allocations: AtomicUsize::new(0), releases: AtomicUsize::new(0), author_calls: AtomicUsize::new(0) });
        // The counter lives inside the DSO, not just the caller's allocator.
        assert_eq!(count(|| drop(std::hint::black_box(Box::new([0u8; 32])))).1, 1, "allocation counter positive control");
        let mut inner = ptr::null_mut();
        let result = unsafe { __export::configure(&Module(harness.clone()), SvBytes::borrowed(b"{}"), base, &mut inner) };
        if result.is_success() { unsafe { *out = Box::into_raw(Box::new(Holding { inner, harness })).cast(); } }
        result
    })
}
unsafe extern "C" fn prepare(raw: *const c_void, input: *const SvProjectorPrepare, out: *mut *mut c_void, recovery: *mut SvRecovery, limits: *mut SvDeliveryLimits) -> SvStatus {
    caught(|| {
        unsafe { *out = ptr::null_mut(); }
        let c = unsafe { &*raw.cast::<Holding>() };
        let mut inner = ptr::null_mut();
        let result = unsafe { generated().prepare.unwrap()(c.inner, input, &mut inner, recovery, limits) };
        if result.is_success() { unsafe { *out = Box::into_raw(Box::new(Holding { inner, harness: c.harness.clone() })).cast(); } }
        result
    })
}
unsafe extern "C" fn start(raw: *mut *mut c_void, host: *const SvProjectorHostV1, input: *const SvProjectorStart, out: *mut *mut c_void) -> SvStatus {
    caught(|| {
        unsafe { *out = ptr::null_mut(); }
        let c = unsafe { &mut *(*raw).cast::<Holding>() };
        let mut callbacks = Box::new(callbacks::Callbacks { original: unsafe { *host }, harness: c.harness.clone() });
        let table = Box::new(callbacks.table());
        let original = c.inner;
        // A malformed generated transfer (including during a source probe)
        // must not make this interposer free callbacks before the real host
        // can retain them. Default to retained even if our own report panics.
        let mut worker = ManuallyDrop::new(Box::new(Worker { inner: ptr::null_mut(), callbacks, _table: table }));
        let table = &*worker._table as *const SvProjectorHostV1;
        let result = unsafe { generated().start.unwrap()(&mut c.inner, table, input, &mut worker.inner) };
        let unchanged = c.inner == original;
        if c.inner.is_null() {
            drop(unsafe { Box::from_raw((*raw).cast::<Holding>()) });
            unsafe { *raw = ptr::null_mut(); }
        } else if !unchanged {
            // Preserve a changed handle identity instead of normalizing it to
            // an apparently unchanged prepared holding. Old custody is unknown.
            unsafe { *raw = Box::into_raw(Box::new(Holding { inner: c.inner, harness: c.harness.clone() })).cast(); }
        }
        if !worker.inner.is_null() {
            unsafe { *out = Box::into_raw(ManuallyDrop::into_inner(worker)).cast(); }
        } else if !result.is_success() && unchanged {
            drop(ManuallyDrop::into_inner(worker));
        }
        result
    })
}
unsafe extern "C" fn poll(raw: *const c_void, ready: *mut u32, observation: *mut SvProjectorFailureObservation) -> SvStatus {
    let worker = unsafe { &*raw.cast::<Worker>() };
    let (result, allocations) = count(|| unsafe { generated().poll_worker.unwrap()(worker.inner, ready, observation) });
    let h = &worker.callbacks.harness;
    h.polls.fetch_add(1, Ordering::Relaxed);
    h.allocations.fetch_add(allocations, Ordering::Relaxed);
    if matches!(unsafe { (*observation).slot }, 2 | 3 | 4) { h.release_observed.store(true, Ordering::Release); }
    // One test-only status on a budget breach fails the real Node gate, rather
    // than merely logging a measurement that would not influence its oracle.
    if allocations != 0 { (&ProjectorFailure::HostPanicked).into() } else { result }
}
unsafe extern "C" fn stop(raw: *mut c_void) -> SvStatus {
    unsafe { generated().request_stop.unwrap()((*raw.cast::<Worker>()).inner) }
}
unsafe extern "C" fn join(raw: *mut *mut c_void, out: *mut SvProjectorCompletion) -> SvStatus {
    caught(|| {
        let mut worker = ManuallyDrop::new(unsafe { Box::from_raw((*raw).cast::<Worker>()) });
        let result = unsafe { generated().join.unwrap()(&mut worker.inner, out) };
        if worker.inner.is_null() { unsafe { *raw = ptr::null_mut(); } }
        let confirmed = result.is_success() && worker.inner.is_null()
            && unsafe { *out }.decode(None).is_ok_and(|completion| completion.joined);
        let h = &worker.callbacks.harness;
        h.event(format_args!("joined={} polls={} allocations={} releases={} release_observed={} author_calls={}",
            unsafe { (*out).joined }, h.polls.load(Ordering::Relaxed), h.allocations.load(Ordering::Relaxed),
            h.releases.load(Ordering::Relaxed), h.release_observed.load(Ordering::Acquire), h.author_calls.load(Ordering::Relaxed)));
        if confirmed { drop(ManuallyDrop::into_inner(worker)); }
        result
    })
}
unsafe fn destroy(raw: *mut *mut c_void, configured: bool) -> SvStatus {
    caught(|| {
        let mut c = unsafe { Box::from_raw((*raw).cast::<Holding>()) };
        unsafe { *raw = ptr::null_mut(); }
        let table = generated();
        unsafe { if configured { table.destroy_configured.unwrap()(&mut c.inner) } else { table.destroy_prepared.unwrap()(&mut c.inner) } }
    })
}
unsafe extern "C" fn destroy_prepared(raw: *mut *mut c_void) -> SvStatus { unsafe { destroy(raw, false) } }
unsafe extern "C" fn destroy_configured(raw: *mut *mut c_void) -> SvStatus { unsafe { destroy(raw, true) } }
struct Table(SvProjectorV1);
// Immutable table of static functions and static target bytes only.
unsafe impl Sync for Table {}
unsafe impl Send for Table {}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn statevec_projector_plugin_entry_v1(bytes: u64, out: *mut SvDiscovery) -> u32 {
    static TABLE: OnceLock<Table> = OnceLock::new();
    if bytes != size_of::<SvDiscovery>() as u64 || out.is_null() { return 1; }
    let table = TABLE.get_or_init(|| Table(SvProjectorV1 { configure: Some(configure), prepare: Some(prepare), start: Some(start),
        poll_worker: Some(poll), request_stop: Some(stop), join: Some(join), destroy_prepared: Some(destroy_prepared),
        destroy_configured: Some(destroy_configured), ..generated() }));
    unsafe { *out = SvDiscovery { table: (&table.0 as *const SvProjectorV1).cast(), table_bytes: size_of::<SvProjectorV1>() as u64 }; }
    0
}
