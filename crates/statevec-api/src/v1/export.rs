//! SDK-generated Core V5 entrypoints. Authors implement safe traits only.
use super::{
    CoreAbiError, SvStatus, author::*, borrow::*, candidate::Candidate, ffi::*, tx::PluginTx,
};
use crate::RuntimeCommandRef;
use statevec_model::SchemaIdentity;
use std::ffi::c_void;
use std::mem::size_of;

#[cfg(all(target_arch = "x86_64", target_os = "linux", target_env = "gnu"))]
const TARGET: &str = "x86_64-unknown-linux-gnu";
#[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
const TARGET: &str = "aarch64-unknown-linux-gnu";
#[cfg(not(all(
    any(target_arch = "x86_64", target_arch = "aarch64"),
    target_os = "linux",
    target_env = "gnu"
)))]
const TARGET: &str = "unsupported";

/// Private macro support. Only built from static target bytes and monomorphized
/// SDK functions; this wrapper does not make arbitrary raw tables Sync.
#[doc(hidden)]
pub struct CoreExportTable(SvCoreV5);
// SAFETY: constructor owns an immutable table containing only static target
// bytes/function addresses. It contains no configured or runtime state.
unsafe impl Sync for CoreExportTable {}
impl CoreExportTable {
    pub const fn new<M: CoreModule>(
        configure: unsafe extern "C" fn(SvBytes, *mut *mut c_void) -> SvStatus,
    ) -> Self {
        let rules =
            <<M::Configured as ConfiguredCore>::Runtime as CoreRuntime>::TX_INVARIANTS.is_some();
        Self(SvCoreV5 {
            h: SvHeader {
                abi_major: CORE_ABI_MAJOR,
                kind: CORE_KIND,
                table_bytes: size_of::<SvCoreV5>() as u64,
                provided_caps: CORE_CAPABILITIES,
                required_caps: CORE_BASE_CAPABILITIES
                    | if rules { CORE_CANDIDATE_CAPABILITY } else { 0 },
            },
            build_target: SvBytes {
                ptr: TARGET.as_ptr(),
                len: TARGET.len() as u64,
            },
            configure: Some(configure),
            metadata: Some(metadata::<M::Configured>),
            create_instance: Some(create::<M::Configured>),
            preflight_batch: Some(preflight::<<M::Configured as ConfiguredCore>::Runtime>),
            run_tx: Some(run::<<M::Configured as ConfiguredCore>::Runtime>),
            initialize_core: Some(initialize::<<M::Configured as ConfiguredCore>::Runtime>),
            migrate: Some(migrate::<<M::Configured as ConfiguredCore>::Runtime>),
            validate_tx_invariants: Some(rules_call::<<M::Configured as ConfiguredCore>::Runtime>),
            destroy_instance: Some(destroy::<Instance<<M::Configured as ConfiguredCore>::Runtime>>),
            destroy_configured: Some(destroy::<Configured<M::Configured>>),
        })
    }
    /// # Safety
    /// `out` is writable for its declared extent and not aliased during the call.
    pub unsafe fn discover(&'static self, out_bytes: u64, out: *mut SvDiscovery) -> u32 {
        if out_bytes != size_of::<SvDiscovery>() as u64 || pointer(out).is_err() {
            return 1;
        }
        // SAFETY: caller's initialized-or-uninitialized output, exact extent
        // and alignment checked; all fields are written without reading it.
        unsafe {
            out.write(SvDiscovery {
                table: (&self.0 as *const SvCoreV5).cast(),
                table_bytes: size_of::<SvCoreV5>() as u64,
            })
        };
        0
    }
}

struct Configured<C> {
    value: C,
    name: String,
    revision: String,
    idl: Vec<u8>,
    schema: SchemaIdentity,
}

// Keep zero-sized runtimes in distinct live allocations too. The byte has no
// business meaning; it only prevents Box<ZST>'s shared dangling address.
struct Instance<R> {
    runtime: R,
    _allocation: u8,
}

