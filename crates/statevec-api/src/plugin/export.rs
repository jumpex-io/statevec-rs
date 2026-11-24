//! SDK-generated Core entrypoints. Authors implement safe traits only.
use super::author::*;
use super::borrow::*;
use super::candidate::Candidate;
use super::ffi::*;
use super::tx::PluginTx;
use super::{ConfigureError, CoreAbiError, SvStatus};
use crate::{RuntimeCommandEnvelope, RuntimeCommandRef};
use statevec_model::{CommandKind, CommandPayloadValidator, CommandSchemaError, SchemaIdentity};
use std::ffi::c_void;
use std::mem::size_of;

#[cfg(all(target_arch = "x86_64", target_os = "linux", target_env = "gnu"))]
const TARGET: &str = "x86_64-unknown-linux-gnu";
#[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
const TARGET: &str = "aarch64-unknown-linux-gnu";
#[cfg(not(all(any(target_arch = "x86_64", target_arch = "aarch64"), target_os = "linux", target_env = "gnu")))]
const TARGET: &str = "unsupported";

/// Private macro support. Only built from static target bytes and monomorphized
/// SDK functions; this wrapper does not make arbitrary raw tables Sync.
#[doc(hidden)]
pub struct CoreExportTable(SvCore);
// SAFETY: constructor owns an immutable table containing only static target
// bytes/function addresses. It contains no configured or runtime state.
unsafe impl Sync for CoreExportTable {}
impl CoreExportTable {
    /// # Safety
    /// `configure` must forward its arguments to `__export::configure` with
    /// this same `M`, or never report success. The table's `metadata`,
    /// `create_instance` and `destroy_configured` read every handle it returns
    /// as `M`'s configured value.
    pub const unsafe fn new<M: CoreModule>(
        configure: unsafe extern "C" fn(SvBytes, *mut *mut c_void) -> SvStatus,
    ) -> Self {
        let rules = <<M::Configured as ConfiguredCore>::Runtime as CoreRuntime>::TX_INVARIANTS.is_some();
        Self(SvCore {
            h: SvHeader {
                abi_major: CORE_ABI_MAJOR,
                kind: CORE_KIND,
                table_bytes: size_of::<SvCore>() as u64,
                provided_caps: CORE_CAPABILITIES,
                required_caps: CORE_BASE_CAPABILITIES | if rules { CORE_CANDIDATE_CAPABILITY } else { 0 },
            },
            build_target: SvBytes { ptr: TARGET.as_ptr(), len: TARGET.len() as u64 },
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
                table: (&self.0 as *const SvCore).cast(),
                table_bytes: size_of::<SvCore>() as u64,
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
    validators: CommandValidators,
}

// Keep zero-sized runtimes in distinct live allocations too. The byte has no
// business meaning; it only prevents Box<ZST>'s shared dangling address.
struct Instance<R> {
    runtime: R,
    validators: CommandValidators,
    _allocation: u8,
}

/// One generated validator for every command kind the configured schema
/// declares, in ascending kind order. Static function pointers only.
#[derive(Clone)]
struct CommandValidators(Box<[(CommandKind, CommandPayloadValidator)]>);
impl CommandValidators {
    /// Configuration must not succeed without a validator for every declared
    /// command: the first kind without one is refused with the SDK's code.
    fn of(schema: &statevec_model::SchemaRegistry) -> Result<Self, ConfigureError> {
        schema
            .command_defs()
            .map(|definition| {
                let kind = definition.kind;
                let validator = schema.command_payload_validator(kind);
                validator.map(|validator| (kind, validator)).ok_or(ConfigureError::missing_command_validator(kind))
            })
            .collect::<Result<_, _>>()
            .map(Self)
    }
    fn check(&self, command: &RuntimeCommandRef<'_>) -> Result<(), CommandSchemaError> {
        let kind = command.command_kind();
        match self.0.binary_search_by_key(&kind, |(declared, _)| *declared) {
            Ok(index) => (self.0[index].1)(command.payload()),
            Err(_) => Err(CommandSchemaError::UnsupportedKind { kind }),
        }
    }
}

/// # Safety
/// Caller supplies valid borrowed configuration and one writable handle output;
/// these pointers are disjoint and valid for the complete synchronous call.
#[doc(hidden)]
pub unsafe fn configure<M: CoreModule>(module: &M, config: SvBytes, out: *mut *mut c_void) -> SvStatus {
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
        let configured = match module.configure(CoreConfig::new(bytes)) {
            Ok(value) => value,
            Err(source) => return (&source).into(),
        };
        let metadata = configured.metadata();
        for (text, maximum) in [(metadata.name, 128), (metadata.revision, 4096)] {
            if text.len() > maximum {
                return (&CoreAbiError::Length { actual: text.len() as u64, maximum: maximum as u64 }).into();
            }
            if text.as_bytes().contains(&0) {
                return (&CoreAbiError::EmbeddedNul).into();
            }
        }
        let validators = match CommandValidators::of(metadata.schema) {
            Ok(validators) => validators,
            Err(source) => return (&source).into(),
        };
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
            return (&CoreAbiError::Length { actual: idl.len() as u64, maximum: maximum as u64 }).into();
        }
        let holding = Box::new(Configured { value: configured, name, revision, idl, schema, validators });
        // SAFETY: ownership transfers exactly once, after complete construction.
        unsafe { out.write(Box::into_raw(holding).cast()) };
        SvStatus::SUCCESS
    })
}

