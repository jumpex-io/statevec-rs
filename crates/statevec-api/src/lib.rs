// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

//! Runtime-facing StateVec host APIs and plugin contracts.
//!
//! Domain plugins use this crate to implement [`RuntimePlugin`] and execute
//! deterministic transactions against a host-provided [`RuntimeHostContext`].
//! Runtime hosts use the same traits and the exported plugin ABI to load domain
//! code across a process or dynamic-library boundary.

pub use statevec_model::command::Command;
use statevec_model::event::{EventKind, GeneratedEventAccess};
pub use statevec_model::record::RecordKey;
use statevec_model::record::{GeneratedRecordAccess, RecordKind, SysId};
use statevec_model::{CommandDefinition, CommandKind, SchemaRegistry};

mod business_code;
mod plugin_abi_v1;
pub use business_code::BusinessRejectCode;
mod throughput_probe;
pub use plugin_abi_v1::{
    ExportedRuntimePluginV2Handle, RUNTIME_PLUGIN_ABI_VERSION_V2, RUNTIME_PLUGIN_ENTRY_V2_SYMBOL, RuntimeBytesMutRef,
    RuntimeBytesMutVisitor, RuntimeBytesRef, RuntimeBytesVisitor, RuntimeCallStatus, RuntimeCanonicalIndexCountView,
    RuntimeCommandView, RuntimeErrorBuf, RuntimeErrorKind, RuntimeErrorPhase, RuntimeHostContextV1,
    RuntimeHostContextV1Adapter, RuntimeHostVTableV1, RuntimePluginApiV2, RuntimePluginEntryV2, RuntimeReadContextV1,
    RuntimeReadContextV1Adapter, RuntimeReadVTableV1, RuntimeRecordKeyView, RuntimeRecordKeyVisitor,
    clear_runtime_error, runtime_bytes_slice, runtime_bytes_slice_mut, runtime_error_kind, runtime_error_message,
    runtime_error_text, runtime_plugin_create_runtime_v1, runtime_plugin_destroy_runtime_v1, runtime_plugin_name_v1,
    runtime_plugin_on_unload_v1, runtime_plugin_run_tx_v1, runtime_plugin_schema_bytes_v1,
    runtime_plugin_validate_biz_invariants_v1, write_runtime_error,
};
pub use throughput_probe::RuntimeApiProbe;
use throughput_probe::{
    on_runtime_host_update_typed_by_uk, on_runtime_host_with_read_typed_by_uk,
    on_typed_tx_update_or_create_typed_by_uk, on_typed_tx_update_typed_by_uk, on_typed_tx_with_read_typed_by_uk,
};

/// Logical API compatibility version.
///
/// Compatibility is keyed to the stable runtime plugin ABI, not the crate
/// package version.
pub const STATEVEC_API_VERSION: &str = "1";
/// Numeric runtime plugin ABI compatibility version.
pub const STATEVEC_API_COMPAT_VERSION: u32 = RUNTIME_PLUGIN_ABI_VERSION_V2;

#[cfg(test)]
mod ut_api_compat_version {
    #[test]
    fn api_compat_version_is_not_the_crate_package_version() {
        assert_eq!(super::STATEVEC_API_VERSION, "1");
        assert_eq!(super::STATEVEC_API_COMPAT_VERSION, super::RUNTIME_PLUGIN_ABI_VERSION_V2);
        assert_ne!(super::STATEVEC_API_VERSION, env!("CARGO_PKG_VERSION"));
    }
}

/// Error returned by host context operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeHostError {
    /// Human-readable error message.
    pub message: String,
}

impl RuntimeHostError {
    /// Creates a host error from a message.
    pub fn new(message: impl Into<String>) -> Self {
        Self { message: message.into() }
    }
}

impl std::fmt::Display for RuntimeHostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for RuntimeHostError {}

/// Capped canonical-index count result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanonicalIndexCount {
    /// Exact count when fewer than the requested cap exist.
    Exact(usize),
    /// At least the requested cap exists; the exact count may be larger.
    AtLeast(usize),
}

/// Error returned while creating or loading a runtime plugin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimePluginLoadError {
    /// Human-readable error message.
    pub message: String,
}

impl RuntimePluginLoadError {
    /// Creates a plugin load error from a message.
    pub fn new(message: impl Into<String>) -> Self {
        Self { message: message.into() }
    }
}

impl std::fmt::Display for RuntimePluginLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for RuntimePluginLoadError {}

/// Error returned by plugin transaction execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimePluginError {
    /// Human-readable error message.
    pub message: String,
}

impl RuntimePluginError {
    /// Creates a plugin execution error from a message.
    pub fn new(message: impl Into<String>) -> Self {
        Self { message: message.into() }
    }
}

impl std::fmt::Display for RuntimePluginError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for RuntimePluginError {}

/// Error returned when unloading a runtime plugin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimePluginUnloadError {
    /// Human-readable error message.
    pub message: String,
}

