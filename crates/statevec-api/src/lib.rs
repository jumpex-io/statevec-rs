// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

//! Runtime-facing StateVec host APIs and plugin contracts.
//!
//! Domain plugins use this crate to implement [`RuntimePlugin`] and execute
//! deterministic transactions against a host-provided [`RuntimeHostContext`].
//! Runtime hosts use the same traits and the exported plugin ABI to load domain
//! code across a process or dynamic-library boundary.

pub use statevec_model::command::Command;
use statevec_model::event::GeneratedEventAccess;
pub use statevec_model::record::RecordKey;
use statevec_model::record::{GeneratedRecordAccess, RecordKind, SysId};
use statevec_model::{CommandDefinition, SchemaRegistry};

mod plugin_abi_v1;
mod throughput_probe;
pub use plugin_abi_v1::{
    ExportedRuntimePluginV1Handle, RUNTIME_PLUGIN_ABI_VERSION_V1, RUNTIME_PLUGIN_ENTRY_V1_SYMBOL,
    RuntimeBytesMutRef, RuntimeBytesMutVisitor, RuntimeBytesRef, RuntimeBytesVisitor,
    RuntimeCallStatus, RuntimeCommandView, RuntimeErrorBuf, RuntimeErrorKind, RuntimeErrorPhase,
    RuntimeHostContextV1, RuntimeHostContextV1Adapter, RuntimeHostVTableV1, RuntimePluginApiV1,
    RuntimePluginEntryV1, RuntimeReadContextV1, RuntimeReadContextV1Adapter, RuntimeReadVTableV1,
    RuntimeRecordKeyView, RuntimeRecordKeyVisitor, clear_runtime_error, runtime_bytes_slice,
    runtime_bytes_slice_mut, runtime_error_kind, runtime_error_message, runtime_error_text,
    runtime_plugin_create_runtime_v1, runtime_plugin_destroy_runtime_v1, runtime_plugin_name_v1,
    runtime_plugin_on_unload_v1, runtime_plugin_run_tx_v1, runtime_plugin_schema_bytes_v1,
    runtime_plugin_validate_biz_invariants_v1, write_runtime_error,
};
pub use throughput_probe::RuntimeApiProbe;
use throughput_probe::{
    on_runtime_host_update_typed_by_pk, on_runtime_host_with_read_typed_by_pk,
    on_typed_tx_update_or_create_typed_by_pk, on_typed_tx_update_typed_by_pk,
    on_typed_tx_with_read_typed_by_pk,
};

/// Logical API compatibility version.
///
/// Compatibility is keyed to the stable runtime plugin ABI, not the crate
/// package version.
pub const STATEVEC_API_VERSION: &str = "1";
/// Numeric runtime plugin ABI compatibility version.
pub const STATEVEC_API_COMPAT_VERSION: u32 = RUNTIME_PLUGIN_ABI_VERSION_V1;

#[cfg(test)]
mod ut_api_compat_version {
    #[test]
    fn api_compat_version_is_not_the_crate_package_version() {
        assert_eq!(super::STATEVEC_API_VERSION, "1");
        assert_eq!(
            super::STATEVEC_API_COMPAT_VERSION,
            super::RUNTIME_PLUGIN_ABI_VERSION_V1
        );
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
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for RuntimeHostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for RuntimeHostError {}

/// Error returned while creating or loading a runtime plugin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimePluginLoadError {
    /// Human-readable error message.
    pub message: String,
}

impl RuntimePluginLoadError {
    /// Creates a plugin load error from a message.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
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
        Self {
            message: message.into(),
        }
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
        Self {
            message: message.into(),
        }
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

    /// Reads a record by primary-key bytes and passes its bytes to `f` when found.
    fn with_read_typed_by_pk_raw(
        &self,
        record_kind: RecordKind,
        pk: &[u8],
        f: &mut dyn FnMut(&[u8]),
    ) -> Result<bool, RuntimeHostError>;

    /// Creates a record and initializes its data bytes through `init`.
    fn create_typed_raw(
        &mut self,
        record_kind: RecordKind,
        init: &mut dyn FnMut(&mut [u8]),
    ) -> Result<RecordKey, RuntimeHostError>;

