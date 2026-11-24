//! One physical SDK worker. Spawn refusal retains the caller's identical slot;
//! accepted spawn is consumed only by real join, never by a readiness hint.
use super::{caught, pointer, PreparedHolding};
use crate::plugin::ffi::*;
use crate::plugin::io::ScopeData;
use crate::plugin::{
    PreparedProjector, ProjectorAbiError as A, ProjectorCallError, ProjectorError as F, ProjectorIoKind,
    ProjectorIoStage,
};
use std::ffi::c_void;
use std::sync::Arc;
use std::thread::JoinHandle;

struct Worker { scope: Arc<ScopeData>, join: Option<JoinHandle<SvProjectorCompletion>> }

pub(super) unsafe extern "C" fn start<P: PreparedProjector>(prepared: *mut *mut c_void, host: *const SvProjectorHost, context: *const SvProjectorStart, out: *mut *mut c_void) -> SvStatus {
    caught(|| {
        if let Err(cause) = pointer(out) { return (&cause).into(); }
        // SAFETY: `out` passed `pointer` above and the host lends it for this call.
        unsafe { out.write(std::ptr::null_mut()); }
        if let Err(cause) = pointer(prepared).and_then(|()| pointer(host)).and_then(|()| pointer(context)) { return (&cause).into(); }
        // SAFETY: `prepared` passed `pointer` above and holds the host's initialized handle.
        let raw = unsafe { prepared.read() }.cast::<PreparedHolding<P>>();
        if let Err(cause) = pointer(raw) { return (&cause).into(); }
        // Inspect the header before borrowing the complete table: a shorter
        // foreign table never licenses a read of its absent function slots.
        // SAFETY: `host` passed `pointer` above, and every host table starts with a readable `SvHeader`.
        let header = unsafe { host.cast::<SvHeader>().read() };
        if header.abi_major != PROJECTOR_ABI_MAJOR { return (&A::Version { expected: PROJECTOR_ABI_MAJOR, actual: header.abi_major }).into(); }
        if header.kind != PROJECTOR_HOST_KIND { return (&A::Kind { expected: PROJECTOR_HOST_KIND, actual: header.kind }).into(); }
        let expected = crate::plugin::borrow::host_table_bytes(&header);
        if header.table_bytes < expected { return (&A::TableSize { expected, actual: header.table_bytes }).into(); }
        // SAFETY: the header declares and the size check admits the query table.
        let query = (header.provided_caps & ENGINE_QUERY_CAPABILITY != 0)
            .then(|| unsafe { &(*host.cast::<SvProjectorHostQuery>()).query });
        // SAFETY: exact call-borrowed inputs and known live SDK Prepared handle.
        let holding = unsafe { &*raw };
        // SAFETY: `context` passed `pointer` above; the host lends the start input for this call.
        let context = unsafe { &*context };
        let binding = holding.binding;
        if binding.identity != context.identity || binding.peer_id != context.peer_id
            || binding.projector_id != context.projector_id || binding.mode != context.recovery_mode {
            return (&A::BindingMismatch).into();
        }
        // SAFETY: `pointer(host)` and the size check above admit a full `SvProjectorHost`, lent for this call;
        // `ScopeData::new` validates it and keeps only a copy.
        let scope = match ScopeData::new(unsafe { &*host }, query, context) { Ok(value) => Arc::new(value), Err(cause) => return (&cause).into() };
        let mut worker = Box::new(Worker { scope: scope.clone(), join: None });
        let pending = holding.pending.clone();
        // Only an accepted thread can take P. Dropping the closure after a
        // physical spawn refusal drops an Arc, not the caller's Prepared.
        let result = std::thread::Builder::new().name("statevec-projector".into()).spawn(move || {
            let value = pending.lock().unwrap_or_else(|e| e.into_inner()).take().expect("one accepted Prepared");
            scope.run(|io| value.run(io))
        });
        match result {
            Err(source) => (&F::Io { stage: ProjectorIoStage::WorkerSpawn, kind: ProjectorIoKind::from_io(source.kind()), os_code: source.raw_os_error() }).into(),
            Ok(join) => {
                worker.join = Some(join);
                // Accepted thread owns P even if it already failed. These
                // infallible stores transfer exactly once before returning Ok.
                // SAFETY: `prepared` passed `pointer` above and the host lends it for this call.
                unsafe { prepared.write(std::ptr::null_mut()); }
                // SAFETY: the host hands its owned prepared handle to `start` once, and this table's `prepare`
                // boxed it as `PreparedHolding<P>`; `holding` is no longer used and the thread keeps its own `Arc`.
                drop(unsafe { Box::from_raw(raw) });
                // SAFETY: `out` passed `pointer` above; the boxed worker passes to the host once, on success.
                unsafe { out.write(Box::into_raw(worker).cast()); }
                SvStatus::SUCCESS
            }
        }
    })
}