unsafe extern "C" fn metadata<C: ConfiguredCore>(handle: *const c_void, out: *mut SvCoreMetadata) -> SvStatus {
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

unsafe extern "C" fn create<C: ConfiguredCore>(handle: *const c_void, out: *mut *mut c_void) -> SvStatus {
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
            Ok(runtime) => Box::new(Instance { runtime, validators: configured.validators.clone(), _allocation: 0 }),
            Err(source) => return (&super::CoreError::Construction(source)).into(),
        };
        // SAFETY: successful construction transfers a unique plugin allocation.
        unsafe { out.write(Box::into_raw(instance).cast()) };
        SvStatus::SUCCESS
    })
}

/// # Safety
/// `command` checks `raw` for null and alignment. The caller guarantees `raw` points to an initialized `SvCommand`
/// whose payload bytes are readable, both unmodified for `'a`, and must not let `'a` outlive the host's call.
unsafe fn command<'a>(raw: *const SvCommand) -> Result<RuntimeCommandRef<'a>, CoreAbiError> {
    pointer(raw)?;
    // SAFETY: caller supplies one initialized command, borrowed for the call.
    let raw = unsafe { &*raw };
    let reserved = u64::from(raw.reserved16) | u64::from(raw.reserved32);
    if reserved != 0 {
        return Err(CoreAbiError::Reserved { actual: reserved });
    }
    // SAFETY: payload belongs to this command's call-scoped host input.
    Ok(RuntimeCommandRef::new(raw.kind, raw.ext_seq, raw.ref_ext_time_us, unsafe { bytes(raw.payload)? }))
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
        let instance = unsafe { &*instance.cast::<Instance<R>>() };
        for (ordinal, raw) in commands.iter().enumerate() {
            // SAFETY: out remains valid; command borrows the checked array.
            unsafe { bad.write(ordinal as u64) };
            // SAFETY: `raw` is an element of the checked `commands` slice; the host lends each
            // command and its payload for this call.
            let command = match unsafe { command(raw) } {
                Ok(value) => value,
                Err(source) => return (&source).into(),
            };
            // The declared schema decides first; author code sees only
            // commands its generated accessors can read.
            let checked = instance.validators.check(&command);
            if let Err(source) = checked.and_then(|()| instance.runtime.preflight_command(&command)) {
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
    tx: *const SvTx,
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
        // SAFETY: the host lends its complete `SvTx` table for this synchronous call; its
        // callbacks catch unwinds, keep no loan and never run in parallel.
        let mut tx = match unsafe { PluginTx::from_raw(tx) } {
            Ok(value) => value,
            Err(source) => return (&source).into(),
        };
        // The SDK holds the transaction loan until the call returns and does
        // not let author code retain it.
        let status = caught(|| {
            // SAFETY: `instance` is a live `Instance<R>` from this table's `create`; the host calls
            // it on one thread only and destroys it after every call has returned.
            let result = unsafe { &*instance.cast::<Instance<R>>() }.runtime.run_tx(&mut tx, &command);
            tx.finish(result)
        });
        tx.failure.finish_status(status)
    })
}