    /// Updates a record by primary-key bytes in-place when found.
    fn update_typed_by_pk_raw(
        &mut self,
        record_kind: RecordKind,
        pk: &[u8],
        f: &mut dyn FnMut(&mut [u8]),
    ) -> Result<bool, RuntimeHostError>;

    /// Deletes a record by primary-key bytes.
    fn delete_by_pk_raw(
        &mut self,
        record_kind: RecordKind,
        pk: &[u8],
    ) -> Result<bool, RuntimeHostError>;

    /// Emits an event payload from the current transaction.
    fn emit_typed_event_raw(
        &mut self,
        event_kind: u8,
        payload: &[u8],
    ) -> Result<(), RuntimeHostError>;

    /// Iterates visible record keys for one record kind.
    fn for_each_record_key_raw(
        &self,
        kind: RecordKind,
        f: &mut dyn FnMut(RecordKey),
    ) -> Result<(), RuntimeHostError>;

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

    /// Read a record by primary key without mutating it.
    ///
    /// Prefer [`RuntimeHostContextExt::update_typed_by_pk`] when the next step
    /// is to modify the same record. The update path keeps the mutation
    /// in-place and avoids an extra resolve/read/then-update round-trip.
    fn with_read_typed_by_pk<R, P, T, F>(&self, pk: P, f: F) -> Result<Option<T>, RuntimeHostError>
    where
        R: GeneratedRecordAccess,
        P: AsRef<[u8]>,
        F: FnOnce(R::Access<'_>) -> T,
    {
        on_runtime_host_with_read_typed_by_pk();
        let mut f = Some(f);
        let mut out = None;
        let found = self.with_read_typed_by_pk_raw(R::KIND, pk.as_ref(), &mut |data| {
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

    /// Update a record in-place by primary key.
    ///
    /// This is the preferred hot-path API when a command needs to mutate an
    /// existing record. It avoids the read-then-update pattern that would
    /// otherwise resolve the primary key and touch the same record twice.
    fn update_typed_by_pk<R, P, T, F>(&mut self, pk: P, f: F) -> Result<Option<T>, RuntimeHostError>
    where
        R: GeneratedRecordAccess,
        P: AsRef<[u8]>,
        F: for<'b> FnOnce(&mut R::UpdateBuilder<'b>) -> T,
    {
        on_runtime_host_update_typed_by_pk();
        let mut f = Some(f);
        let mut out = None;
        let found = self.update_typed_by_pk_raw(R::KIND, pk.as_ref(), &mut |buf| {
            let apply = f.take().expect("update callback invoked more than once");
            let mut builder = R::wrap_update(buf);
            out = Some(apply(&mut builder));
        })?;
        Ok(found.then_some(out).flatten())
    }

    /// Updates an existing record by primary key or creates a new one.
    fn update_or_create_typed_by_pk<R, P, T, FU, FC>(
        &mut self,
        pk: P,
        update: FU,
        create: FC,
    ) -> Result<T, RuntimeHostError>
    where
        R: GeneratedRecordAccess,
        P: AsRef<[u8]>,
        FU: for<'b> FnOnce(&mut R::UpdateBuilder<'b>) -> T,
        FC: for<'b> FnOnce(&mut R::NewBuilder<'b>) -> T,
    {
        if let Some(value) = self.update_typed_by_pk::<R, P, T, FU>(pk, update)? {
            return Ok(value);
        }

        let mut out = None;
        self.create_typed::<R, _>(|builder| {
            out = Some(create(builder));
        })?;
        Ok(out.expect("create closure must produce a value"))
    }

    /// Deletes a generated record by primary key.
    fn delete_by_pk<R, P>(&mut self, pk: P) -> Result<bool, RuntimeHostError>
    where
        R: GeneratedRecordAccess,
        P: AsRef<[u8]>,
    {
        self.delete_by_pk_raw(R::KIND, pk.as_ref())
    }

    /// Emits a generated event payload.
    fn emit_typed_event<E>(&mut self, payload: Vec<u8>) -> Result<(), RuntimeHostError>
    where
        E: GeneratedEventAccess,
    {
        self.emit_typed_event_raw(E::KIND, &payload)
    }

    /// Iterates record keys for one record kind.
    fn for_each_record_key(
        &self,
        kind: RecordKind,
        f: &mut dyn FnMut(RecordKey),
    ) -> Result<(), RuntimeHostError> {
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
    fn create(
        &self,
        plugin_config_text: &str,
    ) -> Result<Box<dyn RuntimePlugin>, RuntimePluginLoadError>;
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

    /// Reads a record by primary-key bytes and passes its bytes to `f` when found.
    fn with_read_typed_by_pk_raw(
        &self,
        record_kind: RecordKind,
        pk: &[u8],
        f: &mut dyn FnMut(&[u8]),
    ) -> Result<bool, RuntimeHostError>;

    /// Iterates visible record keys for one record kind.
    fn for_each_record_key_raw(
        &self,
        kind: RecordKind,
        f: &mut dyn FnMut(RecordKey),
    ) -> Result<(), RuntimeHostError>;
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

    /// Reads a generated record by primary key.
    fn with_read_typed_by_pk<R, P, T, F>(&self, pk: P, f: F) -> Result<Option<T>, RuntimeHostError>
    where
        R: GeneratedRecordAccess,
        P: AsRef<[u8]>,
        F: FnOnce(R::Access<'_>) -> T,
    {
        let mut result = None;
        let mut f = Some(f);
        let found = self.with_read_typed_by_pk_raw(R::KIND, pk.as_ref(), &mut |data| {
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
    fn command_kind(&self) -> u8;
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
    command_kind: u8,
    ext_seq: u64,
    ref_ext_time_us: u64,
    payload: &'a [u8],
}

impl<'a> RuntimeCommandRef<'a> {
    /// Creates a borrowed runtime command envelope.
    #[inline]
    pub fn new(command_kind: u8, ext_seq: u64, ref_ext_time_us: u64, payload: &'a [u8]) -> Self {
        Self {
            command_kind,
            ext_seq,
            ref_ext_time_us,
            payload,
        }
    }
}

impl RuntimeCommandEnvelope for RuntimeCommandRef<'_> {
    #[inline(always)]
    fn command_kind(&self) -> u8 {
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
    fn command_kind(&self) -> u8 {
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
    fn with_read_raw<T>(
        &self,
        key: RecordKey,
        f: impl FnOnce(&[u8]) -> T,
    ) -> Result<Option<T>, Self::Error>;
    /// Iterates record keys for one record kind.
    fn for_each_record_key(&self, kind: RecordKind, f: &mut dyn FnMut(RecordKey));
}

/// Primary-key lookup capability for raw transaction contexts.
pub trait TxPkContext: TxReadContext {
    /// Resolves primary-key bytes to a system id.
    fn resolve_pk(&self, kind: RecordKind, pk: &[u8]) -> Result<Option<SysId>, Self::Error>;
}

/// Raw write capability for transaction contexts.
pub trait TxWriteContext: TxReadContext {
    /// Creates a record from encoded data bytes.
    fn create_raw(&mut self, kind: RecordKind, data: Vec<u8>) -> Result<RecordKey, Self::Error>;
    /// Updates a record by key.
    fn update_raw<T>(
        &mut self,
        key: RecordKey,
        f: impl FnOnce(&mut [u8]) -> T,
    ) -> Result<Option<T>, Self::Error>;
    /// Deletes a record by key.
    fn delete_raw(&mut self, key: RecordKey) -> Result<bool, Self::Error>;
    /// Emits an event payload.
    fn emit_event_raw(&mut self, event_kind: u8, payload: Vec<u8>);
    /// Emits host-side diagnostic text.
    fn debug_log(&mut self, _message: String) {}
}

/// Raw create capability that accepts a host-assigned system id.
pub trait TxSysIdCreateContext: TxWriteContext {
    /// Creates a record with an explicit system id.
    fn create_with_sys_id_raw(
        &mut self,
        kind: RecordKind,
        sys_id: SysId,
        data: Vec<u8>,
    ) -> Result<RecordKey, Self::Error>;
}

/// Full raw transaction context capability set.
pub trait TxContext: TxPkContext + TxSysIdCreateContext {}

impl<T: TxPkContext + TxSysIdCreateContext + ?Sized> TxContext for T {}

/// Typed transaction convenience API over generated StateVec accessors.
pub trait TypedTxContext {
    /// Host error type.
    type Error;

    /// Reads a generated record by system id.
    fn with_read_typed<R, T, F>(&self, sys_id: SysId, f: F) -> Result<Option<T>, Self::Error>
    where
        R: GeneratedRecordAccess,
        F: FnOnce(R::Access<'_>) -> T;

    /// Read a record by primary key without mutating it.
    ///
    /// Prefer [`TypedTxContext::update_typed_by_pk`] when the next step is to
    /// modify the same record. The update path keeps the operation in-place and
    /// avoids the extra resolve/read/then-update round-trip that this read-first
    /// pattern introduces on hot paths.
    fn with_read_typed_by_pk<R, P, T, F>(&self, pk: P, f: F) -> Result<Option<T>, Self::Error>
    where
        R: GeneratedRecordAccess,
        P: AsRef<[u8]>,
        F: FnOnce(R::Access<'_>) -> T;

    /// Creates a generated record.
    fn create_typed<R, F>(&mut self, init: F) -> Result<RecordKey, Self::Error>
    where
        R: GeneratedRecordAccess,
        F: for<'b> FnOnce(&mut R::NewBuilder<'b>);

    /// Update a record in-place by primary key.
    ///
    /// This is the preferred hot-path API when a command needs to modify an
    /// existing record. It avoids the read-then-update pattern that would
    /// otherwise resolve the primary key and touch the same record twice.
    fn update_typed_by_pk<R, P, T, F>(&mut self, pk: P, f: F) -> Result<Option<T>, Self::Error>
    where
        R: GeneratedRecordAccess,
        P: AsRef<[u8]>,
        F: for<'b> FnOnce(&mut R::UpdateBuilder<'b>) -> T;

    /// Updates an existing record by primary key or creates a new one.
    fn update_or_create_typed_by_pk<R, P, T, FU, FC>(
        &mut self,
        pk: P,
        update: FU,
        create: FC,
    ) -> Result<T, Self::Error>
    where
        R: GeneratedRecordAccess,
        P: AsRef<[u8]>,
        FU: for<'b> FnOnce(&mut R::UpdateBuilder<'b>) -> T,
        FC: for<'b> FnOnce(&mut R::NewBuilder<'b>) -> T,
    {
        if let Some(value) = self.update_typed_by_pk::<R, P, T, FU>(pk, update)? {
            return Ok(value);
        }
        let mut out = None;
        self.create_typed::<R, _>(|builder| {
            out = Some(create(builder));
        })?;
        Ok(out.expect("create closure must produce a value"))
    }

    /// Deletes a generated record by primary key.
    fn delete_by_pk<R, P>(&mut self, pk: P) -> Result<bool, Self::Error>
    where
        R: GeneratedRecordAccess,
        P: AsRef<[u8]>;

    /// Emits a generated event payload.
    fn emit_typed_event<E>(&mut self, payload: Vec<u8>)
    where
        E: GeneratedEventAccess;

    /// Iterates record keys for one record kind.
    fn for_each_record_key(&self, kind: RecordKind, f: &mut dyn FnMut(RecordKey));

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
        TxReadContext::with_read_raw(
            self,
            RecordKey {
                kind: R::KIND,
                sys_id,
            },
            |data| f(R::wrap(data)),
        )
    }

    fn with_read_typed_by_pk<R, P, T, F>(&self, pk: P, f: F) -> Result<Option<T>, Self::Error>
    where
        R: GeneratedRecordAccess,
        P: AsRef<[u8]>,
        F: FnOnce(R::Access<'_>) -> T,
    {
        on_typed_tx_with_read_typed_by_pk();
        let Some(sys_id) = TxPkContext::resolve_pk(self, R::KIND, pk.as_ref())? else {
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

    fn update_typed_by_pk<R, P, T, F>(&mut self, pk: P, f: F) -> Result<Option<T>, Self::Error>
    where
        R: GeneratedRecordAccess,
        P: AsRef<[u8]>,
        F: for<'b> FnOnce(&mut R::UpdateBuilder<'b>) -> T,
    {
        on_typed_tx_update_typed_by_pk();
        let Some(sys_id) = TxPkContext::resolve_pk(self, R::KIND, pk.as_ref())? else {
            return Ok(None);
        };
        TxWriteContext::update_raw(
            self,
            RecordKey {
                kind: R::KIND,
                sys_id,
            },
            |data| {
                let mut builder = R::wrap_update(data);
                f(&mut builder)
            },
        )
    }

    fn update_or_create_typed_by_pk<R, P, T, FU, FC>(
        &mut self,
        pk: P,
        update: FU,
        create: FC,
    ) -> Result<T, Self::Error>
    where
        R: GeneratedRecordAccess,
        P: AsRef<[u8]>,
        FU: for<'b> FnOnce(&mut R::UpdateBuilder<'b>) -> T,
        FC: for<'b> FnOnce(&mut R::NewBuilder<'b>) -> T,
    {
        on_typed_tx_update_or_create_typed_by_pk();
        if let Some(value) = self.update_typed_by_pk::<R, P, T, FU>(pk, update)? {
            return Ok(value);
        }

        let mut out = None;
        self.create_typed::<R, _>(|builder| {
            out = Some(create(builder));
        })?;
        Ok(out.expect("create closure must produce a value"))
    }

    fn delete_by_pk<R, P>(&mut self, pk: P) -> Result<bool, Self::Error>
    where
        R: GeneratedRecordAccess,
        P: AsRef<[u8]>,
    {
        let Some(sys_id) = TxPkContext::resolve_pk(self, R::KIND, pk.as_ref())? else {
            return Ok(false);
        };
        TxWriteContext::delete_raw(
            self,
            RecordKey {
                kind: R::KIND,
                sys_id,
            },
        )
    }

    fn emit_typed_event<E>(&mut self, payload: Vec<u8>)
    where
        E: GeneratedEventAccess,
    {
        TxWriteContext::emit_event_raw(self, E::KIND, payload);
    }

    fn for_each_record_key(&self, kind: RecordKind, f: &mut dyn FnMut(RecordKey)) {
        TxReadContext::for_each_record_key(self, kind, f);
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

    fn with_read_typed_by_pk<R, P, T, F>(&self, pk: P, f: F) -> Result<Option<T>, Self::Error>
    where
        R: GeneratedRecordAccess,
        P: AsRef<[u8]>,
        F: FnOnce(R::Access<'_>) -> T,
    {
        RuntimeHostContextExt::with_read_typed_by_pk::<R, P, T, F>(self, pk, f)
    }

    fn create_typed<R, F>(&mut self, init: F) -> Result<RecordKey, Self::Error>
    where
        R: GeneratedRecordAccess,
        F: for<'b> FnOnce(&mut R::NewBuilder<'b>),
    {
        RuntimeHostContextExt::create_typed::<R, F>(self, init)
    }

    fn update_typed_by_pk<R, P, T, F>(&mut self, pk: P, f: F) -> Result<Option<T>, Self::Error>
    where
        R: GeneratedRecordAccess,
        P: AsRef<[u8]>,
        F: for<'b> FnOnce(&mut R::UpdateBuilder<'b>) -> T,
    {
        RuntimeHostContextExt::update_typed_by_pk::<R, P, T, F>(self, pk, f)
    }

    fn delete_by_pk<R, P>(&mut self, pk: P) -> Result<bool, Self::Error>
    where
        R: GeneratedRecordAccess,
        P: AsRef<[u8]>,
    {
        RuntimeHostContextExt::delete_by_pk::<R, P>(self, pk)
    }

    fn emit_typed_event<E>(&mut self, payload: Vec<u8>)
    where
        E: GeneratedEventAccess,
    {
        RuntimeHostContextExt::emit_typed_event::<E>(self, payload)
            .expect("host emit_typed_event_raw failed");
    }

    fn for_each_record_key(&self, kind: RecordKind, f: &mut dyn FnMut(RecordKey)) {
        let _ = RuntimeHostContextExt::for_each_record_key(self, kind, f);
    }

    fn debug_log(&mut self, message: String) {
        let _ = RuntimeHostContext::debug_log(self, message);
    }
}

/// Minimal producer-side contract for a single globally ordered queue.
///
/// The producer appends one opaque record payload and receives the queue's
/// acceptance sequence (`ext_seq`) back. This is the `accepted` boundary.
pub trait QueueProducer {
    type Error;

    fn append(&mut self, record: &[u8]) -> Result<u64, Self::Error>;
}

/// Minimal consumer-side contract for a single globally ordered queue.
///
/// `poll()` yields records in source order. `commit_through(ext_seq)` advances
/// the durable consumed boundary only after downstream committed durability has
/// been established.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueRecord<T> {
    /// Queue-level source sequence.
    pub ext_seq: u64,
    /// Source-provided reference time in microseconds.
    pub ref_ext_time_us: u64,
    /// Opaque record payload.
    pub record: T,
}

/// Consumer-side contract for a single globally ordered queue.
pub trait QueueConsumer {
    /// Opaque record payload type.
    type Record;
    /// Queue error type.
    type Error;

    /// Polls the next source record if one is available.
    fn poll(&mut self) -> Result<Option<QueueRecord<Self::Record>>, Self::Error>;
    /// Advances the durable consumed boundary through `ext_seq`.
    fn commit_through(&mut self, ext_seq: u64) -> Result<(), Self::Error>;
}

/// Resume metadata query for queue consumers.
pub trait QueueConsumerResume {
    /// Queue error type.
    type Error;

    /// Returns the next source sequence to consume when known.
    fn resume_next_ext_seq(&mut self) -> Result<Option<u64>, Self::Error>;
}

/// Query surface for command outcomes keyed by queue sequence.
pub trait CommittedResultQuery {
    /// Query error type.
    type Error;

    /// Returns the committed status for an accepted source sequence.
    fn query_committed_by_ext_seq(
        &mut self,
        ext_seq: u64,
    ) -> Result<Option<CommittedStatus>, Self::Error>;
}

impl<F, E> CommittedResultQuery for F
where
    F: FnMut(u64) -> Result<Option<CommittedStatus>, E>,
{
    type Error = E;

    fn query_committed_by_ext_seq(
        &mut self,
        ext_seq: u64,
    ) -> Result<Option<CommittedStatus>, Self::Error> {
        self(ext_seq)
    }
}

/// Version byte used by submit request queue records.
pub const SUBMIT_REQUEST_RECORD_VERSION: u8 = 1;
/// Fixed header: version(1) + command_kind(1) + payload_len(4).
const SUBMIT_REQUEST_HEADER_LEN: usize = 6;
/// Offset to the payload_len field within the submit request header.
const SUBMIT_REQUEST_PAYLOAD_LEN_OFFSET: usize = 2;

/// Version byte used by committed result queue records.
pub const COMMITTED_RESULT_RECORD_VERSION: u8 = 2;
/// Total: version(1) + ext_seq(8) + status_tag(1) + status_value(8) = 18.
pub const COMMITTED_RESULT_RECORD_LEN: usize = 18;
const COMMITTED_RESULT_STATUS_TAG_OFFSET: usize = 9;
const COMMITTED_STATUS_TAG_COMMITTED: u8 = 1;
const COMMITTED_STATUS_TAG_REJECTED: u8 = 2;

/// Encoded submit request written to ingress queues.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmitRequest {
    /// Command kind.
    pub command_kind: u8,
    /// Encoded command payload.
    pub payload: Vec<u8>,
}

impl SubmitRequest {
    /// Creates a submit request.
    pub fn new(command_kind: u8, payload: Vec<u8>) -> Self {
        Self {
            command_kind,
            payload,
        }
    }
}

/// Receipt returned when a command is accepted by the queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueueReceipt {
    /// Queue-level acceptance identifier assigned when the command is accepted.
    pub ext_seq: u64,
}

/// Receipt returned when a command has a determined final outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommittedReceipt {
    /// Queue-level acceptance identifier for the command that reached a
    /// determined final outcome.
    pub ext_seq: u64,
    /// Determined final outcome keyed by `ext_seq`.
    pub status: CommittedStatus,
}

/// Fixed committed-result record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommittedResultRecord {
    /// Queue-level acceptance identifier for the command that reached a
    /// determined final outcome.
    pub ext_seq: u64,
    /// Determined final outcome published/queryable by `ext_seq`.
    pub status: CommittedStatus,
}

