//! Physical faults around the exact public history author. The native receiver,
//! publication/cookies and owner ACK remain real; only queue/reply observations
//! are delayed/lost. No configurable production fault switch.
#![cfg(target_os = "linux")]
#[path = "../../examples/projector_v1_history/author.rs"]
mod author;
mod history_crash;
use statevec_projector_api::plugin_v1::{__export, ControlReply, ProjectorFailure, ffi::*};
use std::{
    ffi::c_void,
    mem::{ManuallyDrop, size_of},
    ptr,
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
};

unsafe extern "C" fn configure(config: SvBytes, base: SvBytes, out: *mut *mut c_void) -> SvStatus {
    caught(|| {
        let result = unsafe { __export::configure(&author::History, config, base, out) };
        if result.is_success() {
            // The real configure already validated the borrowed UTF-8 span.
            let bytes = unsafe { std::slice::from_raw_parts(base.ptr, base.len as usize) };
            history_crash::configure(std::path::Path::new(std::str::from_utf8(bytes).unwrap()));
        }
        result
    })
}
static BASE: __export::ProjectorExportTable =
    __export::ProjectorExportTable::new::<author::History>(configure);
fn base() -> SvProjectorV1 {
    let mut out = SvDiscovery {
        table: ptr::null(),
        table_bytes: 0,
    };
    assert_eq!(
        unsafe { BASE.discover(size_of::<SvDiscovery>() as u64, &mut out) },
        0
    );
    unsafe { *out.table.cast::<SvProjectorV1>() }
}
fn caught(f: impl FnOnce() -> SvStatus) -> SvStatus {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(value) => value,
        Err(payload) => {
            std::mem::forget(payload);
            (&ProjectorFailure::PluginPanicked).into()
        }
    }
}
struct Proxy {
    original: SvProjectorHostV1,
    crash: Option<history_crash::Cut>,
    first: Mutex<Option<SvOffsetCheckpoint>>,
    attempts: AtomicUsize,
    reply: AtomicU64,
    lost: AtomicBool,
    completed: AtomicBool,
}
fn with_proxy(raw: *const c_void, f: impl FnOnce(&Proxy) -> SvStatus) -> SvStatus {
    if raw.is_null() {
        return (&ProjectorFailure::HostPanicked).into();
    }
    caught(|| f(unsafe { &*raw.cast::<Proxy>() }))
}
macro_rules! forward {
    ($name:ident($($arg:ident:$ty:ty),*))=>{
        unsafe extern "C" fn $name(raw:*mut c_void,$($arg:$ty),*)->SvStatus {
            with_proxy(raw,|p|unsafe { (p.original.$name.unwrap())(p.original.context,$($arg),*) })
        }
    }
}
forward!(receive(nanos:u64,tag:*mut u32,frame:*mut u64));
forward!(stop_receive());
forward!(transaction(frame:u64,index:u64,out:*mut SvTransactionInfo));
forward!(event(frame:u64,tx:u64,index:u64,out:*mut SvEvent));
forward!(change(frame:u64,tx:u64,index:u64,out:*mut SvRecordChange));
forward!(release_frame(frame:*mut u64));
forward!(offer_status(tag:*mut u32,reply:*mut u64));
forward!(release_reply(reply:*mut u64));
forward!(notify(kind:u32,failure:*const SvProjectorFailureObservation));
forward!(state_info(state:u64,out:*mut SvStateInfo));
forward!(state_next(state:u64,tag:*mut u32,out:*mut SvStateRecord));
forward!(finish_state(state:*mut u64,cut:*mut SvRebuildCut));
forward!(release_state(state:*mut u64));
forward!(offer_rebuild(cut:*const SvRebuildCut,tag:*mut u32,reply:*mut u64));
unsafe extern "C" fn stopping(raw: *const c_void, out: *mut u32) -> SvStatus {
    with_proxy(raw, |p| unsafe {
        p.original.stopping.unwrap()(p.original.context, out)
    })
}
unsafe extern "C" fn frame_info(raw: *mut c_void, frame: u64, out: *mut SvFrameInfo) -> SvStatus {
    with_proxy(raw, |p| {
        let result = unsafe { p.original.frame_info.unwrap()(p.original.context, frame, out) };
        if result.is_success() {
            if let Some(cut) = &p.crash {
                cut.frame(&p.original, unsafe { *out });
            }
        }
        result
    })
}
unsafe extern "C" fn offer_checkpoint(
    raw: *mut c_void,
    value: *const SvOffsetCheckpoint,
    tag: *mut u32,
    reply: *mut u64,
) -> SvStatus {
    with_proxy(raw, |p| {
        if value.is_null() || tag.is_null() || reply.is_null() {
            return (&ProjectorFailure::HostPanicked).into();
        }
        if let Some(cut) = &p.crash {
            cut.before_offer(&p.original, unsafe { *value });
            let status = unsafe { p.original.offer_checkpoint.unwrap()(p.original.context, value, tag, reply) };
            if status.is_success() && unsafe { *tag == 0 } {
                p.reply.store(unsafe { *reply }, Ordering::Release);
            }
            return status;
        }
        if !p.completed.load(Ordering::Acquire) {
            let value = unsafe { *value };
            let exact = {
                let mut first = p.first.lock().unwrap_or_else(|e| e.into_inner());
                *first.get_or_insert(value) == value
            };
            let attempt = p.attempts.fetch_add(1, Ordering::Relaxed) + 1;
            eprintln!("history-probe report attempt={attempt} exact={exact}");
            if !exact {
                return (&ProjectorFailure::HostPanicked).into();
            }
            if attempt <= 2 {
                unsafe {
                    tag.write(1);
                    reply.write(0);
                }
                return SvStatus::SUCCESS;
            }
        }
        let status =
            unsafe { p.original.offer_checkpoint.unwrap()(p.original.context, value, tag, reply) };
        if !p.completed.load(Ordering::Acquire) && status.is_success() && unsafe { *tag == 0 } {
            p.reply.store(unsafe { *reply }, Ordering::Release);
        }
        status
    })
}
unsafe extern "C" fn poll_reply(
    raw: *mut c_void,
    reply: u64,
    out: *mut SvControlReply,
) -> SvStatus {
    with_proxy(raw, |p| {
        let status = unsafe { p.original.poll_reply.unwrap()(p.original.context, reply, out) };
        if status.is_success()
            && !out.is_null()
            && p.reply.load(Ordering::Acquire) == reply
            && matches!(unsafe { (*out).decode() }, Ok(ControlReply::Accepted))
        {
            if let Some(cut) = &p.crash {
                if cut.lose_accepted() {
                    unsafe { out.write(SvControlReply { tag: 4, ..SvControlReply::default() }); }
                }
                return status;
            }
            if !p.lost.swap(true, Ordering::AcqRel) {
                unsafe {
                    out.write(SvControlReply {
                        tag: 4,
                        cause: 0,
                        value0: 0,
                        value1: 0,
                    });
                }
                eprintln!("history-probe accepted reply lost");
            } else {
                p.completed.store(true, Ordering::Release);
                eprintln!(
                    "history-probe exact retry accepted attempts={}",
                    p.attempts.load(Ordering::Relaxed)
                );
            }
        }
        status
    })
}
impl Proxy {
    fn table(&self) -> SvProjectorHostV1 {
        let mut table = self.original;
        table.context = (self as *const Self).cast_mut().cast();
        table.receive = Some(receive);
        table.stop_receive = Some(stop_receive);
        table.frame_info = Some(frame_info);
        table.transaction = Some(transaction);
        table.event = Some(event);
        table.change = Some(change);
        table.release_frame = Some(release_frame);
        table.offer_checkpoint = Some(offer_checkpoint);
        table.offer_status = Some(offer_status);
        table.poll_reply = Some(poll_reply);
        table.release_reply = Some(release_reply);
        table.stopping = Some(stopping);
        table.notify = Some(notify);
        table.state_info = Some(state_info);
        table.state_next = Some(state_next);
        table.finish_state = Some(finish_state);
        table.release_state = Some(release_state);
        table.offer_rebuild = Some(offer_rebuild);
        table
    }
}
struct Worker {
    inner: *mut c_void,
    _proxy: Box<Proxy>,
    table: Box<SvProjectorHostV1>,
}
unsafe extern "C" fn start(
    prepared: *mut *mut c_void,
    host: *const SvProjectorHostV1,
    input: *const SvProjectorStart,
    out: *mut *mut c_void,
) -> SvStatus {
    if prepared.is_null() || host.is_null() || input.is_null() || out.is_null() {
        return unsafe { base().start.unwrap()(prepared, host, input, out) };
    }
    caught(|| {
        unsafe {
            *out = ptr::null_mut();
        }
        let original = unsafe { *prepared };
        let proxy = Box::new(Proxy {
            original: unsafe { *host },
            crash: history_crash::Cut::connect(),
            first: Mutex::new(None),
            attempts: AtomicUsize::new(0),
            reply: AtomicU64::new(0),
            lost: AtomicBool::new(false),
            completed: AtomicBool::new(false),
        });
        let table = Box::new(proxy.table());
        let mut worker = ManuallyDrop::new(Box::new(Worker {
            inner: ptr::null_mut(),
            _proxy: proxy,
            table,
        }));
        let table = &*worker.table as *const SvProjectorHostV1;
        let status = unsafe { base().start.unwrap()(prepared, table, input, &mut worker.inner) };
        if !worker.inner.is_null() {
            unsafe {
                *out = Box::into_raw(ManuallyDrop::into_inner(worker)).cast();
            }
        } else if !status.is_success() && unsafe { *prepared == original } {
            drop(ManuallyDrop::into_inner(worker));
        }
        status
    })
}
unsafe extern "C" fn poll(
    raw: *const c_void,
    ready: *mut u32,
    observation: *mut SvProjectorFailureObservation,
) -> SvStatus {
    let worker = unsafe { &*raw.cast::<Worker>() };
    unsafe { base().poll_worker.unwrap()(worker.inner, ready, observation) }
}
unsafe extern "C" fn stop(raw: *mut c_void) -> SvStatus {
    let worker = unsafe { &*raw.cast::<Worker>() };
    unsafe { base().request_stop.unwrap()(worker.inner) }
}
unsafe extern "C" fn join(raw: *mut *mut c_void, out: *mut SvProjectorCompletion) -> SvStatus {
    caught(|| {
        let mut worker = ManuallyDrop::new(unsafe { Box::from_raw((*raw).cast::<Worker>()) });
        let status = unsafe { base().join.unwrap()(&mut worker.inner, out) };
        if worker.inner.is_null() {
            unsafe {
                *raw = ptr::null_mut();
            }
        }
        if status.is_success()
            && worker.inner.is_null()
            && unsafe { (*out).decode(None) }.is_ok_and(|c| c.joined)
        {
            drop(ManuallyDrop::into_inner(worker));
        }
        status
    })
}
struct StaticTable(SvProjectorV1);
// SAFETY: immutable generated function pointers and process-static target bytes;
// this table contains no context, configured handle or mutable author resource.
unsafe impl Send for StaticTable {}
unsafe impl Sync for StaticTable {}
static TABLE: std::sync::OnceLock<StaticTable> = std::sync::OnceLock::new();
#[unsafe(no_mangle)]
pub unsafe extern "C" fn statevec_projector_plugin_entry_v1(size: u64, out: *mut SvDiscovery) -> u32 {
    if out.is_null() || size < size_of::<SvDiscovery>() as u64 {
        return 1;
    }
    let table = TABLE.get_or_init(|| {
        let mut t = base();
        t.start = Some(start);
        t.poll_worker = Some(poll);
        t.request_stop = Some(stop);
        t.join = Some(join);
        StaticTable(t)
    });
    unsafe {
        out.write(SvDiscovery {
            table: (&table.0 as *const SvProjectorV1).cast(),
            table_bytes: size_of::<SvProjectorV1>() as u64,
        });
    }
    0
}