impl RuntimePluginUnloadError {
    /// Creates a plugin unload error from a message.
    pub fn new(message: impl Into<String>) -> Self {
        Self { message: message.into() }
    }
}

impl std::fmt::Display for RuntimePluginUnloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for RuntimePluginUnloadError {}

/// Object-safe host capability boundary used by runtime plugins.
///
/// Engine internals such as `runtime_engine::TxAccess` stay on the host side and
/// implement this trait through an adapter. Plugin-facing code should depend on
/// this capability surface instead of any concrete engine transaction type.
pub trait RuntimeHostContext {
    /// Reads a record by system id and passes its bytes to `f` when found.
    fn with_read_typed_raw(
        &self,
        record_kind: RecordKind,
        sys_id: SysId,
        f: &mut dyn FnMut(&[u8]),
    ) -> Result<bool, RuntimeHostError>;

    /// Reads a record by unique-key bytes and passes its bytes to `f` when found.
    fn with_read_typed_by_uk_raw(
        &self,
        record_kind: RecordKind,
        uk: &[u8],
        f: &mut dyn FnMut(&[u8]),
    ) -> Result<bool, RuntimeHostError>;

    /// Creates a record and initializes its data bytes through `init`.
    fn create_typed_raw(
        &mut self,
        record_kind: RecordKind,
        init: &mut dyn FnMut(&mut [u8]),
    ) -> Result<RecordKey, RuntimeHostError>;

    /// Updates a record by unique-key bytes in-place when found.
    fn update_typed_by_uk_raw(
        &mut self,
        record_kind: RecordKind,
        uk: &[u8],
        f: &mut dyn FnMut(&mut [u8]),
    ) -> Result<bool, RuntimeHostError>;

    /// Deletes a record by unique-key bytes.
    fn delete_by_uk_raw(&mut self, record_kind: RecordKind, uk: &[u8]) -> Result<bool, RuntimeHostError>;

    /// Emits an event payload from the current transaction.
    fn emit_typed_event_raw(&mut self, event_kind: EventKind, payload: &[u8]) -> Result<(), RuntimeHostError>;

    /// Iterates visible record keys for one record kind.
    fn for_each_record_key_raw(&self, kind: RecordKind, f: &mut dyn FnMut(RecordKey)) -> Result<(), RuntimeHostError>;

    /// Counts a visible canonical-index prefix, scanning at most `cap` entries.
    fn count_index_prefix_capped_raw(
        &self,
        record_kind: RecordKind,
        index_id: u8,
        prefix: &[u8],
        cap: usize,
    ) -> Result<CanonicalIndexCount, RuntimeHostError>;

    /// Emits host-side diagnostic text.
    fn debug_log(&mut self, _message: String) -> Result<(), RuntimeHostError> {
        Ok(())
    }
}

/// Typed convenience methods layered on top of the raw
/// [`RuntimeHostContext`] capability boundary.
///
/// Plugin authors use this extension trait directly on `&mut dyn
/// RuntimeHostContext`, while host-side engine code only needs to implement the
/// raw object-safe methods.
pub trait RuntimeHostContextExt: RuntimeHostContext {
    /// Reads a generated record by system id.
    fn with_read_typed<R, T, F>(&self, sys_id: SysId, f: F) -> Result<Option<T>, RuntimeHostError>
    where
        R: GeneratedRecordAccess,
        F: FnOnce(R::Access<'_>) -> T,
    {
        let mut f = Some(f);
        let mut out = None;
        let found = self.with_read_typed_raw(R::KIND, sys_id, &mut |data| {
            let apply = f.take().expect("callback invoked more than once");
            out = Some(apply(R::wrap(data)));
        })?;
        Ok(found.then_some(out).flatten())
    }

    /// Read a record by unique key without mutating it.
    ///
    /// Prefer [`RuntimeHostContextExt::update_typed_by_uk`] when the next step
    /// is to modify the same record. The update path keeps the mutation
    /// in-place and avoids an extra resolve/read/then-update round-trip.
    fn with_read_typed_by_uk<R, P, T, F>(&self, uk: P, f: F) -> Result<Option<T>, RuntimeHostError>
    where
        R: GeneratedRecordAccess,
        P: AsRef<[u8]>,
        F: FnOnce(R::Access<'_>) -> T,
    {
        on_runtime_host_with_read_typed_by_uk();
        let mut f = Some(f);
        let mut out = None;
        let found = self.with_read_typed_by_uk_raw(R::KIND, uk.as_ref(), &mut |data| {
            let apply = f.take().expect("callback invoked more than once");
            out = Some(apply(R::wrap(data)));
        })?;
        Ok(found.then_some(out).flatten())
    }

    /// Creates a generated record.
    fn create_typed<R, F>(&mut self, init: F) -> Result<RecordKey, RuntimeHostError>
    where
        R: GeneratedRecordAccess,
        F: for<'b> FnOnce(&mut R::NewBuilder<'b>),
    {
        let mut init = Some(init);
        let key = self.create_typed_raw(R::KIND, &mut |buf| {
            let apply = init.take().expect("init callback invoked more than once");
            let mut builder = R::wrap_new(buf);
            apply(&mut builder);
        })?;
        Ok(key)
    }

    /// Update a record in-place by unique key.
    ///
    /// This is the preferred hot-path API when a command needs to mutate an
    /// existing record. It avoids the read-then-update pattern that would
    /// otherwise resolve the unique key and touch the same record twice.
    fn update_typed_by_uk<R, P, T, F>(&mut self, uk: P, f: F) -> Result<Option<T>, RuntimeHostError>
    where
        R: GeneratedRecordAccess,
        P: AsRef<[u8]>,
        F: for<'b> FnOnce(&mut R::UpdateBuilder<'b>) -> T,
    {
        on_runtime_host_update_typed_by_uk();
        let mut f = Some(f);
        let mut out = None;
        let found = self.update_typed_by_uk_raw(R::KIND, uk.as_ref(), &mut |buf| {
            let apply = f.take().expect("update callback invoked more than once");
            let mut builder = R::wrap_update(buf);
            out = Some(apply(&mut builder));
        })?;
        Ok(found.then_some(out).flatten())
    }