/// # Safety
/// Caller supplies valid borrowed configuration and one writable handle output;
/// these pointers are disjoint and valid for the complete synchronous call.
#[doc(hidden)]
pub unsafe fn configure<M: CoreModule>(
    module: &M,
    config: SvBytes,
    out: *mut *mut c_void,
) -> SvStatus {
    caught(|| {
        if let Err(source) = pointer(out) {
            return (&source).into();
        }
        // SAFETY: caller's checked output; failure must not transfer a handle.
        unsafe { out.write(std::ptr::null_mut()) };
        // SAFETY: checked bounded borrow, never retained by this function.
        let bytes = match unsafe { slice(config.ptr, config.len, 1024 * 1024) } {
            Ok(value) => value,
            Err(source) => return (&source).into(),
        };
        if std::str::from_utf8(bytes).is_err() {
            return (&CoreAbiError::Utf8).into();
        }
        let configured = match module.configure(CoreConfig { bytes }) {
            Ok(value) => value,
            Err(source) => return (&source).into(),
        };
        let metadata = configured.metadata();
        for (text, maximum) in [(metadata.name, 128), (metadata.revision, 4096)] {
            if text.len() > maximum {
                return (&CoreAbiError::Length {
                    actual: text.len() as u64,
                    maximum: maximum as u64,
                })
                    .into();
            }
            if text.as_bytes().contains(&0) {
                return (&CoreAbiError::EmbeddedNul).into();
            }
        }
        let (name, revision, idl, schema) = (
            metadata.name.to_owned(),
            metadata.revision.to_owned(),
            // Canonical IDL is an identity projection: it strips delivery
            // metadata such as inlineResponse. The host needs the full schema
            // to reproduce native replies as well as canonical execution.
            metadata.schema.to_idl_json().into_bytes(),
            metadata.schema.identity(),
        );
        let maximum = statevec_model::idl::SchemaIdlLimits::default().max_bytes;
        if idl.len() > maximum {
            return (&CoreAbiError::Length {
                actual: idl.len() as u64,
                maximum: maximum as u64,
            })
                .into();
        }
        let holding = Box::new(Configured {
            value: configured,
            name,
            revision,
            idl,
            schema,
        });
        // SAFETY: ownership transfers exactly once, after complete construction.
        unsafe { out.write(Box::into_raw(holding).cast()) };
        SvStatus::SUCCESS
    })
}

unsafe extern "C" fn metadata<C: ConfiguredCore>(
    handle: *const c_void,
    out: *mut SvCoreMetadata,
) -> SvStatus {
    caught(|| {
        if let Err(source) = pointer(out) {
            return (&source).into();
        }
        // SAFETY: full initialized output even on ordinary refusal.
        unsafe { out.write(std::mem::zeroed()) };
        if let Err(source) = pointer(handle.cast::<Configured<C>>()) {
            return (&source).into();
        }
        // SAFETY: host holds the exact configured allocation returned above;
        // immutable metadata remains live until explicit configured destruction.
        let config = unsafe { &*handle.cast::<Configured<C>>() };
        let value = SvCoreMetadata {
            name: SvBytes::borrowed(config.name.as_bytes()),
            revision: SvBytes::borrowed(config.revision.as_bytes()),
            schema_idl: SvBytes::borrowed(&config.idl),
            tx_rules: u32::from(C::Runtime::TX_INVARIANTS.is_some()),
            reserved: 0,
            schema: config.schema.into(),
            reserved_schema: [0; 6],
        };
        // SAFETY: checked output borrows only this still-live configured object.
        unsafe { out.write(value) };
        SvStatus::SUCCESS
    })
}

unsafe extern "C" fn create<C: ConfiguredCore>(
    handle: *const c_void,
    out: *mut *mut c_void,
) -> SvStatus {
    caught(|| {
        if let Err(source) = pointer(out) {
            return (&source).into();
        }
        // SAFETY: checked full output, initialized before user construction.
        unsafe { out.write(std::ptr::null_mut()) };
        if let Err(source) = pointer(handle.cast::<Configured<C>>()) {
            return (&source).into();
        }
        // SAFETY: exact immutable configured holding remains live throughout.
        let configured = unsafe { &*handle.cast::<Configured<C>>() };
        let instance = match configured.value.create_instance() {
            Ok(runtime) => Box::new(Instance {
                runtime,
                _allocation: 0,
            }),
            Err(source) => return (&super::CoreError::Construction(source)).into(),
        };
        // SAFETY: successful construction transfers a unique plugin allocation.
        unsafe { out.write(Box::into_raw(instance).cast()) };
        SvStatus::SUCCESS
    })
}