/// Domain/runtime rejection category encoded in committed result records.
///
/// Codes `100..=199` are reserved for runtime execution failures that are
/// still reported as determined command outcomes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum RejectedErrorCode {
    /// Domain plugin rejected the command deterministically.
    CommandRejected = 1,
    /// Runtime trapped a panic while executing the command.
    RuntimePanic = 103,
}

impl RejectedErrorCode {
    /// Encodes the error code as `u16`.
    pub fn to_u16(self) -> u16 {
        self as u16
    }

    /// Decodes the error code from `u16`.
    pub fn from_u16(value: u16) -> Option<Self> {
        match value {
            1 => Some(Self::CommandRejected),
            103 => Some(Self::RuntimePanic),
            _ => None,
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

/// Error returned when queue codec records cannot be encoded or decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueueCodecError {
    /// Input ended before all required fields were present.
    Truncated,
    /// Input had extra bytes after the expected record length.
    TrailingBytes { expected: usize, actual: usize },
    /// A field length does not fit the codec.
    FieldTooLarge { field: &'static str, len: u64 },
    /// Record version does not match this codec.
    UnsupportedVersion { expected: u8, found: u8 },
    /// Field value is not in the valid domain.
    InvalidFieldValue { field: &'static str, value: u64 },
}

impl std::fmt::Display for QueueCodecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Truncated => write!(f, "truncated queue record"),
            Self::TrailingBytes { expected, actual } => {
                write!(
                    f,
                    "queue record has trailing bytes: expected {expected}, actual {actual}"
                )
            }
            Self::FieldTooLarge { field, len } => {
                write!(f, "queue record field '{field}' too large: {len} bytes")
            }
            Self::UnsupportedVersion { expected, found } => {
                write!(
                    f,
                    "unsupported queue record version: expected {expected}, found {found}"
                )
            }
            Self::InvalidFieldValue { field, value } => {
                write!(f, "invalid value for queue record field '{field}': {value}")
            }
        }
    }
}

impl std::error::Error for QueueCodecError {}

/// Encodes a submit request queue record.
pub fn encode_submit_request(request: &SubmitRequest) -> Result<Vec<u8>, QueueCodecError> {
    let payload_len =
        u32::try_from(request.payload.len()).map_err(|_| QueueCodecError::FieldTooLarge {
            field: "payload",
            len: request.payload.len() as u64,
        })?;
    let mut out = Vec::with_capacity(SUBMIT_REQUEST_HEADER_LEN + request.payload.len());
    out.push(SUBMIT_REQUEST_RECORD_VERSION);
    out.push(request.command_kind);
    out.extend_from_slice(&payload_len.to_le_bytes());
    out.extend_from_slice(&request.payload);
    Ok(out)
}

/// Decodes a submit request queue record.
pub fn decode_submit_request(bytes: &[u8]) -> Result<SubmitRequest, QueueCodecError> {
    if bytes.len() < SUBMIT_REQUEST_HEADER_LEN {
        return Err(QueueCodecError::Truncated);
    }
    let version = bytes[0];
    if version != SUBMIT_REQUEST_RECORD_VERSION {
        return Err(QueueCodecError::UnsupportedVersion {
            expected: SUBMIT_REQUEST_RECORD_VERSION,
            found: version,
        });
    }
    let command_kind = bytes[1];
    let payload_len = u32::from_le_bytes(
        bytes[SUBMIT_REQUEST_PAYLOAD_LEN_OFFSET..SUBMIT_REQUEST_PAYLOAD_LEN_OFFSET + 4]
            .try_into()
            .map_err(|_| QueueCodecError::Truncated)?,
    ) as usize;
    let payload_off = SUBMIT_REQUEST_HEADER_LEN;
    let expected = payload_off + payload_len;
    if bytes.len() < expected {
        return Err(QueueCodecError::Truncated);
    }
    if bytes.len() != expected {
        return Err(QueueCodecError::TrailingBytes {
            expected,
            actual: bytes.len(),
        });
    }
    Ok(SubmitRequest {
        command_kind,
        payload: bytes[payload_off..expected].to_vec(),
    })
}

/// Encodes a committed-result record into its fixed-width representation.
pub fn encode_committed_result_record_fixed(
    record: CommittedResultRecord,
) -> [u8; COMMITTED_RESULT_RECORD_LEN] {
    let mut out = [0u8; COMMITTED_RESULT_RECORD_LEN];
    out[0] = COMMITTED_RESULT_RECORD_VERSION;
    out[1..9].copy_from_slice(&record.ext_seq.to_le_bytes());
    match record.status {
        CommittedStatus::Committed { tx_seq } => {
            out[COMMITTED_RESULT_STATUS_TAG_OFFSET] = COMMITTED_STATUS_TAG_COMMITTED;
            out[10..18].copy_from_slice(&tx_seq.to_le_bytes());
        }
        CommittedStatus::Rejected { error_code } => {
            out[COMMITTED_RESULT_STATUS_TAG_OFFSET] = COMMITTED_STATUS_TAG_REJECTED;
            out[10..18].copy_from_slice(&(error_code.to_u16() as u64).to_le_bytes());
        }
    }
    out
}

/// Encodes a committed-result record.
pub fn encode_committed_result_record(record: CommittedResultRecord) -> Vec<u8> {
    encode_committed_result_record_fixed(record).to_vec()
}

/// Decodes a committed-result record.
pub fn decode_committed_result_record(
    bytes: &[u8],
) -> Result<CommittedResultRecord, QueueCodecError> {
    if bytes.len() < COMMITTED_RESULT_RECORD_LEN {
        return Err(QueueCodecError::Truncated);
    }
    let version = bytes[0];
    if version != COMMITTED_RESULT_RECORD_VERSION {
        return Err(QueueCodecError::UnsupportedVersion {
            expected: COMMITTED_RESULT_RECORD_VERSION,
            found: version,
        });
    }
    if bytes.len() != COMMITTED_RESULT_RECORD_LEN {
        return Err(QueueCodecError::TrailingBytes {
            expected: COMMITTED_RESULT_RECORD_LEN,
            actual: bytes.len(),
        });
    }
    let status_tag = bytes[COMMITTED_RESULT_STATUS_TAG_OFFSET];
    let status_value = u64::from_le_bytes(
        bytes[10..18]
            .try_into()
            .map_err(|_| QueueCodecError::Truncated)?,
    );
    let status = match status_tag {
        COMMITTED_STATUS_TAG_COMMITTED => CommittedStatus::Committed {
            tx_seq: status_value,
        },
        COMMITTED_STATUS_TAG_REJECTED => {
            let code_u16 =
                u16::try_from(status_value).map_err(|_| QueueCodecError::InvalidFieldValue {
                    field: "error_code",
                    value: status_value,
                })?;
            let error_code = RejectedErrorCode::from_u16(code_u16).ok_or(
                QueueCodecError::InvalidFieldValue {
                    field: "error_code",
                    value: status_value,
                },
            )?;
            CommittedStatus::Rejected { error_code }
        }
        other => {
            return Err(QueueCodecError::InvalidFieldValue {
                field: "status_tag",
                value: other as u64,
            });
        }
    };
    Ok(CommittedResultRecord {
        ext_seq: u64::from_le_bytes(
            bytes[1..9]
                .try_into()
                .map_err(|_| QueueCodecError::Truncated)?,
        ),
        status,
    })
}
