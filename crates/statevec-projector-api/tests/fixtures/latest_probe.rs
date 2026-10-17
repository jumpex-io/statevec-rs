//! Linux qualification only: pause the unchanged public latest plugin at an
//! actual state read or its first rebuild offer. No model, source, EOF, cut,
//! native status or ACK is synthesized. The harness owns the SIGKILL decision.
#![cfg(target_os = "linux")]
#[path = "../../examples/projector_v2_latest/plugin.rs"]
mod plugin;
use statevec_projector_api::{ProjectorStatus, v1::{__export, host::ControlReply, ProjectorError, ffi::*}};
use std::{ffi::c_void, io::{Read, Write}, mem::{ManuallyDrop, size_of}, os::unix::net::UnixStream,
    path::PathBuf, ptr, sync::{Mutex, OnceLock, atomic::{AtomicBool, AtomicUsize, Ordering}}, time::{Duration, Instant}};

struct Settings { socket: PathBuf, report: PathBuf }
static SETTINGS: OnceLock<Settings> = OnceLock::new();
fn caught(f: impl FnOnce() -> SvStatus) -> SvStatus {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(result) => result,
        Err(payload) => { std::mem::forget(payload); (&ProjectorError::PluginPanicked).into() }
    }
}
unsafe extern "C" fn configure(config: SvBytes, base_dir: SvBytes, out: *mut *mut c_void) -> SvStatus {
    caught(|| {
        let result = unsafe { __export::configure(&plugin::Latest, config, base_dir, out) };
        if result.is_success() {
            // Real configure has validated these borrowed spans and JSON. The
            // fixture only locates its control socket/output, not recovery data.
            let bytes = unsafe { std::slice::from_raw_parts(config.ptr, config.len as usize) };
            let base = unsafe { std::slice::from_raw_parts(base_dir.ptr, base_dir.len as usize) };
            let base = PathBuf::from(std::str::from_utf8(base).unwrap());
            let value: serde_json::Value = serde_json::from_slice(bytes).unwrap();
            let report = base.join(value["report_path"].as_str().unwrap());
            assert!(SETTINGS.set(Settings { socket: base.join("latest-probe.sock"), report }).is_ok(),
                "one configured test attachment per Node process");
        }
        result
    })
}
static BASE: __export::ProjectorExportTable = __export::ProjectorExportTable::new::<plugin::Latest>(configure);
fn base() -> SvProjectorV2 {
    let mut out = SvDiscovery { table: ptr::null(), table_bytes: 0 };
    assert_eq!(unsafe { BASE.discover(size_of::<SvDiscovery>() as u64, &mut out) }, 0);
    unsafe { *out.table.cast::<SvProjectorV2>() }
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Cut { BeforeInstall, BeforeOffer }
struct Proxy {
    original: SvProjectorHostV2,
    channel: Mutex<UnixStream>,
    cut: Cut,
    target: u64,
    output_start: usize,
    paused: AtomicBool,
    records: AtomicUsize,
    eof: AtomicBool,
    finished: AtomicBool,
}
impl Proxy {
    fn pause(&self, label: &str) {
        if self.paused.swap(true, Ordering::SeqCst) { return; }
        let output = std::fs::read(&SETTINGS.get().unwrap().report).unwrap();
        let current = &output[self.output_start..];
        let records = self.records.load(Ordering::SeqCst);
        assert!(records > 0, "cut must follow a real populated source read");
        match self.cut {
            Cut::BeforeInstall => {
                assert!(current.is_empty(), "no model/reset before first record is returned");
                assert!(!self.eof.load(Ordering::SeqCst) && !self.finished.load(Ordering::SeqCst));
            }
            Cut::BeforeOffer => {
                assert!(self.eof.load(Ordering::SeqCst) && self.finished.load(Ordering::SeqCst));
                let reset: serde_json::Value = serde_json::from_slice(current).expect("reset must precede actual offer_rebuild");
                assert_eq!(reset["type"], "reset");
                assert_eq!(reset["raft_index"], self.target.to_string());
                assert_eq!(reset["records"].as_array().unwrap().len(), records);
            }
        }
        // Query and release a real control reply before the cut. No ACK has
        // been forwarded; neither a status mirror nor output order proves this.
        let deadline = Instant::now() + Duration::from_secs(4);
        let mut tag = 0;
        let mut reply = 0;
        assert_eq!(unsafe { self.original.offer_status.unwrap()(self.original.context, &mut tag, &mut reply) }, SvStatus::SUCCESS);
        assert_eq!(tag, 0, "native status offer must be queued");
        loop {
            let mut out = SvControlReply::default();
            assert_eq!(unsafe { self.original.poll_reply.unwrap()(self.original.context, reply, &mut out) }, SvStatus::SUCCESS);
            match out.decode().unwrap() {
                ControlReply::Pending => { assert!(Instant::now() < deadline); std::thread::yield_now(); }
                ControlReply::Status(ProjectorStatus::Rebuilding { target_raft_index }) => {
                    assert_eq!(target_raft_index, self.target);
                    break;
                }
                other => panic!("baseline must still be unaccepted: {other:?}"),
            }
        }
        assert_eq!(unsafe { self.original.release_reply.unwrap()(self.original.context, &mut reply) }, SvStatus::SUCCESS);
        assert_eq!(reply, 0);
        let mut channel = self.channel.lock().unwrap();
        writeln!(channel, "{}", serde_json::json!({"cut":label,"target":self.target,"records":records,
            "eof":self.eof.load(Ordering::SeqCst),"finished":self.finished.load(Ordering::SeqCst),"native":"Rebuilding"})).unwrap();
        let mut command = [0];
        channel.read_exact(&mut command).expect("harness must release or kill the held real callback");
        assert_eq!(command, [b'G']);
    }
}
fn with_proxy(raw: *const c_void, f: impl FnOnce(&Proxy) -> SvStatus) -> SvStatus {
    caught(|| f(unsafe { raw.cast::<Proxy>().as_ref().expect("live callback context") }))
}
macro_rules! forward {
    ($name:ident($($arg:ident:$ty:ty),*)) => {
        unsafe extern "C" fn $name(raw:*mut c_void,$($arg:$ty),*)->SvStatus {
            with_proxy(raw,|p|unsafe { p.original.$name.unwrap()(p.original.context,$($arg),*) })
        }
    }
}
forward!(receive(nanos:u64,tag:*mut u32,frame:*mut u64));
forward!(stop_receive());
forward!(frame_info(frame:u64,out:*mut SvFrameInfo));
forward!(transaction(frame:u64,index:u64,out:*mut SvTransactionInfo));
forward!(event(frame:u64,tx:u64,index:u64,out:*mut SvEvent));
forward!(change(frame:u64,tx:u64,index:u64,out:*mut SvRecordChange));
forward!(release_frame(frame:*mut u64));
forward!(report_durable_offset(value:*const SvDurableOffsetReport,tag:*mut u32,reply:*mut u64));
forward!(offer_status(tag:*mut u32,reply:*mut u64));
forward!(poll_reply(reply:u64,out:*mut SvControlReply));
forward!(release_reply(reply:*mut u64));
forward!(notify(kind:u32,failure:*const SvProjectorFailureObservation));
forward!(state_info(state:u64,out:*mut SvStateInfo));
forward!(release_state(state:*mut u64));
unsafe extern "C" fn stopping(raw:*const c_void,out:*mut u32)->SvStatus {
    with_proxy(raw, |p| unsafe { p.original.stopping.unwrap()(p.original.context, out) })
}
unsafe extern "C" fn state_next(raw:*mut c_void,state:u64,tag:*mut u32,out:*mut SvStateRecord)->SvStatus {
    with_proxy(raw, |p| {
        let status = unsafe { p.original.state_next.unwrap()(p.original.context, state, tag, out) };
        if status.is_success() {
            match unsafe { *tag } {
                0 => { p.records.fetch_add(1, Ordering::SeqCst); if p.cut == Cut::BeforeInstall { p.pause("before-install"); } }
                1 => p.eof.store(true, Ordering::SeqCst),
                _ => (),
            }
        }
        status
    })
}
unsafe extern "C" fn finish_state(raw:*mut c_void,state:*mut u64,cut:*mut SvRebuildCut)->SvStatus {
    with_proxy(raw, |p| {
        let status = unsafe { p.original.finish_state.unwrap()(p.original.context, state, cut) };
        if status.is_success() { p.finished.store(true, Ordering::SeqCst); }
        status
    })
}
unsafe extern "C" fn offer_rebuild(raw:*mut c_void,cut:*const SvRebuildCut,tag:*mut u32,reply:*mut u64)->SvStatus {
    with_proxy(raw, |p| {
        if p.cut == Cut::BeforeOffer { p.pause("reset-before-offer"); }
        unsafe { p.original.offer_rebuild.unwrap()(p.original.context, cut, tag, reply) }
    })
}
impl Proxy {
    fn table(&self) -> SvProjectorHostV2 {
        let mut table = self.original;
        table.context = (self as *const Self).cast_mut().cast();
        table.receive = Some(receive); table.stop_receive = Some(stop_receive);
        table.frame_info = Some(frame_info); table.transaction = Some(transaction);
        table.event = Some(event); table.change = Some(change); table.release_frame = Some(release_frame);
        table.report_durable_offset = Some(report_durable_offset); table.offer_status = Some(offer_status);
        table.poll_reply = Some(poll_reply); table.release_reply = Some(release_reply);
        table.stopping = Some(stopping); table.notify = Some(notify);
        table.state_info = Some(state_info); table.state_next = Some(state_next);
        table.finish_state = Some(finish_state); table.release_state = Some(release_state);
        table.offer_rebuild = Some(offer_rebuild);
        table
    }
}
struct Worker { inner:*mut c_void, _proxy:Box<Proxy>, table:Box<SvProjectorHostV2> }
unsafe extern "C" fn start(prepared:*mut *mut c_void,host:*const SvProjectorHostV2,input:*const SvProjectorStart,out:*mut *mut c_void)->SvStatus {
    if prepared.is_null() || host.is_null() || input.is_null() || out.is_null() {
        return unsafe { base().start.unwrap()(prepared,host,input,out) };
    }
    caught(|| {
        unsafe { *out = ptr::null_mut(); }
        let settings = SETTINGS.get().unwrap();
        let mut channel = UnixStream::connect(&settings.socket).unwrap();
        channel.set_read_timeout(Some(Duration::from_secs(8))).unwrap();
        channel.set_write_timeout(Some(Duration::from_secs(8))).unwrap();
        let mut command = [0];
        channel.read_exact(&mut command).unwrap();
        let cut = match command[0] { b'S' => Cut::BeforeInstall, b'A' => Cut::BeforeOffer, _ => panic!("unknown physical cut") };
        let original = unsafe { *prepared };
        let proxy = Box::new(Proxy { original:unsafe { *host },channel:Mutex::new(channel),cut,
            target:unsafe { (*input).rebuild_target },output_start:std::fs::metadata(&settings.report).unwrap().len().try_into().unwrap(),
            paused:AtomicBool::new(false),records:AtomicUsize::new(0),eof:AtomicBool::new(false),finished:AtomicBool::new(false) });
        let table = Box::new(proxy.table());
        let mut worker = ManuallyDrop::new(Box::new(Worker { inner:ptr::null_mut(),_proxy:proxy,table }));
        let table = &*worker.table as *const SvProjectorHostV2;
        let status = unsafe { base().start.unwrap()(prepared,table,input,&mut worker.inner) };
        if !worker.inner.is_null() { unsafe { *out = Box::into_raw(ManuallyDrop::into_inner(worker)).cast(); } }
        else if !status.is_success() && unsafe { *prepared == original } { drop(ManuallyDrop::into_inner(worker)); }
        status
    })
}
unsafe extern "C" fn poll(raw:*const c_void,ready:*mut u32,observation:*mut SvProjectorFailureObservation)->SvStatus {
    let worker = unsafe { &*raw.cast::<Worker>() };
    unsafe { base().poll_worker.unwrap()(worker.inner,ready,observation) }
}
unsafe extern "C" fn stop(raw:*mut c_void)->SvStatus {
    let worker = unsafe { &*raw.cast::<Worker>() };
    unsafe { base().request_stop.unwrap()(worker.inner) }
}
unsafe extern "C" fn join(raw:*mut *mut c_void,out:*mut SvProjectorCompletion)->SvStatus {
    caught(|| {
        let mut worker = ManuallyDrop::new(unsafe { Box::from_raw((*raw).cast::<Worker>()) });
        let status = unsafe { base().join.unwrap()(&mut worker.inner,out) };
        if worker.inner.is_null() { unsafe { *raw = ptr::null_mut(); } }
        if status.is_success() && worker.inner.is_null() && unsafe { (*out).decode(None) }.is_ok_and(|c| c.joined) {
            drop(ManuallyDrop::into_inner(worker));
        }
        status
    })
}
struct StaticTable(SvProjectorV2);
// SAFETY: immutable generated function pointers and process-static target;
// worker-owned contexts remain in the separate retained Proxy allocation.
unsafe impl Send for StaticTable {}
unsafe impl Sync for StaticTable {}
static TABLE: OnceLock<StaticTable> = OnceLock::new();
#[unsafe(no_mangle)]
pub unsafe extern "C" fn statevec_projector_plugin_entry_v2(size:u64,out:*mut SvDiscovery)->u32 {
    if out.is_null() || size < size_of::<SvDiscovery>() as u64 { return 1; }
    let table = TABLE.get_or_init(|| {
        let mut table = base();
        table.start = Some(start); table.poll_worker = Some(poll); table.request_stop = Some(stop); table.join = Some(join);
        StaticTable(table)
    });
    unsafe { out.write(SvDiscovery { table:(&table.0 as *const SvProjectorV2).cast(),table_bytes:size_of::<SvProjectorV2>() as u64 }); }
    0
}