    /// Updates an existing record by unique key or creates a new one.
    fn update_or_create_typed_by_uk<R, P, T, FU, FC>(
        &mut self,
        uk: P,
        update: FU,
        create: FC,
    ) -> Result<T, RuntimeHostError>
    where
        R: GeneratedRecordAccess,
        P: AsRef<[u8]>,
        FU: for<'b> FnOnce(&mut R::UpdateBuilder<'b>) -> T,
        FC: for<'b> FnOnce(&mut R::NewBuilder<'b>) -> T,
    {
        if let Some(value) = self.update_typed_by_uk::<R, P, T, FU>(uk, update)? {
            return Ok(value);
        }

        let mut out = None;
        self.create_typed::<R, _>(|builder| {
            out = Some(create(builder));
        })?;
        Ok(out.expect("create closure must produce a value"))
    }

    /// Deletes a generated record by unique key.
    fn delete_by_uk<R, P>(&mut self, uk: P) -> Result<bool, RuntimeHostError>
    where
        R: GeneratedRecordAccess,
        P: AsRef<[u8]>,
    {
        self.delete_by_uk_raw(R::KIND, uk.as_ref())
    }

    /// Emits a generated event payload.
    fn emit_typed_event<E>(&mut self, payload: Vec<u8>) -> Result<(), RuntimeHostError>
    where
        E: GeneratedEventAccess,
    {
        self.emit_typed_event_raw(E::KIND, &payload)
    }

    /// Iterates record keys for one record kind.
    fn for_each_record_key(&self, kind: RecordKind, f: &mut dyn FnMut(RecordKey)) -> Result<(), RuntimeHostError> {
        self.for_each_record_key_raw(kind, &mut |key| f(key))
    }
}

impl<T: RuntimeHostContext + ?Sized> RuntimeHostContextExt for T {}

/// Factory used by a runtime host to create configured plugin instances.
pub trait RuntimePluginFactory {
    /// Stable plugin name.
    fn plugin_name(&self) -> &'static str;

    /// Schema registry exposed by this plugin.
    fn schema_registry(&self) -> SchemaRegistry;

    /// Command definitions exposed by this plugin.
    fn command_definitions(&self) -> &'static [&'static CommandDefinition] {
        &[]
    }

    /// Creates a configured runtime plugin instance.
    fn create(&self, plugin_config_text: &str) -> Result<Box<dyn RuntimePlugin>, RuntimePluginLoadError>;
}

/// Read-only state access surface for invariant validation.
///
/// This is the read-only subset of [`RuntimeHostContext`], provided to
/// [`RuntimePlugin::validate_biz_invariants`] so that domain-specific business
/// invariants can be checked without mutating state.
pub trait BizInvariantReadContext {
    /// Reads a record by system id and passes its bytes to `f` when found.
    fn with_read_typed_raw(
        &self,
        record_kind: RecordKind,
        sys_id: SysId,
        f: &mut dyn FnMut(&[u8]),
    ) -> Result<bool, RuntimeHostError>;

    /// Reads a record by unique-key bytes and passes its bytes to `f` when found.
    fn with_read_typed_by_uk_raw(
        &self,
        record_kind: RecordKind,
        uk: &[u8],
        f: &mut dyn FnMut(&[u8]),
    ) -> Result<bool, RuntimeHostError>;

    /// Iterates visible record keys for one record kind.
    fn for_each_record_key_raw(&self, kind: RecordKind, f: &mut dyn FnMut(RecordKey)) -> Result<(), RuntimeHostError>;
}