unsafe fn command<'a>(raw: *const SvCommand) -> Result<RuntimeCommandRef<'a>, CoreAbiError> {
    pointer(raw)?;
    // SAFETY: caller supplies one initialized command, borrowed for the call.
    let raw = unsafe { &*raw };
    let reserved = u64::from(raw.reserved16) | u64::from(raw.reserved32);
    if reserved != 0 {
        return Err(CoreAbiError::Reserved { actual: reserved });
    }
    // SAFETY: payload belongs to this command's call-scoped host input.
    Ok(RuntimeCommandRef::new(
        raw.kind,
        raw.ext_seq,
        raw.ref_ext_time_us,
        unsafe { bytes(raw.payload)? },
    ))
}

unsafe extern "C" fn preflight<R: CoreRuntime>(
    instance: *const c_void,
    commands: *const SvCommand,
    count: u64,
    bad: *mut u64,
) -> SvStatus {
    caught(|| {
        if let Err(source) = pointer(bad) {
            return (&source).into();
        }
        // SAFETY: valid out ordinal, initialized on success and failure.
        unsafe { bad.write(0) };
        if let Err(source) = pointer(instance.cast::<Instance<R>>()) {
            return (&source).into();
        }
        // SAFETY: host supplies an initialized bounded array; check byte extent
        // before iteration. Existing engine admission supplies the command cap.
        let commands = match unsafe { slice(commands, count, u64::MAX) } {
            Ok(value) => value,
            Err(source) => return (&source).into(),
        };
        // SAFETY: exact live instance from create, called only on its engine thread.
        let instance = &unsafe { &*instance.cast::<Instance<R>>() }.runtime;
        for (ordinal, raw) in commands.iter().enumerate() {
            // SAFETY: out remains valid; command borrows the checked array.
            unsafe { bad.write(ordinal as u64) };
            let command = match unsafe { command(raw) } {
                Ok(value) => value,
                Err(source) => return (&source).into(),
            };
            if let Err(source) = instance.preflight_command(&command) {
                return (&super::CoreSchemaError::from(&source)).into();
            }
        }
        // SAFETY: canonical success does not report a failing ordinal.
        unsafe { bad.write(0) };
        SvStatus::SUCCESS
    })
}

unsafe extern "C" fn run<R: CoreRuntime>(
    instance: *const c_void,
    tx: *const SvTxV5,
    raw: *const SvCommand,
) -> SvStatus {
    caught(|| {
        if let Err(source) = pointer(instance.cast::<Instance<R>>()) {
            return (&source).into();
        }
        // SAFETY: host supplies call-scoped command and complete transaction table.
        let command = match unsafe { command(raw) } {
            Ok(value) => value,
            Err(source) => return (&source).into(),
        };
        let mut tx = match unsafe { PluginTx::from_raw(tx) } {
            Ok(value) => value,
            Err(source) => return (&source).into(),
        };
        // SAFETY: live engine-thread instance; SDK holds the transaction loan
        // until the call returns and does not let author code retain it.
        let status = caught(|| {
            let result = unsafe { &*instance.cast::<Instance<R>>() }
                .runtime
                .run_tx(&mut tx, &command);
            tx.finish(result)
        });
        tx.failure.finish_status(status)
    })
}

unsafe extern "C" fn rules_call<R: CoreRuntime>(
    instance: *const c_void,
    candidate: *const SvCandidateV5,
) -> SvStatus {
    caught(|| {
        let Some(hook) = R::TX_INVARIANTS else {
            return (&CoreAbiError::InvalidHandle).into();
        };
        if let Err(source) = pointer(instance.cast::<Instance<R>>()) {
            return (&source).into();
        }
        // SAFETY: host loans an immutable candidate and checked callback table.
        let candidate = match unsafe { Candidate::from_raw(candidate) } {
            Ok(value) => value,
            Err(source) => return (&source).into(),
        };
        // SAFETY: live instance, no mutable runtime alias during the rule callback.
        let status = caught(|| {
            let result = hook(
                &unsafe { &*instance.cast::<Instance<R>>() }.runtime,
                &candidate,
            );
            candidate.failure.finish(result)
        });
        candidate.failure.finish_status(status)
    })
}

