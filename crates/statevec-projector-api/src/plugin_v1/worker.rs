//! One physical SDK worker. Spawn refusal retains the caller's identical slot;
//! accepted spawn is consumed only by real join, never by a readiness hint.
use super::{caught, pointer, PreparedHolding};
use crate::plugin_v1::{ffi::*, io::ScopeData, PreparedProjector, ProjectorAbiFailure as A, ProjectorFailure as F, ProjectorCallFailure, ProjectorIoStage, ProjectorIoKind};
use std::{ffi::c_void, sync::Arc, thread::JoinHandle};

struct Worker { scope: Arc<ScopeData>, join: Option<JoinHandle<SvProjectorCompletion>> }

pub(super) unsafe extern "C" fn start<P: PreparedProjector>(prepared: *mut *mut c_void, host: *const SvProjectorHostV1, context: *const SvProjectorStart, out: *mut *mut c_void) -> SvStatus {
    caught(|| {
        if let Err(cause) = pointer(out) { return (&cause).into(); }
        unsafe { out.write(std::ptr::null_mut()); }
        if let Err(cause) = pointer(prepared).and_then(|()| pointer(host)).and_then(|()| pointer(context)) { return (&cause).into(); }
        let raw = unsafe { prepared.read() }.cast::<PreparedHolding<P>>();
        if let Err(cause) = pointer(raw) { return (&cause).into(); }
        // Inspect the header before borrowing the complete table: a shorter
        // foreign table never licenses a read of its absent function slots.
        let header = unsafe { host.cast::<SvHeader>().read() };
        if header.abi_major != 1 { return (&A::Version { expected: 1, actual: header.abi_major }).into(); }
        if header.kind != PROJECTOR_HOST_KIND { return (&A::Kind { expected: PROJECTOR_HOST_KIND, actual: header.kind }).into(); }
        let expected = std::mem::size_of::<SvProjectorHostV1>() as u64;
        if header.table_bytes != expected { return (&A::TableSize { expected, actual: header.table_bytes }).into(); }
        // SAFETY: exact call-borrowed inputs and known live SDK Prepared handle.
        let holding = unsafe { &*raw };
        let context = unsafe { &*context };
        let binding = holding.binding;
        if binding.identity != context.identity || binding.peer_id != context.peer_id
            || binding.projector_id != context.projector_id || binding.mode != context.recovery_mode {
            return (&A::InvalidHandle).into();
        }
        let scope = match ScopeData::new(unsafe { &*host }, context) { Ok(value) => Arc::new(value), Err(cause) => return (&cause).into() };
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
                unsafe { prepared.write(std::ptr::null_mut()); }
                drop(unsafe { Box::from_raw(raw) });
                unsafe { out.write(Box::into_raw(worker).cast()); }
                SvStatus::SUCCESS
            }
        }
    })
}

pub(super) unsafe extern "C" fn request_stop(raw: *mut c_void) -> SvStatus {
    caught(|| {
        if let Err(cause) = pointer(raw.cast::<Worker>()) { return (&cause).into(); }
        let worker = unsafe { &*raw.cast::<Worker>() };
        // Host callback failures are terminal worker observations, not legal
        // Projector RequestStop invocation returns. Actual join retains them.
        if let Err(cause) = worker.scope.stop() { worker.scope.record(1, cause); }
        SvStatus::SUCCESS
    })
}
pub(super) unsafe extern "C" fn poll(raw: *const c_void, ready: *mut u32, failure: *mut SvProjectorFailureObservation) -> SvStatus {
    // No author call, lock, allocation, I/O, waiting or retry in this observer.
    if let Err(cause) = pointer(ready).and_then(|()| pointer(failure)) { return (&cause).into(); }
    unsafe { ready.write(0); failure.write(SvProjectorFailureObservation::NONE); }
    if let Err(cause) = pointer(raw.cast::<Worker>()) { return (&cause).into(); }
    let worker = unsafe { &*raw.cast::<Worker>() };
    let (finished, observed) = worker.scope.poll();
    unsafe { ready.write(u32::from(finished)); failure.write(observed); }
    SvStatus::SUCCESS
}
pub(super) unsafe extern "C" fn join(raw: *mut *mut c_void, out: *mut SvProjectorCompletion) -> SvStatus {
    caught(|| {
        if let Err(cause) = pointer(out).and_then(|()| pointer(raw)) { return (&cause).into(); }
        unsafe { out.write(SvProjectorCompletion::EMPTY); }
        let handle = unsafe { raw.read() }.cast::<Worker>();
        if let Err(cause) = pointer(handle) { return (&cause).into(); }
        unsafe { raw.write(std::ptr::null_mut()); }
        let mut worker = unsafe { Box::from_raw(handle) };
        let result = worker.join.take().expect("accepted worker retains its join").join();
        let completion = match result {
            // request_stop may have published a callback failure after the
            // body returned. Join samples the canonical retained result only
            // after the sole stop caller and physical worker have both ended.
            Ok(_) => worker.scope.terminal(),
            Err(payload) => {
                std::mem::forget(payload);
                let mut completion = worker.scope.terminal();
                completion.worker_join_failure = (&ProjectorCallFailure::WorkerJoinPanicked).into();
                completion
            }
        };
        if completion.joined == 0 {
            // Preserve the exact outstanding-loan bookkeeping on uncertain
            // retirement. The host likewise retains its callback dependencies.
            std::mem::forget(worker.scope.clone());
        }
        unsafe { out.write(completion); }
        SvStatus::SUCCESS
    })
}