/// Typed convenience methods for [`BizInvariantReadContext`].
pub trait InvariantReadContextExt: BizInvariantReadContext {
    /// Reads a generated record by system id.
    fn with_read_typed<R, T, F>(&self, sys_id: SysId, f: F) -> Result<Option<T>, RuntimeHostError>
    where
        R: GeneratedRecordAccess,
        F: FnOnce(R::Access<'_>) -> T,
    {
        let mut result = None;
        let mut f = Some(f);
        let found = self.with_read_typed_raw(R::KIND, sys_id, &mut |data| {
            if let Some(f) = f.take() {
                result = Some(f(R::wrap(data)));
            }
        })?;
        if found { Ok(result) } else { Ok(None) }
    }

    /// Reads a generated record by unique key.
    fn with_read_typed_by_uk<R, P, T, F>(&self, uk: P, f: F) -> Result<Option<T>, RuntimeHostError>
    where
        R: GeneratedRecordAccess,
        P: AsRef<[u8]>,
        F: FnOnce(R::Access<'_>) -> T,
    {
        let mut result = None;
        let mut f = Some(f);
        let found = self.with_read_typed_by_uk_raw(R::KIND, uk.as_ref(), &mut |data| {
            if let Some(f) = f.take() {
                result = Some(f(R::wrap(data)));
            }
        })?;
        if found { Ok(result) } else { Ok(None) }
    }
}

impl<T: BizInvariantReadContext + ?Sized> InvariantReadContextExt for T {}

/// Domain runtime plugin contract.
pub trait RuntimePlugin {
    /// Stable plugin name.
    fn name(&self) -> &'static str;

    /// Schema registry exposed by this plugin.
    fn schema_registry(&self) -> SchemaRegistry;

    /// Command definitions exposed by this plugin.
    fn command_definitions(&self) -> &'static [&'static CommandDefinition] {
        &[]
    }

    /// Executes one command transaction.
    fn run_tx(
        &self,
        tx: &mut dyn RuntimeHostContext,
        command: &dyn RuntimeCommandEnvelope,
    ) -> Result<(), RuntimePluginError>;

    /// Validate domain-specific business invariants against committed state.
    ///
    /// Called at two points:
    /// - after recovery replay, before declaring recovered state healthy
    /// - before publishing a new recovery boundary (snapshot publication)
    ///
    /// Return `Ok(())` if all invariants hold, or `Err(message)` to block
    /// the operation. The default implementation performs no checks.
    fn validate_biz_invariants(&self, _ctx: &dyn BizInvariantReadContext) -> Result<(), String> {
        Ok(())
    }

    /// Called before plugin unload so domain code can release resources.
    fn on_unload(&mut self) -> Result<(), RuntimePluginUnloadError> {
        Ok(())
    }
}

/// Borrowed command envelope passed to runtime plugins.
pub trait RuntimeCommandEnvelope {
    /// Returns the command kind.
    fn command_kind(&self) -> CommandKind;
    /// Returns the source queue sequence.
    fn ext_seq(&self) -> u64;
    /// Returns the source-provided reference time in microseconds.
    fn ref_ext_time_us(&self) -> u64;
    /// Returns the encoded command payload.
    fn payload(&self) -> &[u8];
}

/// Borrowed runtime command envelope.
#[derive(Debug, Clone, Copy)]
pub struct RuntimeCommandRef<'a> {
    command_kind: CommandKind,
    ext_seq: u64,
    ref_ext_time_us: u64,
    payload: &'a [u8],
}

impl<'a> RuntimeCommandRef<'a> {
    /// Creates a borrowed runtime command envelope.
    #[inline]
    pub fn new(command_kind: CommandKind, ext_seq: u64, ref_ext_time_us: u64, payload: &'a [u8]) -> Self {
        Self { command_kind, ext_seq, ref_ext_time_us, payload }
    }
}

impl RuntimeCommandEnvelope for RuntimeCommandRef<'_> {
    #[inline(always)]
    fn command_kind(&self) -> CommandKind {
        self.command_kind
    }

    #[inline(always)]
    fn ext_seq(&self) -> u64 {
        self.ext_seq
    }

    #[inline(always)]
    fn ref_ext_time_us(&self) -> u64 {
        self.ref_ext_time_us
    }

    #[inline(always)]
    fn payload(&self) -> &[u8] {
        self.payload
    }
}

impl RuntimeCommandEnvelope for Command {
    #[inline(always)]
    fn command_kind(&self) -> CommandKind {
        self.command_kind()
    }

    #[inline(always)]
    fn ext_seq(&self) -> u64 {
        self.ext_seq()
    }

    #[inline(always)]
    fn ref_ext_time_us(&self) -> u64 {
        self.ref_ext_time_us()
    }

    #[inline(always)]
    fn payload(&self) -> &[u8] {
        self.payload()
    }
}