unsafe extern "C" fn initialize<R: CoreRuntime>(
    instance: *const c_void,
    tx: *const SvTxV5,
    input: SvBytes,
) -> SvStatus {
    caught(|| {
        if let Err(source) = pointer(instance.cast::<Instance<R>>()) {
            return (&source).into();
        }
        // SAFETY: checked scoped input and host transaction table. They remain
        // valid until this synchronous callback and all visitors return.
        let input = match unsafe { bytes(input) } {
            Ok(value) => value,
            Err(source) => return (&source).into(),
        };
        let mut tx = match unsafe { PluginTx::from_raw(tx) } {
            Ok(value) => value,
            Err(source) => return (&source).into(),
        };
        let status = caught(|| {
            // SAFETY: exact live SDK allocation, exclusively on its engine thread.
            let result = unsafe { &*instance.cast::<Instance<R>>() }.runtime.initialize_core(&mut tx, input);
            tx.finish(result)
        });
        tx.failure.finish_status(status)
    })
}

unsafe extern "C" fn migrate<R: CoreRuntime>(
    instance: *const c_void,
    tx: *const SvMigrationV5,
    source: *const [u8; 32],
    input: SvBytes,
) -> SvStatus {
    caught(|| {
        if let Err(source) = pointer(instance.cast::<Instance<R>>()).and_then(|()| pointer(source)) {
            return (&source).into();
        }
        // SAFETY: host lends this fixed version and input for the call.
        let version = statevec_model::CoreVersion::from_digest(statevec_model::Digest32::new(unsafe { *source }));
        let input = match unsafe { bytes(input) } {
            Ok(value) => value,
            Err(source) => return (&source).into(),
        };
        // SAFETY: host loans a complete scoped migration table.
        let mut tx = match unsafe { super::MigrationTx::from_raw(tx) } {
            Ok(value) => value,
            Err(source) => return (&source).into(),
        };
        let status = caught(|| {
            // SAFETY: same live engine-thread SDK instance, no retained visitor.
            let result = unsafe { &*instance.cast::<Instance<R>>() }.runtime.migrate(&mut tx, version, input);
            tx.failure.finish(result)
        });
        tx.failure.finish_status(status)
    })
}

unsafe extern "C" fn destroy<T>(out: *mut *mut c_void) -> SvStatus {
    caught(|| {
        if let Err(source) = pointer(out) {
            return (&source).into();
        }
        // SAFETY: host's initialized in/out slot holds the unique allocation.
        let handle = unsafe { out.read() }.cast::<T>();
        if let Err(source) = pointer(handle) {
            return (&source).into();
        }
        // SAFETY: null before Drop, including a caught destructor panic. The
        // caller cannot retry a possibly consumed allocation after failure.
        unsafe { out.write(std::ptr::null_mut()) };
        // SAFETY: only this SDK's create/configure minted this T allocation;
        // destruction runs in its allocating module, never host's allocator.
        drop(unsafe { Box::from_raw(handle) });
        SvStatus::SUCCESS
    })
}

/// Export one unit-struct CoreModule. User code supplies no raw pointer or C
/// adapter. Pair this ABI with a qualified Core V5 host; this is not a V1 shim.
#[macro_export]
macro_rules! export_core_plugin_v5 {
    ($module:ident) => {
        #[cfg(panic = "abort")]
        compile_error!("StateVec Core plugins require panic = \"unwind\"");

        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn statevec_core_plugin_entry_v5(
            out_bytes: u64,
            out: *mut $crate::v1::ffi::SvDiscovery,
        ) -> u32 {
            static MODULE: $module = $module;
            unsafe extern "C" fn configure(
                config: $crate::v1::ffi::SvBytes,
                out: *mut *mut ::std::ffi::c_void,
            ) -> $crate::v1::SvStatus {
                // SAFETY: forwarding the host's scoped V3 input/output contract.
                unsafe { $crate::v1::__export::configure(&MODULE, config, out) }
            }
            static TABLE: $crate::v1::__export::CoreExportTable =
                $crate::v1::__export::CoreExportTable::new::<$module>(configure);
            // SAFETY: caller supplies a valid fixed discovery output.
            unsafe { TABLE.discover(out_bytes, out) }
        }
    };
}

#[cfg(test)]
#[path = "../ut_plugin_v3_export.rs"]
mod tests;