pub(super) unsafe extern "C" fn request_stop(raw: *mut c_void) -> SvStatus {
    caught(|| {
        if let Err(cause) = pointer(raw.cast::<Worker>()) { return (&cause).into(); }
        // SAFETY: `raw` is the live `Worker` from `start`; the host never calls this during or after `join`,
        // and access is shared only.
        let worker = unsafe { &*raw.cast::<Worker>() };
        // An explicit caller-owned stop result must survive an earlier run
        // failure. Forward it to the host's independent stop slot; join still
        // consumes the worker and reports the unchanged run/release causes.
        match worker.scope.stop() { Ok(()) => SvStatus::SUCCESS, Err(cause) => (&cause).into() }
    })
}
pub(super) unsafe extern "C" fn poll(raw: *const c_void, ready: *mut u32, failure: *mut SvProjectorFailureObservation) -> SvStatus {
    // No author call, lock, allocation, I/O, waiting or retry in this observer.
    if let Err(cause) = pointer(ready).and_then(|()| pointer(failure)) { return (&cause).into(); }
    // SAFETY: both outputs passed `pointer` above and the host lends them for this call.
    unsafe { ready.write(0); failure.write(SvProjectorFailureObservation::NONE); }
    if let Err(cause) = pointer(raw.cast::<Worker>()) { return (&cause).into(); }
    // SAFETY: `raw` is the live `Worker` from `start`; the host never calls this during or after `join`,
    // and access is shared only.
    let worker = unsafe { &*raw.cast::<Worker>() };
    let (finished, observed) = worker.scope.poll();
    // SAFETY: the same checked outputs as above.
    unsafe { ready.write(u32::from(finished)); failure.write(observed); }
    SvStatus::SUCCESS
}
pub(super) unsafe extern "C" fn join(raw: *mut *mut c_void, out: *mut SvProjectorCompletion) -> SvStatus {
    caught(|| {
        if let Err(cause) = pointer(out).and_then(|()| pointer(raw)) { return (&cause).into(); }
        // SAFETY: `out` passed `pointer` above and the host lends it for this call.
        unsafe { out.write(SvProjectorCompletion::EMPTY); }
        // SAFETY: `raw` passed `pointer` above and holds the host's initialized handle.
        let handle = unsafe { raw.read() }.cast::<Worker>();
        if let Err(cause) = pointer(handle) { return (&cause).into(); }
        // SAFETY: the same checked slot as the read above.
        unsafe { raw.write(std::ptr::null_mut()); }
        // SAFETY: `handle` is the `Worker` that `start` boxed; the host joins it once and never uses it
        // again, and its slot is now null.
        let mut worker = unsafe { Box::from_raw(handle) };
        let result = worker.join.take().expect("accepted worker retains its join").join();
        let completion = match result {
            // Sample the canonical retained result after the physical worker
            // and every scoped service have ended, not a readiness hint.
            Ok(_) => worker.scope.terminal(),
            Err(payload) => {
                std::mem::forget(payload);
                let mut completion = worker.scope.terminal();
                completion.worker_join_failure = (&ProjectorCallError::WorkerJoinPanicked).into();
                completion
            }
        };
        if completion.joined == 0 {
            // Preserve the exact outstanding-borrow bookkeeping on uncertain
            // retirement. The host likewise retains its callback dependencies.
            std::mem::forget(worker.scope.clone());
        }
        // SAFETY: `out` passed `pointer` above and the host lends it for this call.
        unsafe { out.write(completion); }
        SvStatus::SUCCESS
    })
}