/// Read-only raw transaction capability used by typed convenience APIs.
pub trait TxReadContext {
    /// Host error type.
    type Error;

    /// Reads a record by key.
    fn with_read_raw<T>(&self, key: RecordKey, f: impl FnOnce(&[u8]) -> T) -> Result<Option<T>, Self::Error>;
    /// Iterates record keys for one record kind. An error may follow a partial
    /// visitor sequence; only `Ok(())` certifies that the scan completed.
    fn for_each_record_key(&self, kind: RecordKind, f: &mut dyn FnMut(RecordKey)) -> Result<(), Self::Error>;
    /// Counts a visible canonical-index prefix, scanning at most `cap` entries.
    fn count_index_prefix_capped_raw(
        &self,
        kind: RecordKind,
        index_id: u8,
        prefix: &[u8],
        cap: usize,
    ) -> Result<CanonicalIndexCount, Self::Error>;
}

/// Unique-key lookup capability for raw transaction contexts.
pub trait TxUkContext: TxReadContext {
    /// Resolves unique-key bytes to a system id.
    fn resolve_uk(&self, kind: RecordKind, uk: &[u8]) -> Result<Option<SysId>, Self::Error>;
    /// Resolves bytes for a specific unique-key id to a system id.
    fn resolve_uk_id(&self, kind: RecordKind, uk_id: u8, uk: &[u8]) -> Result<Option<SysId>, Self::Error>;
}

/// Raw write capability for transaction contexts.
pub trait TxWriteContext: TxReadContext {
    /// Creates a record from encoded data bytes.
    fn create_raw(&mut self, kind: RecordKind, data: Vec<u8>) -> Result<RecordKey, Self::Error>;
    /// Updates a record by key without changing immutable fields or unique keys.
    /// A refused update leaves the transaction-visible record unchanged, including
    /// earlier successful writes. The caller may handle the error and continue.
    fn update_raw<T>(&mut self, key: RecordKey, f: impl FnOnce(&mut [u8]) -> T) -> Result<Option<T>, Self::Error>;
    /// Deletes a record by key.
    fn delete_raw(&mut self, key: RecordKey) -> Result<bool, Self::Error>;
    /// Emits an event payload.
    fn emit_event_raw(&mut self, event_kind: EventKind, payload: Vec<u8>);
    /// Emits host-side diagnostic text.
    fn debug_log(&mut self, _message: String) {}
}

/// Raw create capability that accepts a host-assigned system id.
pub trait TxSysIdCreateContext: TxWriteContext {
    /// Creates a record with an explicit system id.
    /// An id already used by any record cannot be reused, even after deletion.
    fn create_with_sys_id_raw(
        &mut self,
        kind: RecordKind,
        sys_id: SysId,
        data: Vec<u8>,
    ) -> Result<RecordKey, Self::Error>;
}

/// Full raw transaction context capability set.
pub trait TxContext: TxUkContext + TxSysIdCreateContext {}

impl<T: TxUkContext + TxSysIdCreateContext + ?Sized> TxContext for T {}

/// Typed transaction convenience API over generated StateVec accessors.
pub trait TypedTxContext {
    /// Host error type.
    type Error;

    /// Reads a generated record by system id.
    fn with_read_typed<R, T, F>(&self, sys_id: SysId, f: F) -> Result<Option<T>, Self::Error>
    where
        R: GeneratedRecordAccess,
        F: FnOnce(R::Access<'_>) -> T;

    /// Read a record by unique key without mutating it.
    ///
    /// Prefer [`TypedTxContext::update_typed_by_uk`] when the next step is to
    /// modify the same record. The update path keeps the operation in-place and
    /// avoids the extra resolve/read/then-update round-trip that this read-first
    /// pattern introduces on hot paths.
    fn with_read_typed_by_uk<R, P, T, F>(&self, uk: P, f: F) -> Result<Option<T>, Self::Error>
    where
        R: GeneratedRecordAccess,
        P: AsRef<[u8]>,
        F: FnOnce(R::Access<'_>) -> T;

    /// Read a record by a specific unique-key id without mutating it.
    fn with_read_typed_by_uk_id<R, P, T, F>(&self, uk_id: u8, uk: P, f: F) -> Result<Option<T>, Self::Error>
    where
        R: GeneratedRecordAccess,
        P: AsRef<[u8]>,
        F: FnOnce(R::Access<'_>) -> T;

    /// Creates a generated record.
    fn create_typed<R, F>(&mut self, init: F) -> Result<RecordKey, Self::Error>
    where
        R: GeneratedRecordAccess,
        F: for<'b> FnOnce(&mut R::NewBuilder<'b>);

    /// Update a record in-place by unique key.
    ///
    /// This is the preferred hot-path API when a command needs to modify an
    /// existing record. It avoids the read-then-update pattern that would
    /// otherwise resolve the unique key and touch the same record twice.
    fn update_typed_by_uk<R, P, T, F>(&mut self, uk: P, f: F) -> Result<Option<T>, Self::Error>
    where
        R: GeneratedRecordAccess,
        P: AsRef<[u8]>,
        F: for<'b> FnOnce(&mut R::UpdateBuilder<'b>) -> T;

    /// Update a record in-place by a specific unique-key id.
    fn update_typed_by_uk_id<R, P, T, F>(&mut self, uk_id: u8, uk: P, f: F) -> Result<Option<T>, Self::Error>
    where
        R: GeneratedRecordAccess,
        P: AsRef<[u8]>,
        F: for<'b> FnOnce(&mut R::UpdateBuilder<'b>) -> T;

