//! Generated module boundary. No user-written C handoff or destructor is needed.
use super::{author::*, borrow::pointer, ffi::*, inputs, ProjectorAbiFailure as A, ProjectorFailure as F, ProjectorOperation as Op};
use std::{ffi::c_void, mem::size_of, sync::{Arc, Mutex}};

#[path = "worker.rs"]
mod worker;

#[cfg(all(target_arch = "x86_64", target_os = "linux", target_env = "gnu"))]
const TARGET: &str = "x86_64-unknown-linux-gnu";
#[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
const TARGET: &str = "aarch64-unknown-linux-gnu";
#[cfg(not(all(any(target_arch = "x86_64", target_arch = "aarch64"), target_os = "linux", target_env = "gnu")))]
const TARGET: &str = "unsupported";

/// Macro support; not a wrapper for arbitrary foreign tables.
pub struct ProjectorExportTable(SvProjectorV1);
// SAFETY: only immutable static target bytes and monomorphized SDK functions.
unsafe impl Sync for ProjectorExportTable {}
impl ProjectorExportTable {
    pub const fn new<M: ProjectorModule>(configure: unsafe extern "C" fn(SvBytes, SvBytes, *mut *mut c_void) -> SvStatus) -> Self {
        Self(SvProjectorV1 {
            h: SvHeader { abi_major: 1, kind: PROJECTOR_KIND, table_bytes: size_of::<SvProjectorV1>() as u64,
                provided_caps: PROJECTOR_BASE_CAPABILITIES | inputs::support(M::RECOVERY_SUPPORT), required_caps: PROJECTOR_BASE_CAPABILITIES },
            build_target: SvBytes { ptr: TARGET.as_ptr(), len: TARGET.len() as u64 },
            configure: Some(configure), prepare: Some(prepare::<M>), start: Some(worker::start::<<M::Configured as ConfiguredProjector>::Prepared>),
            request_stop: Some(worker::request_stop), poll_worker: Some(worker::poll), join: Some(worker::join),
            destroy_prepared: Some(destroy::<PreparedHolding<<M::Configured as ConfiguredProjector>::Prepared>>),
            destroy_configured: Some(destroy::<Configured<M::Configured>>),
        })
    }
    /// # Safety
    /// Caller lends a disjoint writable discovery output of `out_bytes` bytes.
    pub unsafe fn discover(&'static self, out_bytes: u64, out: *mut SvDiscovery) -> u32 {
        if out_bytes != size_of::<SvDiscovery>() as u64 || pointer(out).is_err() { return 1; }
        unsafe { out.write(SvDiscovery { table: (&self.0 as *const SvProjectorV1).cast(), table_bytes: size_of::<SvProjectorV1>() as u64 }); }
        0
    }
}

struct Configured<C> { value: C, _allocation: u8 }
struct PreparedHolding<P> { pending: Arc<Mutex<Option<P>>>, binding: Binding }
#[derive(Clone, Copy)]
struct Binding { identity: SvProjectionIdentity, peer_id: u64, projector_id: SvProjectorId, mode: u32 }

fn caught(f: impl FnOnce() -> SvStatus) -> SvStatus {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(value) => value,
        Err(payload) => { std::mem::forget(payload); (&F::PluginPanicked).into() }
    }
}
fn allowed(status: SvStatus, operation: Op) -> SvStatus {
    match super::decode_call(status, operation) {
        Ok(_) => status, Err(cause) => (&cause).into(),
    }
}
fn drop_owned<T>(value: T) -> SvStatus { caught(|| { drop(value); SvStatus::SUCCESS }) }

/// # Safety
/// The caller supplies bounded readable spans and a disjoint writable output.
pub unsafe fn configure<M: ProjectorModule>(module: &M, config: SvBytes, base_dir: SvBytes, out: *mut *mut c_void) -> SvStatus {
    caught(|| {
        if let Err(cause) = pointer(out) { return (&cause).into(); }
        unsafe { out.write(std::ptr::null_mut()); }
        let config = match unsafe { inputs::text(config, inputs::MAX_CONFIG_BYTES) } { Ok(value) => value, Err(cause) => return (&cause).into() };
        let base = match unsafe { inputs::base_dir(base_dir) } { Ok(value) => value, Err(cause) => return (&cause).into() };
        let value = match module.configure(ProjectorConfig { bytes: config.as_bytes(), base_dir: base }) {
            Ok(value) => value, Err(cause) => return (&cause).into(),
        };
        let holding = Box::new(Configured { value, _allocation: 0 });
        unsafe { out.write(Box::into_raw(holding).cast()); }
        SvStatus::SUCCESS
    })
}

