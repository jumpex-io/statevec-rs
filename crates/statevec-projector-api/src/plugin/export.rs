//! Generated module boundary. No user-written C handoff or destructor is needed.
use super::borrow::pointer;
use super::ffi::*;
use super::author::*;
use super::{ProjectorAbiError as A, ProjectorError as F, ProjectorOperation as Op, inputs};
use std::ffi::c_void;
use std::mem::size_of;
use std::sync::{Arc, Mutex};

#[path = "worker.rs"]
mod worker;

#[cfg(all(target_arch = "x86_64", target_os = "linux", target_env = "gnu"))]
const TARGET: &str = "x86_64-unknown-linux-gnu";
#[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
const TARGET: &str = "aarch64-unknown-linux-gnu";
#[cfg(not(all(any(target_arch = "x86_64", target_arch = "aarch64"), target_os = "linux", target_env = "gnu")))]
const TARGET: &str = "unsupported";

/// Macro support; not a wrapper for arbitrary foreign tables.
pub struct ProjectorExportTable(SvProjector);
// SAFETY: only immutable static target bytes and monomorphized SDK functions.
unsafe impl Sync for ProjectorExportTable {}
impl ProjectorExportTable {
    /// # Safety
    /// `configure` must forward its arguments to `__export::configure` with
    /// this same `M`, or never report success. The table's `prepare` and
    /// `destroy_configured` read every handle it returns as `M`'s configured
    /// value.
    pub const unsafe fn new<M: ProjectorModule>(configure: unsafe extern "C" fn(SvBytes, SvBytes, *mut *mut c_void) -> SvStatus) -> Self {
        Self(SvProjector {
            h: SvHeader {
                abi_major: PROJECTOR_ABI_MAJOR,
                kind: PROJECTOR_KIND,
                table_bytes: size_of::<SvProjector>() as u64,
                provided_caps: PROJECTOR_BASE_CAPABILITIES
                    | inputs::support(M::SUPPORTED_HISTORY_MODES)
                    | match M::SCHEMA_COMPATIBILITY {
                        SchemaCompatibility::Exact => 0,
                        SchemaCompatibility::Additive => ADDITIVE_CAPABILITY,
                    },
                required_caps: PROJECTOR_BASE_CAPABILITIES,
            },
            build_target: SvBytes { ptr: TARGET.as_ptr(), len: TARGET.len() as u64 },
            configure: Some(configure), prepare: Some(prepare::<M>), start: Some(worker::start::<<M::Configured as ConfiguredProjector>::Prepared>),
            request_stop: Some(worker::request_stop), poll_worker: Some(worker::poll), join: Some(worker::join),
            destroy_prepared: Some(destroy::<PreparedHolding<<M::Configured as ConfiguredProjector>::Prepared>>),
            destroy_configured: Some(destroy::<Configured<M::Configured>>),
            decoder_idl: Some(decoder_idl::<M>),
        })
    }
    /// # Safety
    /// Caller lends a disjoint writable discovery output of `out_bytes` bytes.
    pub unsafe fn discover(&'static self, out_bytes: u64, out: *mut SvDiscovery) -> u32 {
        if out_bytes != size_of::<SvDiscovery>() as u64 || pointer(out).is_err() { return 1; }
        // SAFETY: extent and `pointer(out)` checked above; the caller lends `out` writable and unaliased, and
        // the table address written is `'static`.
        unsafe { out.write(SvDiscovery { table: (&self.0 as *const SvProjector).cast(), table_bytes: size_of::<SvProjector>() as u64 }); }
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

unsafe extern "C" fn decoder_idl<M: ProjectorModule>(out: *mut SvBytes) -> SvStatus {
    caught(|| {
        if let Err(cause) = pointer(out) { return (&cause).into(); }
        // SAFETY: `out` passed `pointer` above and the host lends it for this call.
        unsafe { out.write(SvBytes::EMPTY); }
        let idl = M::decoder_idl();
        if idl.len() > inputs::MAX_IDL_BYTES as usize {
            return (&A::Length { actual: idl.len() as u64, maximum: inputs::MAX_IDL_BYTES }).into();
        }
        // SAFETY: `out` passed `pointer` above and the host lends it for this call; `idl` is `'static`.
        unsafe { out.write(SvBytes::borrowed(idl)); }
        SvStatus::SUCCESS
    })
}

/// # Safety
/// The caller supplies bounded readable spans and a disjoint writable output.
pub unsafe fn configure<M: ProjectorModule>(module: &M, config: SvBytes, base_dir: SvBytes, out: *mut *mut c_void) -> SvStatus {
    caught(|| {
        if let Err(cause) = pointer(out) { return (&cause).into(); }
        // SAFETY: `out` passed `pointer` above and the caller lends it for this call.
        unsafe { out.write(std::ptr::null_mut()); }
        // SAFETY: the caller lends `config` for this call; `ProjectorConfig` borrows it and the `'static`
        // configured value cannot keep it.
        let config = match unsafe { inputs::text(config, inputs::MAX_CONFIG_BYTES) } { Ok(value) => value, Err(cause) => return (&cause).into() };
        // SAFETY: as for `config`: lent for this call and not kept by the `'static` configured value.
        let base = match unsafe { inputs::base_dir(base_dir) } { Ok(value) => value, Err(cause) => return (&cause).into() };
        let value = match module.configure(ProjectorConfig::new(config.as_bytes(), base)) {
            Ok(value) => value, Err(cause) => return (&cause).into(),
        };
        let holding = Box::new(Configured { value, _allocation: 0 });
        // SAFETY: `out` passed `pointer` above; the boxed handle (non-zero-sized, so a unique address)
        // passes to the caller once, on success.
        unsafe { out.write(Box::into_raw(holding).cast()); }
        SvStatus::SUCCESS
    })
}

unsafe extern "C" fn prepare<M: ProjectorModule>(raw: *const c_void, context: *const SvProjectorPrepare, out: *mut *mut c_void, recovery: *mut SvRecovery, limits: *mut SvDeliveryLimits) -> SvStatus {
    caught(|| {
        for result in [pointer(out), pointer(recovery), pointer(limits)] {
            if let Err(cause) = result { return (&cause).into(); }
        }
        // SAFETY: all three outputs passed `pointer` above; the host lends them as disjoint locals for this call.
        unsafe { out.write(std::ptr::null_mut()); recovery.write(SvRecovery::default()); limits.write(SvDeliveryLimits::default()); }
        if let Err(cause) = pointer(raw.cast::<Configured<M::Configured>>()).and_then(|()| pointer(context)) { return (&cause).into(); }
        // SAFETY: exact live configured holding of this table, checked by `pointer` above.
        let config = unsafe { &*raw.cast::<Configured<M::Configured>>() };
        // SAFETY: `context` passed `pointer` above; the host lends the prepare input for this call.
        let context = unsafe { &*context };
        let cluster = inputs::cluster_id(context.identity.cluster);
        // SAFETY: the host lends `schema_idl` with `context` for this call; `ProjectorPrepare` borrows it and
        // the `'static` prepared value cannot keep it.
        let input = match unsafe { inputs::prepare(context, &cluster, inputs::support(M::SUPPORTED_HISTORY_MODES)) } {
            Ok(value) => value, Err(cause) => return (&cause).into(),
        };
        let value = match config.value.prepare(input) {
            Ok(value) => value, Err(cause) => return allowed((&cause).into(), Op::Prepare),
        };
        // Catch metadata panic before explicitly dropping this owned Prepared;
        // never unwind its potentially panicking destructor during another unwind.
        let metadata = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let result = SvRecovery::encode(&value.start_request())?;
            let valid_mode = matches!((context.recovery_mode, result.tag),
                (recovery_mode::CONTINUOUS_HISTORY, recovery_tag::HISTORY_FROM_GENESIS | recovery_tag::HISTORY_FROM_OFFSET)
                | (recovery_mode::LATEST_STATE_ONLY, recovery_tag::LATEST_STATE));
            if !valid_mode { return Err(A::BindingMismatch); }
            if result.tag == recovery_tag::HISTORY_FROM_OFFSET && (result.offset.cluster != context.identity.cluster || result.offset.projector_id != context.projector_id) {
                return Err(A::BindingMismatch);
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
        // SAFETY: all three outputs passed `pointer` above; the boxed handle passes to the host once, on success.
        unsafe { recovery.write(rec); limits.write(bound); out.write(Box::into_raw(holding).cast()); }
        SvStatus::SUCCESS
    })
}

unsafe extern "C" fn destroy<T>(out: *mut *mut c_void) -> SvStatus {
    caught(|| {
        if let Err(cause) = pointer(out) { return (&cause).into(); }
        // SAFETY: `out` passed `pointer` above and holds the host's initialized handle.
        let handle = unsafe { out.read() }.cast::<T>();
        if let Err(cause) = pointer(handle) { return (&cause).into(); }
        // Null first: a caught Drop panic never authorizes a second destruction.
        // SAFETY: the same checked slot as the read above.
        unsafe { out.write(std::ptr::null_mut()); }
        // SAFETY: this table's `prepare`, or its `configure` as `new` requires, boxed the handle as `T`; the
        // host destroys each handle once, with the matching destroy, after its last use.
        drop_owned(unsafe { Box::from_raw(handle) })
    })
}

/// Export one unit-struct module. The SDK owns the C lifecycle and scoped borrows.
#[macro_export]
macro_rules! export_projector_plugin {
    ($module:ident) => {
        #[cfg(panic = "abort")]
        compile_error!("StateVec Projector plugins require panic = \"unwind\"");
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn statevec_projector_entry(out_bytes: u64, out: *mut $crate::plugin::ffi::SvDiscovery) -> u32 {
            static MODULE: $module = $module;
            unsafe extern "C" fn configure(config: $crate::plugin::ffi::SvBytes, base: $crate::plugin::ffi::SvBytes, out: *mut *mut ::std::ffi::c_void) -> $crate::plugin::SvStatus {
                // SAFETY: forwarding the host's scoped Projector input/output contract.
                unsafe { $crate::plugin::__export::configure(&MODULE, config, base, out) }
            }
            // SAFETY: `configure` forwards to `__export::configure` for `$module`.
            static TABLE: $crate::plugin::__export::ProjectorExportTable = unsafe { $crate::plugin::__export::ProjectorExportTable::new::<$module>(configure) };
            // SAFETY: caller supplies a valid fixed discovery output.
            unsafe { TABLE.discover(out_bytes, out) }
        }
    };
}