    /// Updates an existing record by unique key or creates a new one.
    fn update_or_create_typed_by_uk<R, P, T, FU, FC>(&mut self, uk: P, update: FU, create: FC) -> Result<T, Self::Error>
    where
        R: GeneratedRecordAccess,
        P: AsRef<[u8]>,
        FU: for<'b> FnOnce(&mut R::UpdateBuilder<'b>) -> T,
        FC: for<'b> FnOnce(&mut R::NewBuilder<'b>) -> T,
    {
        if let Some(value) = self.update_typed_by_uk::<R, P, T, FU>(uk, update)? {
            return Ok(value);
        }
        let mut out = None;
        self.create_typed::<R, _>(|builder| {
            out = Some(create(builder));
        })?;
        Ok(out.expect("create closure must produce a value"))
    }

    /// Deletes a generated record by unique key.
    fn delete_by_uk<R, P>(&mut self, uk: P) -> Result<bool, Self::Error>
    where
        R: GeneratedRecordAccess,
        P: AsRef<[u8]>;

    /// Deletes a generated record by a specific unique-key id.
    fn delete_by_uk_id<R, P>(&mut self, uk_id: u8, uk: P) -> Result<bool, Self::Error>
    where
        R: GeneratedRecordAccess,
        P: AsRef<[u8]>;

    /// Emits a generated event payload.
    fn emit_typed_event<E>(&mut self, payload: Vec<u8>)
    where
        E: GeneratedEventAccess;

    /// Iterates record keys for one record kind. An error may follow a partial
    /// visitor sequence; only `Ok(())` certifies that the scan completed.
    fn for_each_record_key(&self, kind: RecordKind, f: &mut dyn FnMut(RecordKey)) -> Result<(), Self::Error>;

    /// Counts a canonical-index prefix, scanning at most `cap` entries.
    fn count_index_prefix_capped_raw(
        &self,
        record_kind: RecordKind,
        index_id: u8,
        prefix: &[u8],
        cap: usize,
    ) -> Result<CanonicalIndexCount, Self::Error>;

    /// Counts a generated record canonical-index prefix.
    fn count_index_prefix_capped<R, P>(
        &self,
        index_id: u8,
        prefix: P,
        cap: usize,
    ) -> Result<CanonicalIndexCount, Self::Error>
    where
        R: GeneratedRecordAccess,
        P: AsRef<[u8]>,
    {
        self.count_index_prefix_capped_raw(R::KIND, index_id, prefix.as_ref(), cap)
    }

    /// Emits host-side diagnostic text.
    fn debug_log(&mut self, _message: String) {}
}

impl<Ctx: TxContext + ?Sized> TypedTxContext for Ctx {
    type Error = <Ctx as TxReadContext>::Error;

    fn with_read_typed<R, T, F>(&self, sys_id: SysId, f: F) -> Result<Option<T>, Self::Error>
    where
        R: GeneratedRecordAccess,
        F: FnOnce(R::Access<'_>) -> T,
    {
        TxReadContext::with_read_raw(self, RecordKey { kind: R::KIND, sys_id }, |data| f(R::wrap(data)))
    }

    fn with_read_typed_by_uk<R, P, T, F>(&self, uk: P, f: F) -> Result<Option<T>, Self::Error>
    where
        R: GeneratedRecordAccess,
        P: AsRef<[u8]>,
        F: FnOnce(R::Access<'_>) -> T,
    {
        self.with_read_typed_by_uk_id::<R, P, T, F>(0, uk, f)
    }

    fn with_read_typed_by_uk_id<R, P, T, F>(&self, uk_id: u8, uk: P, f: F) -> Result<Option<T>, Self::Error>
    where
        R: GeneratedRecordAccess,
        P: AsRef<[u8]>,
        F: FnOnce(R::Access<'_>) -> T,
    {
        on_typed_tx_with_read_typed_by_uk();
        let Some(sys_id) = TxUkContext::resolve_uk_id(self, R::KIND, uk_id, uk.as_ref())? else {
            return Ok(None);
        };
        TypedTxContext::with_read_typed::<R, T, F>(self, sys_id, f)
    }

    fn create_typed<R, F>(&mut self, init: F) -> Result<RecordKey, Self::Error>
    where
        R: GeneratedRecordAccess,
        F: for<'b> FnOnce(&mut R::NewBuilder<'b>),
    {
        let mut data = vec![0u8; R::DATA_LEN];
        init(&mut R::wrap_new(&mut data));
        TxWriteContext::create_raw(self, R::KIND, data)
    }

    fn update_typed_by_uk<R, P, T, F>(&mut self, uk: P, f: F) -> Result<Option<T>, Self::Error>
    where
        R: GeneratedRecordAccess,
        P: AsRef<[u8]>,
        F: for<'b> FnOnce(&mut R::UpdateBuilder<'b>) -> T,
    {
        self.update_typed_by_uk_id::<R, P, T, F>(0, uk, f)
    }

    fn update_typed_by_uk_id<R, P, T, F>(&mut self, uk_id: u8, uk: P, f: F) -> Result<Option<T>, Self::Error>
    where
        R: GeneratedRecordAccess,
        P: AsRef<[u8]>,
        F: for<'b> FnOnce(&mut R::UpdateBuilder<'b>) -> T,
    {
        on_typed_tx_update_typed_by_uk();
        let Some(sys_id) = TxUkContext::resolve_uk_id(self, R::KIND, uk_id, uk.as_ref())? else {
            return Ok(None);
        };
        TxWriteContext::update_raw(self, RecordKey { kind: R::KIND, sys_id }, |data| {
            let mut builder = R::wrap_update(data);
            f(&mut builder)
        })
    }