unsafe extern "C" fn prepare<M: ProjectorModule>(raw: *const c_void, context: *const SvProjectorPrepare, out: *mut *mut c_void, recovery: *mut SvRecovery, limits: *mut SvDeliveryLimits) -> SvStatus {
    caught(|| {
        for result in [pointer(out), pointer(recovery), pointer(limits)] {
            if let Err(cause) = result { return (&cause).into(); }
        }
        unsafe { out.write(std::ptr::null_mut()); recovery.write(SvRecovery::default()); limits.write(SvDeliveryLimits::default()); }
        if let Err(cause) = pointer(raw.cast::<Configured<M::Configured>>()).and_then(|()| pointer(context)) { return (&cause).into(); }
        // SAFETY: exact live configured holding and call-borrowed context.
        let config = unsafe { &*raw.cast::<Configured<M::Configured>>() };
        let context = unsafe { &*context };
        let cluster = inputs::cluster_id(context.identity.cluster);
        let input = match unsafe { inputs::prepare(context, &cluster, inputs::support(M::RECOVERY_SUPPORT)) } {
            Ok(value) => value, Err(cause) => return (&cause).into(),
        };
        let value = match config.value.prepare(input) {
            Ok(value) => value, Err(cause) => return allowed((&cause).into(), Op::Prepare),
        };
        // Catch metadata panic before explicitly dropping this owned Prepared;
        // never unwind its potentially panicking destructor during another unwind.
        let metadata = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let result = SvRecovery::encode(&value.recovery())?;
            let valid_mode = matches!((context.recovery_mode, result.tag), (1, 0 | 1) | (2, 2));
            if !valid_mode { return Err(A::InvalidHandle); }
            if result.tag == 1 && (result.offset.cluster != context.identity.cluster || result.offset.projector_id != context.projector_id) {
                return Err(A::InvalidHandle);
            }
            Ok((result, SvDeliveryLimits::from(value.delivery_limits())))
        }));
        let (rec, bound) = match metadata {
            Ok(Ok(value)) => value,
            error => {
                let cause = match error {
                    Ok(Err(cause)) => SvStatus::from(&cause),
                    Err(payload) => { std::mem::forget(payload); (&F::PluginPanicked).into() },
                    Ok(Ok(_)) => unreachable!(),
                };
                let cleanup = drop_owned(value);
                return if cleanup.is_success() { cause } else { cleanup };
            }
        };
        let holding = Box::new(PreparedHolding { pending: Arc::new(Mutex::new(Some(value))), binding: Binding {
            identity: context.identity, peer_id: context.peer_id, projector_id: context.projector_id, mode: context.recovery_mode,
        } });
        unsafe { recovery.write(rec); limits.write(bound); out.write(Box::into_raw(holding).cast()); }
        SvStatus::SUCCESS
    })
}

unsafe extern "C" fn destroy<T>(out: *mut *mut c_void) -> SvStatus {
    caught(|| {
        if let Err(cause) = pointer(out) { return (&cause).into(); }
        let handle = unsafe { out.read() }.cast::<T>();
        if let Err(cause) = pointer(handle) { return (&cause).into(); }
        // Null first: a caught Drop panic never authorizes a second destruction.
        unsafe { out.write(std::ptr::null_mut()); }
        drop_owned(unsafe { Box::from_raw(handle) })
    })
}

/// Export one unit-struct module. The SDK owns the C lifecycle and scoped loans.
#[macro_export]
macro_rules! export_projector_plugin_v1 {
    ($module:ident) => {
        #[cfg(panic = "abort")]
        compile_error!("StateVec Projector plugins require panic = \"unwind\"");
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn statevec_projector_plugin_entry_v1(out_bytes: u64, out: *mut $crate::plugin_v1::ffi::SvDiscovery) -> u32 {
            static MODULE: $module = $module;
            unsafe extern "C" fn configure(config: $crate::plugin_v1::ffi::SvBytes, base: $crate::plugin_v1::ffi::SvBytes, out: *mut *mut ::std::ffi::c_void) -> $crate::plugin_v1::SvStatus {
                unsafe { $crate::plugin_v1::__export::configure(&MODULE, config, base, out) }
            }
            static TABLE: $crate::plugin_v1::__export::ProjectorExportTable = $crate::plugin_v1::__export::ProjectorExportTable::new::<$module>(configure);
            unsafe { TABLE.discover(out_bytes, out) }
        }
    };
}