unsafe extern "C" fn rules_call<R: CoreRuntime>(instance: *const c_void, candidate: *const SvCandidate) -> SvStatus {
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
        let status = caught(|| {
            // SAFETY: `instance` is a live `Instance<R>` from this table's `create`, with no mutable
            // alias during the rule callback; the host calls it on one thread only and destroys it
            // after every call has returned.
            let result = hook(&unsafe { &*instance.cast::<Instance<R>>() }.runtime, &candidate);
            candidate.failure.finish(result)
        });
        candidate.failure.finish_status(status)
    })
}

unsafe extern "C" fn initialize<R: CoreRuntime>(
    instance: *const c_void,
    tx: *const SvTx,
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
        // SAFETY: the host lends its complete `SvTx` table for this synchronous call; its
        // callbacks catch unwinds, keep no loan and never run in parallel.
        let mut tx = match unsafe { PluginTx::from_raw(tx) } {
            Ok(value) => value,
            Err(source) => return (&source).into(),
        };
        let status = caught(|| {
            // SAFETY: exact live SDK allocation, exclusively on its engine thread.
            let result = unsafe { &*instance.cast::<Instance<R>>() }
                .runtime
                .initialize_core(&mut tx, input);
            tx.finish(result)
        });
        tx.failure.finish_status(status)
    })
}

unsafe extern "C" fn migrate<R: CoreRuntime>(
    instance: *const c_void,
    tx: *const SvMigration,
    source: *const [u8; 32],
    input: SvBytes,
) -> SvStatus {
    caught(|| {
        if let Err(source) = pointer(instance.cast::<Instance<R>>()).and_then(|()| pointer(source)) {
            return (&source).into();
        }
        // SAFETY: host lends this fixed version and input for the call.
        let version = statevec_model::CoreVersion::from_digest(statevec_model::Digest32::new(unsafe { *source }));
        // SAFETY: the host lends `input` for this synchronous call; the author receives it only
        // as a call-scoped `&[u8]` and cannot keep it.
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
            let result = unsafe { &*instance.cast::<Instance<R>>() }
                .runtime
                .migrate(&mut tx, version, input);
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

/// Export a unit struct implementing [`CoreModule`](crate::plugin::CoreModule)
/// from the final `cdylib` crate.
#[macro_export]
macro_rules! export_core_plugin {
    ($module:ident) => {
        #[cfg(panic = "abort")]
        compile_error!("StateVec Core plugins require panic = \"unwind\"");

        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn statevec_core_entry(
            out_bytes: u64,
            out: *mut $crate::plugin::ffi::SvDiscovery,
        ) -> u32 {
            static MODULE: $module = $module;
            unsafe extern "C" fn configure(
                config: $crate::plugin::ffi::SvBytes,
                out: *mut *mut ::std::ffi::c_void,
            ) -> $crate::plugin::SvStatus {
                // SAFETY: forwarding the host's scoped Core input/output contract.
                unsafe { $crate::plugin::__export::configure(&MODULE, config, out) }
            }
            // SAFETY: `configure` forwards to `__export::configure` for `$module`.
            static TABLE: $crate::plugin::__export::CoreExportTable =
                unsafe { $crate::plugin::__export::CoreExportTable::new::<$module>(configure) };
            // SAFETY: caller supplies a valid fixed discovery output.
            unsafe { TABLE.discover(out_bytes, out) }
        }
    };
}

#[cfg(test)]
#[path = "../ut_plugin_v3_export.rs"]
mod tests;