    fn update_or_create_typed_by_uk<R, P, T, FU, FC>(&mut self, uk: P, update: FU, create: FC) -> Result<T, Self::Error>
    where
        R: GeneratedRecordAccess,
        P: AsRef<[u8]>,
        FU: for<'b> FnOnce(&mut R::UpdateBuilder<'b>) -> T,
        FC: for<'b> FnOnce(&mut R::NewBuilder<'b>) -> T,
    {
        on_typed_tx_update_or_create_typed_by_uk();
        if let Some(value) = self.update_typed_by_uk::<R, P, T, FU>(uk, update)? {
            return Ok(value);
        }

        let mut out = None;
        self.create_typed::<R, _>(|builder| {
            out = Some(create(builder));
        })?;
        Ok(out.expect("create closure must produce a value"))
    }

    fn delete_by_uk<R, P>(&mut self, uk: P) -> Result<bool, Self::Error>
    where
        R: GeneratedRecordAccess,
        P: AsRef<[u8]>,
    {
        self.delete_by_uk_id::<R, P>(0, uk)
    }

    fn delete_by_uk_id<R, P>(&mut self, uk_id: u8, uk: P) -> Result<bool, Self::Error>
    where
        R: GeneratedRecordAccess,
        P: AsRef<[u8]>,
    {
        let Some(sys_id) = TxUkContext::resolve_uk_id(self, R::KIND, uk_id, uk.as_ref())? else {
            return Ok(false);
        };
        TxWriteContext::delete_raw(self, RecordKey { kind: R::KIND, sys_id })
    }

    fn emit_typed_event<E>(&mut self, payload: Vec<u8>)
    where
        E: GeneratedEventAccess,
    {
        TxWriteContext::emit_event_raw(self, E::KIND, payload);
    }

    fn for_each_record_key(&self, kind: RecordKind, f: &mut dyn FnMut(RecordKey)) -> Result<(), Self::Error> {
        TxReadContext::for_each_record_key(self, kind, f)
    }

    fn count_index_prefix_capped_raw(
        &self,
        record_kind: RecordKind,
        index_id: u8,
        prefix: &[u8],
        cap: usize,
    ) -> Result<CanonicalIndexCount, Self::Error> {
        TxReadContext::count_index_prefix_capped_raw(self, record_kind, index_id, prefix, cap)
    }

    fn debug_log(&mut self, message: String) {
        TxWriteContext::debug_log(self, message);
    }
}

/// Bridge `dyn RuntimeHostContext` to [`TypedTxContext`] so that
/// `command_dispatch!`-generated code can operate on plugin-facing host
/// adapters without requiring engine-internal raw transaction capabilities.
impl TypedTxContext for dyn RuntimeHostContext + '_ {
    type Error = RuntimeHostError;

    fn with_read_typed<R, T, F>(&self, sys_id: SysId, f: F) -> Result<Option<T>, Self::Error>
    where
        R: GeneratedRecordAccess,
        F: FnOnce(R::Access<'_>) -> T,
    {
        RuntimeHostContextExt::with_read_typed::<R, T, F>(self, sys_id, f)
    }

    fn with_read_typed_by_uk<R, P, T, F>(&self, uk: P, f: F) -> Result<Option<T>, Self::Error>
    where
        R: GeneratedRecordAccess,
        P: AsRef<[u8]>,
        F: FnOnce(R::Access<'_>) -> T,
    {
        RuntimeHostContextExt::with_read_typed_by_uk::<R, P, T, F>(self, uk, f)
    }

    fn with_read_typed_by_uk_id<R, P, T, F>(&self, uk_id: u8, uk: P, f: F) -> Result<Option<T>, Self::Error>
    where
        R: GeneratedRecordAccess,
        P: AsRef<[u8]>,
        F: FnOnce(R::Access<'_>) -> T,
    {
        if uk_id == 0 {
            return RuntimeHostContextExt::with_read_typed_by_uk::<R, P, T, F>(self, uk, f);
        }
        Err(RuntimeHostError::new("RuntimeHostContext does not support secondary UK lookup yet"))
    }

    fn create_typed<R, F>(&mut self, init: F) -> Result<RecordKey, Self::Error>
    where
        R: GeneratedRecordAccess,
        F: for<'b> FnOnce(&mut R::NewBuilder<'b>),
    {
        RuntimeHostContextExt::create_typed::<R, F>(self, init)
    }

    fn update_typed_by_uk<R, P, T, F>(&mut self, uk: P, f: F) -> Result<Option<T>, Self::Error>
    where
        R: GeneratedRecordAccess,
        P: AsRef<[u8]>,
        F: for<'b> FnOnce(&mut R::UpdateBuilder<'b>) -> T,
    {
        RuntimeHostContextExt::update_typed_by_uk::<R, P, T, F>(self, uk, f)
    }

    fn update_typed_by_uk_id<R, P, T, F>(&mut self, uk_id: u8, uk: P, f: F) -> Result<Option<T>, Self::Error>
    where
        R: GeneratedRecordAccess,
        P: AsRef<[u8]>,
        F: for<'b> FnOnce(&mut R::UpdateBuilder<'b>) -> T,
    {
        if uk_id == 0 {
            return RuntimeHostContextExt::update_typed_by_uk::<R, P, T, F>(self, uk, f);
        }
        Err(RuntimeHostError::new("RuntimeHostContext does not support secondary UK lookup yet"))
    }

    fn delete_by_uk<R, P>(&mut self, uk: P) -> Result<bool, Self::Error>
    where
        R: GeneratedRecordAccess,
        P: AsRef<[u8]>,
    {
        RuntimeHostContextExt::delete_by_uk::<R, P>(self, uk)
    }

    fn delete_by_uk_id<R, P>(&mut self, uk_id: u8, uk: P) -> Result<bool, Self::Error>
    where
        R: GeneratedRecordAccess,
        P: AsRef<[u8]>,
    {
        if uk_id == 0 {
            return RuntimeHostContextExt::delete_by_uk::<R, P>(self, uk);
        }
        Err(RuntimeHostError::new("RuntimeHostContext does not support secondary UK lookup yet"))
    }

    fn emit_typed_event<E>(&mut self, payload: Vec<u8>)
    where
        E: GeneratedEventAccess,
    {
        RuntimeHostContextExt::emit_typed_event::<E>(self, payload).expect("host emit_typed_event_raw failed");
    }

    fn for_each_record_key(&self, kind: RecordKind, f: &mut dyn FnMut(RecordKey)) -> Result<(), Self::Error> {
        RuntimeHostContextExt::for_each_record_key(self, kind, f)
    }

    fn count_index_prefix_capped_raw(
        &self,
        record_kind: RecordKind,
        index_id: u8,
        prefix: &[u8],
        cap: usize,
    ) -> Result<CanonicalIndexCount, Self::Error> {
        RuntimeHostContext::count_index_prefix_capped_raw(self, record_kind, index_id, prefix, cap)
    }

    fn debug_log(&mut self, message: String) {
        let _ = RuntimeHostContext::debug_log(self, message);
    }
}

/// Domain/runtime rejection category for determined command outcomes.
///
/// Business codes occupy `1..=59999`; `60000..=65535` are framework-reserved.
/// This standalone status projection does not turn a cluster execution fault
/// into an ordinary business rejection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RejectedErrorCode {
    /// Exact reason selected by the domain runtime.
    Business(BusinessRejectCode),
    /// Runtime trapped a panic while executing the command.
    RuntimePanic,
}

impl RejectedErrorCode {
    /// Encodes the error code as `u16`.
    pub fn to_u16(self) -> u16 {
        match self {
            Self::Business(code) => code.get(),
            Self::RuntimePanic => 60003,
        }
    }

    /// Decodes the error code from `u16`.
    pub fn from_u16(value: u16) -> Option<Self> {
        match value {
            60003 => Some(Self::RuntimePanic),
            code => BusinessRejectCode::new(code).map(Self::Business),
        }
    }
}

/// Determined command outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommittedStatus {
    /// Determined final outcome with state delta and allocated `tx_seq`.
    Committed { tx_seq: u64 },
    /// Determined final outcome without state delta but still tied to the
    /// command's accepted `ext_seq`.
    Rejected { error_code: RejectedErrorCode },
}
