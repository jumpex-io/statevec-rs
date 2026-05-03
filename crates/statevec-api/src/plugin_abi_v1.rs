// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

use std::ffi::c_void;

use crate::{
    BizInvariantReadContext, RecordKey, RecordKind, RuntimeCommandRef, RuntimeHostContext,
    RuntimeHostError, RuntimePlugin, RuntimePluginFactory, SysId,
};

/// Status returned by FFI ABI calls.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeCallStatus {
    /// Call completed successfully.
    Success = 0,
    /// Call failed and may have written an error buffer.
    Failure = 1,
}

/// Phase in which an ABI error occurred.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeErrorPhase {
    /// Plugin metadata or schema loading.
    Load = 1,
    /// Runtime plugin instance creation.
    Create = 2,
    /// Transaction execution.
    RunTx = 3,
    /// Business invariant validation.
    ValidateBizInvariants = 4,
    /// Plugin unload.
    Unload = 5,
}

/// Stable numeric runtime error kind.
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RuntimeErrorKind(pub u16);

/// Well-known runtime error kinds used by ABI helpers.
pub mod runtime_error_kind {
    use super::RuntimeErrorKind;

    /// ABI precondition violation.
    pub const ABI_CONTRACT_VIOLATION: RuntimeErrorKind = RuntimeErrorKind(1);

    /// Loaded plugin schema did not match host expectations.
    pub const SCHEMA_MISMATCH: RuntimeErrorKind = RuntimeErrorKind(1001);
    /// Plugin or host configuration was invalid.
    pub const CONFIG_ERROR: RuntimeErrorKind = RuntimeErrorKind(1002);

    /// Plugin rejected the command.
    pub const PLUGIN_REJECTED: RuntimeErrorKind = RuntimeErrorKind(2001);
    /// Command envelope or payload was invalid.
    pub const INVALID_COMMAND: RuntimeErrorKind = RuntimeErrorKind(2002);

    /// Host rejected the operation.
    pub const HOST_REJECTED: RuntimeErrorKind = RuntimeErrorKind(4001);
    /// Host encountered an internal error.
    pub const HOST_INTERNAL_ERROR: RuntimeErrorKind = RuntimeErrorKind(4002);

    /// Plugin encountered an internal error.
    pub const PLUGIN_INTERNAL_ERROR: RuntimeErrorKind = RuntimeErrorKind(5001);
}

/// Borrowed immutable byte slice passed across the C ABI.
///
/// This is a view into memory owned by the caller. The callee must not retain
/// the pointer or use it after the ABI call or visitor callback returns.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeBytesRef {
    /// Pointer to the first byte, or null when `len == 0`.
    pub ptr: *const u8,
    /// Number of bytes available at `ptr`.
    pub len: usize,
}

impl RuntimeBytesRef {
    /// Returns an empty byte reference.
    pub const fn empty() -> Self {
        Self {
            ptr: std::ptr::null(),
            len: 0,
        }
    }

    /// Creates a byte reference from a Rust slice.
    pub fn from_slice(bytes: &[u8]) -> Self {
        Self {
            ptr: bytes.as_ptr(),
            len: bytes.len(),
        }
    }
}

/// Borrowed mutable byte slice passed across the C ABI.
///
/// This is a uniquely borrowed view into memory owned by the caller. The callee
/// may mutate the bytes only during the ABI call or visitor callback and must
/// not retain the pointer after it returns.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeBytesMutRef {
    /// Pointer to the first byte, or null when `len == 0`.
    pub ptr: *mut u8,
    /// Number of bytes available at `ptr`.
    pub len: usize,
}

impl RuntimeBytesMutRef {
    /// Returns an empty mutable byte reference.
    pub const fn empty() -> Self {
        Self {
            ptr: std::ptr::null_mut(),
            len: 0,
        }
    }

    /// Creates a mutable byte reference from a Rust slice.
    pub fn from_slice(bytes: &mut [u8]) -> Self {
        Self {
            ptr: bytes.as_mut_ptr(),
            len: bytes.len(),
        }
    }
}

/// Fixed-size ABI error buffer.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeErrorBuf {
    /// Error phase.
    pub phase: RuntimeErrorPhase,
    /// Error kind.
    pub kind: RuntimeErrorKind,
    /// Number of valid bytes in `message_buf`.
    pub message_len: usize,
    /// UTF-8 message bytes truncated to `RUNTIME_ERROR_MESSAGE_CAPACITY`.
    pub message_buf: [u8; RUNTIME_ERROR_MESSAGE_CAPACITY],
}

impl RuntimeErrorBuf {
    /// Creates an empty error buffer with phase and kind metadata.
    pub const fn new(
        phase: RuntimeErrorPhase,
        kind: RuntimeErrorKind,
        _message: RuntimeBytesRef,
    ) -> Self {
        Self {
            phase,
            kind,
            message_len: 0,
            message_buf: [0; RUNTIME_ERROR_MESSAGE_CAPACITY],
        }
    }
}

/// Maximum ABI error message bytes.
pub const RUNTIME_ERROR_MESSAGE_CAPACITY: usize = 512;

/// ABI view of a command envelope.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeCommandView {
    /// Command kind.
    pub command_kind: u8,
    /// Source queue sequence.
    pub ext_seq: u64,
    /// Source-provided reference time in microseconds.
    pub ref_ext_time_us: u64,
    /// Encoded command payload.
    pub payload: RuntimeBytesRef,
}

/// ABI view of a record key.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeRecordKeyView {
    /// Stable record kind.
    pub kind: RecordKind,
    /// Host-assigned system id.
    pub sys_id: SysId,
}

impl From<RecordKey> for RuntimeRecordKeyView {
    fn from(value: RecordKey) -> Self {
        Self {
            kind: value.kind,
            sys_id: value.sys_id,
        }
    }
}

impl From<RuntimeRecordKeyView> for RecordKey {
    fn from(value: RuntimeRecordKeyView) -> Self {
        Self {
            kind: value.kind,
            sys_id: value.sys_id,
        }
    }
}

/// Visitor callback for borrowed byte slices.
pub type RuntimeBytesVisitor =
    unsafe extern "C" fn(visitor_ctx: *mut c_void, bytes: RuntimeBytesRef);
/// Visitor callback for uniquely borrowed mutable byte slices.
pub type RuntimeBytesMutVisitor =
    unsafe extern "C" fn(visitor_ctx: *mut c_void, bytes: RuntimeBytesMutRef);
/// Visitor callback for record keys.
pub type RuntimeRecordKeyVisitor =
    unsafe extern "C" fn(visitor_ctx: *mut c_void, key: RuntimeRecordKeyView);

/// ABI handle for mutable runtime host access.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct RuntimeHostContextV1 {
    /// Host context pointer.
    pub ctx_ptr: *mut c_void,
    /// Host vtable pointer.
    pub vtable: *const RuntimeHostVTableV1,
}

/// ABI handle for read-only runtime host access.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct RuntimeReadContextV1 {
    /// Host context pointer.
    pub ctx_ptr: *mut c_void,
    /// Read-only host vtable pointer.
    pub vtable: *const RuntimeReadVTableV1,
}

/// Mutable host vtable exposed to runtime plugins.
#[repr(C)]
pub struct RuntimeHostVTableV1 {
    /// Read by system id.
    pub with_read_typed_raw: unsafe extern "C" fn(
        ctx_ptr: *const c_void,
        record_kind: RecordKind,
        sys_id: SysId,
        visitor_ctx: *mut c_void,
        visitor: RuntimeBytesVisitor,
        out_found: *mut bool,
        out_error: *mut RuntimeErrorBuf,
    ) -> RuntimeCallStatus,
    /// Read by primary key.
    pub with_read_typed_by_pk_raw: unsafe extern "C" fn(
        ctx_ptr: *const c_void,
        record_kind: RecordKind,
        pk: RuntimeBytesRef,
        visitor_ctx: *mut c_void,
        visitor: RuntimeBytesVisitor,
        out_found: *mut bool,
        out_error: *mut RuntimeErrorBuf,
    ) -> RuntimeCallStatus,
    /// Create a record.
    pub create_typed_raw: unsafe extern "C" fn(
        ctx_ptr: *mut c_void,
        record_kind: RecordKind,
        init_ctx: *mut c_void,
        init: RuntimeBytesMutVisitor,
        out_key: *mut RuntimeRecordKeyView,
        out_error: *mut RuntimeErrorBuf,
    ) -> RuntimeCallStatus,
    /// Update a record by primary key.
    pub update_typed_by_pk_raw: unsafe extern "C" fn(
        ctx_ptr: *mut c_void,
        record_kind: RecordKind,
        pk: RuntimeBytesRef,
        update_ctx: *mut c_void,
        update: RuntimeBytesMutVisitor,
        out_found: *mut bool,
        out_error: *mut RuntimeErrorBuf,
    ) -> RuntimeCallStatus,
    /// Delete a record by primary key.
    pub delete_by_pk_raw: unsafe extern "C" fn(
        ctx_ptr: *mut c_void,
        record_kind: RecordKind,
        pk: RuntimeBytesRef,
        out_deleted: *mut bool,
        out_error: *mut RuntimeErrorBuf,
    ) -> RuntimeCallStatus,
    /// Emit an event.
    pub emit_typed_event_raw: unsafe extern "C" fn(
        ctx_ptr: *mut c_void,
        event_kind: u8,
        payload: RuntimeBytesRef,
        out_error: *mut RuntimeErrorBuf,
    ) -> RuntimeCallStatus,
    /// Iterate record keys for one kind.
    pub for_each_record_key_raw: unsafe extern "C" fn(
        ctx_ptr: *const c_void,
        kind: RecordKind,
        visitor_ctx: *mut c_void,
        visitor: RuntimeRecordKeyVisitor,
        out_error: *mut RuntimeErrorBuf,
    ) -> RuntimeCallStatus,
    /// Emit host-side diagnostic text.
    pub debug_log: unsafe extern "C" fn(
        ctx_ptr: *mut c_void,
        message: RuntimeBytesRef,
        out_error: *mut RuntimeErrorBuf,
    ) -> RuntimeCallStatus,
}

/// Read-only host vtable exposed to invariant validation.
#[repr(C)]
pub struct RuntimeReadVTableV1 {
    /// Read by system id.
    pub with_read_typed_raw: unsafe extern "C" fn(
        ctx_ptr: *const c_void,
        record_kind: RecordKind,
        sys_id: SysId,
        visitor_ctx: *mut c_void,
        visitor: RuntimeBytesVisitor,
        out_found: *mut bool,
        out_error: *mut RuntimeErrorBuf,
    ) -> RuntimeCallStatus,
    /// Read by primary key.
    pub with_read_typed_by_pk_raw: unsafe extern "C" fn(
        ctx_ptr: *const c_void,
        record_kind: RecordKind,
        pk: RuntimeBytesRef,
        visitor_ctx: *mut c_void,
        visitor: RuntimeBytesVisitor,
        out_found: *mut bool,
        out_error: *mut RuntimeErrorBuf,
    ) -> RuntimeCallStatus,
    /// Iterate record keys for one kind.
    pub for_each_record_key_raw: unsafe extern "C" fn(
        ctx_ptr: *const c_void,
        kind: RecordKind,
        visitor_ctx: *mut c_void,
        visitor: RuntimeRecordKeyVisitor,
        out_error: *mut RuntimeErrorBuf,
    ) -> RuntimeCallStatus,
}

/// Current stable runtime plugin ABI version.
pub const RUNTIME_PLUGIN_ABI_VERSION_V1: u32 = 1;
/// Null-terminated symbol name exported by StateVec runtime plugins.
pub const RUNTIME_PLUGIN_ENTRY_V1_SYMBOL: &[u8] = b"statevec_runtime_plugin_entry_v1\0";

/// ABI function table returned by a runtime plugin entrypoint.
#[repr(C)]
pub struct RuntimePluginApiV1 {
    /// ABI version implemented by the plugin.
    pub abi_version: u32,
    /// Returns the plugin name.
    pub plugin_name: unsafe extern "C" fn() -> RuntimeBytesRef,
    /// Returns schema IDL JSON bytes.
    pub schema_bytes: unsafe extern "C" fn() -> RuntimeBytesRef,
    /// Creates a configured runtime plugin instance.
    pub create_runtime: unsafe extern "C" fn(
        config: RuntimeBytesRef,
        out_runtime: *mut *mut c_void,
        out_error: *mut RuntimeErrorBuf,
    ) -> RuntimeCallStatus,
    /// Destroys a runtime plugin instance.
    pub destroy_runtime: unsafe extern "C" fn(runtime: *mut c_void),
    /// Runs one command transaction.
    pub run_tx: unsafe extern "C" fn(
        runtime: *mut c_void,
        host: RuntimeHostContextV1,
        command: RuntimeCommandView,
        out_error: *mut RuntimeErrorBuf,
    ) -> RuntimeCallStatus,
    /// Validates business invariants against read-only host state.
    pub validate_biz_invariants: unsafe extern "C" fn(
        runtime: *mut c_void,
        host: RuntimeReadContextV1,
        out_error: *mut RuntimeErrorBuf,
    ) -> RuntimeCallStatus,
    /// Unloads a runtime plugin instance.
    pub on_unload: unsafe extern "C" fn(
        runtime: *mut c_void,
        out_error: *mut RuntimeErrorBuf,
    ) -> RuntimeCallStatus,
}

/// Runtime plugin entrypoint function type.
pub type RuntimePluginEntryV1 = unsafe extern "C" fn() -> RuntimePluginApiV1;

/// Boxed plugin handle owned across the ABI boundary.
pub struct ExportedRuntimePluginV1Handle {
    /// Plugin instance.
    pub plugin: Box<dyn RuntimePlugin>,
}

/// Converts an ABI byte reference into a Rust slice.
///
/// # Safety
///
/// The caller must ensure `bytes.ptr` is valid for `bytes.len` bytes for the
/// returned lifetime.
pub unsafe fn runtime_bytes_slice<'a>(bytes: RuntimeBytesRef) -> Result<&'a [u8], &'static str> {
    if bytes.ptr.is_null() {
        if bytes.len == 0 {
            return Ok(&[]);
        }
        return Err("bytes pointer is null but len is non-zero");
    }
    Ok(unsafe { std::slice::from_raw_parts(bytes.ptr, bytes.len) })
}

/// Converts an ABI mutable byte reference into a mutable Rust slice.
///
/// # Safety
///
/// The caller must ensure `bytes.ptr` is valid and uniquely borrowed for
/// `bytes.len` bytes for the returned lifetime.
pub unsafe fn runtime_bytes_slice_mut<'a>(
    bytes: RuntimeBytesMutRef,
) -> Result<&'a mut [u8], &'static str> {
    if bytes.ptr.is_null() {
        if bytes.len == 0 {
            return Ok(&mut []);
        }
        return Err("bytes pointer is null but len is non-zero");
    }
    Ok(unsafe { std::slice::from_raw_parts_mut(bytes.ptr, bytes.len) })
}

/// Resets an optional ABI error buffer.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub fn clear_runtime_error(out_error: *mut RuntimeErrorBuf) {
    if out_error.is_null() {
        return;
    }
    unsafe {
        *out_error = RuntimeErrorBuf::new(
            RuntimeErrorPhase::Load,
            runtime_error_kind::HOST_INTERNAL_ERROR,
            RuntimeBytesRef::empty(),
        );
    }
}

/// Writes an error into an optional ABI error buffer.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub fn write_runtime_error(
    out_error: *mut RuntimeErrorBuf,
    phase: RuntimeErrorPhase,
    kind: RuntimeErrorKind,
    message: &str,
) {
    if out_error.is_null() {
        return;
    }
    let mut error = RuntimeErrorBuf::new(phase, kind, RuntimeBytesRef::empty());
    let mut message_len = message.len().min(RUNTIME_ERROR_MESSAGE_CAPACITY);
    while !message.is_char_boundary(message_len) {
        message_len -= 1;
    }
    error.message_len = message_len;
    error.message_buf[..message_len].copy_from_slice(&message.as_bytes()[..message_len]);
    unsafe {
        *out_error = error;
    }
}

/// Returns the error text from an ABI error buffer.
pub fn runtime_error_text(error: &RuntimeErrorBuf) -> String {
    let message_len = error.message_len.min(RUNTIME_ERROR_MESSAGE_CAPACITY);
    String::from_utf8_lossy(&error.message_buf[..message_len]).into_owned()
}

/// Formats an ABI error buffer for diagnostics.
pub fn runtime_error_message(error: &RuntimeErrorBuf) -> String {
    format!(
        "phase={:?} kind={} message={}",
        error.phase,
        error.kind.0,
        runtime_error_text(error)
    )
}

/// ABI helper returning a plugin factory name.
pub fn runtime_plugin_name_v1(factory: fn() -> Box<dyn RuntimePluginFactory>) -> RuntimeBytesRef {
    let name = factory().plugin_name();
    RuntimeBytesRef::from_slice(name.as_bytes())
}

/// ABI helper returning cached schema IDL JSON bytes.
pub fn runtime_plugin_schema_bytes_v1(
    factory: fn() -> Box<dyn RuntimePluginFactory>,
    cache: &'static std::sync::OnceLock<String>,
) -> RuntimeBytesRef {
    let bytes = cache.get_or_init(|| factory().schema_registry().to_idl_json());
    RuntimeBytesRef::from_slice(bytes.as_bytes())
}

unsafe fn exported_runtime_plugin_handle_mut(
    runtime: *mut c_void,
) -> Result<&'static mut ExportedRuntimePluginV1Handle, &'static str> {
    if runtime.is_null() {
        Err("runtime handle is null")
    } else {
        Ok(unsafe { &mut *runtime.cast::<ExportedRuntimePluginV1Handle>() })
    }
}

/// Creates a runtime plugin instance through the v1 ABI helper.
///
/// # Safety
///
/// `out_runtime` and `out_error`, when non-null, must be valid for writes for
/// the duration of the call. `config` must point to bytes that remain valid for
/// the duration of the call.
pub unsafe fn runtime_plugin_create_runtime_v1(
    factory: fn() -> Box<dyn RuntimePluginFactory>,
    config: RuntimeBytesRef,
    out_runtime: *mut *mut c_void,
    out_error: *mut RuntimeErrorBuf,
) -> RuntimeCallStatus {
    clear_runtime_error(out_error);

    if out_runtime.is_null() {
        write_runtime_error(
            out_error,
            RuntimeErrorPhase::Create,
            runtime_error_kind::ABI_CONTRACT_VIOLATION,
            "out_runtime is null",
        );
        return RuntimeCallStatus::Failure;
    }

    let config_bytes = match unsafe { runtime_bytes_slice(config) } {
        Ok(bytes) => bytes,
        Err(message) => {
            write_runtime_error(
                out_error,
                RuntimeErrorPhase::Create,
                runtime_error_kind::ABI_CONTRACT_VIOLATION,
                message,
            );
            return RuntimeCallStatus::Failure;
        }
    };

    let config_text = match std::str::from_utf8(config_bytes) {
        Ok(value) => value,
        Err(err) => {
            write_runtime_error(
                out_error,
                RuntimeErrorPhase::Create,
                runtime_error_kind::CONFIG_ERROR,
                &format!("plugin config is not valid UTF-8: {err}"),
            );
            return RuntimeCallStatus::Failure;
        }
    };

    let plugin = match factory().create(config_text) {
        Ok(plugin) => plugin,
        Err(err) => {
            write_runtime_error(
                out_error,
                RuntimeErrorPhase::Create,
                runtime_error_kind::CONFIG_ERROR,
                &err.to_string(),
            );
            return RuntimeCallStatus::Failure;
        }
    };

    let handle = Box::new(ExportedRuntimePluginV1Handle { plugin });
    unsafe {
        *out_runtime = Box::into_raw(handle).cast();
    }
    RuntimeCallStatus::Success
}

/// Destroys a runtime plugin instance created by
/// [`runtime_plugin_create_runtime_v1`].
///
/// # Safety
///
/// `runtime` must be null or a handle previously returned by
/// [`runtime_plugin_create_runtime_v1`] and not already destroyed.
pub unsafe fn runtime_plugin_destroy_runtime_v1(runtime: *mut c_void) {
    if runtime.is_null() {
        return;
    }
    unsafe {
        drop(Box::from_raw(
            runtime.cast::<ExportedRuntimePluginV1Handle>(),
        ));
    }
}

/// Runs one command transaction through the v1 ABI helper.
///
/// # Safety
///
/// `runtime` must be a live plugin handle, `host` must contain a valid host
/// context and vtable for the duration of the call, and `command.payload` and
/// `out_error` must remain valid for the duration of the call.
pub unsafe fn runtime_plugin_run_tx_v1(
    runtime: *mut c_void,
    host: RuntimeHostContextV1,
    command: RuntimeCommandView,
    out_error: *mut RuntimeErrorBuf,
) -> RuntimeCallStatus {
    clear_runtime_error(out_error);

    let payload = match unsafe { runtime_bytes_slice(command.payload) } {
        Ok(bytes) => bytes,
        Err(message) => {
            write_runtime_error(
                out_error,
                RuntimeErrorPhase::RunTx,
                runtime_error_kind::ABI_CONTRACT_VIOLATION,
                message,
            );
            return RuntimeCallStatus::Failure;
        }
    };

    let handle = match unsafe { exported_runtime_plugin_handle_mut(runtime) } {
        Ok(handle) => handle,
        Err(message) => {
            write_runtime_error(
                out_error,
                RuntimeErrorPhase::RunTx,
                runtime_error_kind::ABI_CONTRACT_VIOLATION,
                message,
            );
            return RuntimeCallStatus::Failure;
        }
    };

    let mut host_adapter = RuntimeHostContextV1Adapter::new(host);
    let command = RuntimeCommandRef::new(
        command.command_kind,
        command.ext_seq,
        command.ref_ext_time_us,
        payload,
    );
    match handle.plugin.run_tx(&mut host_adapter, &command) {
        Ok(()) => RuntimeCallStatus::Success,
        Err(err) => {
            write_runtime_error(
                out_error,
                RuntimeErrorPhase::RunTx,
                runtime_error_kind::PLUGIN_REJECTED,
                &err.to_string(),
            );
            RuntimeCallStatus::Failure
        }
    }
}

/// Validates plugin business invariants through the v1 ABI helper.
///
/// # Safety
///
/// `runtime` must be a live plugin handle, `host` must contain a valid
/// read-only host context and vtable for the duration of the call, and
/// `out_error` must be null or valid for writes.
pub unsafe fn runtime_plugin_validate_biz_invariants_v1(
    runtime: *mut c_void,
    host: RuntimeReadContextV1,
    out_error: *mut RuntimeErrorBuf,
) -> RuntimeCallStatus {
    clear_runtime_error(out_error);

    let handle = match unsafe { exported_runtime_plugin_handle_mut(runtime) } {
        Ok(handle) => handle,
        Err(message) => {
            write_runtime_error(
                out_error,
                RuntimeErrorPhase::ValidateBizInvariants,
                runtime_error_kind::ABI_CONTRACT_VIOLATION,
                message,
            );
            return RuntimeCallStatus::Failure;
        }
    };

    let host_adapter = RuntimeReadContextV1Adapter::new(host);
    match handle.plugin.validate_biz_invariants(&host_adapter) {
        Ok(()) => RuntimeCallStatus::Success,
        Err(err) => {
            write_runtime_error(
                out_error,
                RuntimeErrorPhase::ValidateBizInvariants,
                runtime_error_kind::PLUGIN_REJECTED,
                &err,
            );
            RuntimeCallStatus::Failure
        }
    }
}

/// Calls the plugin unload hook through the v1 ABI helper.
///
/// # Safety
///
/// `runtime` must be a live plugin handle and `out_error` must be null or valid
/// for writes.
pub unsafe fn runtime_plugin_on_unload_v1(
    runtime: *mut c_void,
    out_error: *mut RuntimeErrorBuf,
) -> RuntimeCallStatus {
    clear_runtime_error(out_error);

    let handle = match unsafe { exported_runtime_plugin_handle_mut(runtime) } {
        Ok(handle) => handle,
        Err(message) => {
            write_runtime_error(
                out_error,
                RuntimeErrorPhase::Unload,
                runtime_error_kind::ABI_CONTRACT_VIOLATION,
                message,
            );
            return RuntimeCallStatus::Failure;
        }
    };

    match handle.plugin.on_unload() {
        Ok(()) => RuntimeCallStatus::Success,
        Err(err) => {
            write_runtime_error(
                out_error,
                RuntimeErrorPhase::Unload,
                runtime_error_kind::PLUGIN_INTERNAL_ERROR,
                &err.to_string(),
            );
            RuntimeCallStatus::Failure
        }
    }
}

struct BytesVisitorForward<'a> {
    callback: &'a mut dyn FnMut(&[u8]),
}

struct BytesVisitorForwardMut<'a> {
    callback: &'a mut dyn FnMut(&mut [u8]),
}

struct RecordKeyVisitorForward<'a> {
    callback: &'a mut dyn FnMut(RecordKey),
}

unsafe extern "C" fn forward_bytes_visitor(visitor_ctx: *mut c_void, bytes: RuntimeBytesRef) {
    let forward = unsafe { &mut *visitor_ctx.cast::<BytesVisitorForward<'_>>() };
    let bytes = unsafe { runtime_bytes_slice(bytes) }.expect("host returned invalid bytes");
    (forward.callback)(bytes);
}

unsafe extern "C" fn forward_bytes_visitor_mut(
    visitor_ctx: *mut c_void,
    bytes: RuntimeBytesMutRef,
) {
    let forward = unsafe { &mut *visitor_ctx.cast::<BytesVisitorForwardMut<'_>>() };
    let bytes =
        unsafe { runtime_bytes_slice_mut(bytes) }.expect("host returned invalid mutable bytes");
    (forward.callback)(bytes);
}

unsafe extern "C" fn forward_record_key_visitor(
    visitor_ctx: *mut c_void,
    key: RuntimeRecordKeyView,
) {
    let forward = unsafe { &mut *visitor_ctx.cast::<RecordKeyVisitorForward<'_>>() };
    (forward.callback)(key.into());
}

#[derive(Debug, Clone, Copy)]
pub struct RuntimeHostContextV1Adapter {
    raw: RuntimeHostContextV1,
}

impl RuntimeHostContextV1Adapter {
    pub const fn new(raw: RuntimeHostContextV1) -> Self {
        Self { raw }
    }

    fn vtable(&self) -> Result<&RuntimeHostVTableV1, RuntimeHostError> {
        unsafe { self.raw.vtable.as_ref() }
            .ok_or_else(|| RuntimeHostError::new("runtime host vtable is null"))
    }
}

impl RuntimeHostContext for RuntimeHostContextV1Adapter {
    fn with_read_typed_raw(
        &self,
        record_kind: RecordKind,
        sys_id: SysId,
        f: &mut dyn FnMut(&[u8]),
    ) -> Result<bool, RuntimeHostError> {
        let mut found = false;
        let mut error = RuntimeErrorBuf::new(
            RuntimeErrorPhase::RunTx,
            runtime_error_kind::HOST_INTERNAL_ERROR,
            RuntimeBytesRef::empty(),
        );
        let mut visitor = BytesVisitorForward { callback: f };
        let status = unsafe {
            (self.vtable()?.with_read_typed_raw)(
                self.raw.ctx_ptr.cast_const(),
                record_kind,
                sys_id,
                (&mut visitor as *mut BytesVisitorForward<'_>).cast(),
                forward_bytes_visitor,
                &mut found,
                &mut error,
            )
        };
        match status {
            RuntimeCallStatus::Success => Ok(found),
            RuntimeCallStatus::Failure => Err(RuntimeHostError::new(runtime_error_message(&error))),
        }
    }

    fn with_read_typed_by_pk_raw(
        &self,
        record_kind: RecordKind,
        pk: &[u8],
        f: &mut dyn FnMut(&[u8]),
    ) -> Result<bool, RuntimeHostError> {
        let mut found = false;
        let mut error = RuntimeErrorBuf::new(
            RuntimeErrorPhase::RunTx,
            runtime_error_kind::HOST_INTERNAL_ERROR,
            RuntimeBytesRef::empty(),
        );
        let mut visitor = BytesVisitorForward { callback: f };
        let status = unsafe {
            (self.vtable()?.with_read_typed_by_pk_raw)(
                self.raw.ctx_ptr.cast_const(),
                record_kind,
                RuntimeBytesRef::from_slice(pk),
                (&mut visitor as *mut BytesVisitorForward<'_>).cast(),
                forward_bytes_visitor,
                &mut found,
                &mut error,
            )
        };
        match status {
            RuntimeCallStatus::Success => Ok(found),
            RuntimeCallStatus::Failure => Err(RuntimeHostError::new(runtime_error_message(&error))),
        }
    }

    fn create_typed_raw(
        &mut self,
        record_kind: RecordKind,
        init: &mut dyn FnMut(&mut [u8]),
    ) -> Result<RecordKey, RuntimeHostError> {
        let mut key = RuntimeRecordKeyView { kind: 0, sys_id: 0 };
        let mut error = RuntimeErrorBuf::new(
            RuntimeErrorPhase::RunTx,
            runtime_error_kind::HOST_INTERNAL_ERROR,
            RuntimeBytesRef::empty(),
        );
        let mut visitor = BytesVisitorForwardMut { callback: init };
        let status = unsafe {
            (self.vtable()?.create_typed_raw)(
                self.raw.ctx_ptr,
                record_kind,
                (&mut visitor as *mut BytesVisitorForwardMut<'_>).cast(),
                forward_bytes_visitor_mut,
                &mut key,
                &mut error,
            )
        };
        match status {
            RuntimeCallStatus::Success => Ok(key.into()),
            RuntimeCallStatus::Failure => Err(RuntimeHostError::new(runtime_error_message(&error))),
        }
    }

    fn update_typed_by_pk_raw(
        &mut self,
        record_kind: RecordKind,
        pk: &[u8],
        f: &mut dyn FnMut(&mut [u8]),
    ) -> Result<bool, RuntimeHostError> {
        let mut found = false;
        let mut error = RuntimeErrorBuf::new(
            RuntimeErrorPhase::RunTx,
            runtime_error_kind::HOST_INTERNAL_ERROR,
            RuntimeBytesRef::empty(),
        );
        let mut visitor = BytesVisitorForwardMut { callback: f };
        let status = unsafe {
            (self.vtable()?.update_typed_by_pk_raw)(
                self.raw.ctx_ptr,
                record_kind,
                RuntimeBytesRef::from_slice(pk),
                (&mut visitor as *mut BytesVisitorForwardMut<'_>).cast(),
                forward_bytes_visitor_mut,
                &mut found,
                &mut error,
            )
        };
        match status {
            RuntimeCallStatus::Success => Ok(found),
            RuntimeCallStatus::Failure => Err(RuntimeHostError::new(runtime_error_message(&error))),
        }
    }

    fn delete_by_pk_raw(
        &mut self,
        record_kind: RecordKind,
        pk: &[u8],
    ) -> Result<bool, RuntimeHostError> {
        let mut deleted = false;
        let mut error = RuntimeErrorBuf::new(
            RuntimeErrorPhase::RunTx,
            runtime_error_kind::HOST_INTERNAL_ERROR,
            RuntimeBytesRef::empty(),
        );
        let status = unsafe {
            (self.vtable()?.delete_by_pk_raw)(
                self.raw.ctx_ptr,
                record_kind,
                RuntimeBytesRef::from_slice(pk),
                &mut deleted,
                &mut error,
            )
        };
        match status {
            RuntimeCallStatus::Success => Ok(deleted),
            RuntimeCallStatus::Failure => Err(RuntimeHostError::new(runtime_error_message(&error))),
        }
    }

    fn emit_typed_event_raw(
        &mut self,
        event_kind: u8,
        payload: &[u8],
    ) -> Result<(), RuntimeHostError> {
        let mut error = RuntimeErrorBuf::new(
            RuntimeErrorPhase::RunTx,
            runtime_error_kind::HOST_INTERNAL_ERROR,
            RuntimeBytesRef::empty(),
        );
        let status = unsafe {
            (self.vtable()?.emit_typed_event_raw)(
                self.raw.ctx_ptr,
                event_kind,
                RuntimeBytesRef::from_slice(payload),
                &mut error,
            )
        };
        match status {
            RuntimeCallStatus::Success => Ok(()),
            RuntimeCallStatus::Failure => Err(RuntimeHostError::new(runtime_error_message(&error))),
        }
    }

    fn for_each_record_key_raw(
        &self,
        kind: RecordKind,
        f: &mut dyn FnMut(RecordKey),
    ) -> Result<(), RuntimeHostError> {
        let mut error = RuntimeErrorBuf::new(
            RuntimeErrorPhase::RunTx,
            runtime_error_kind::HOST_INTERNAL_ERROR,
            RuntimeBytesRef::empty(),
        );
        let mut visitor = RecordKeyVisitorForward { callback: f };
        let status = unsafe {
            (self.vtable()?.for_each_record_key_raw)(
                self.raw.ctx_ptr.cast_const(),
                kind,
                (&mut visitor as *mut RecordKeyVisitorForward<'_>).cast(),
                forward_record_key_visitor,
                &mut error,
            )
        };
        match status {
            RuntimeCallStatus::Success => Ok(()),
            RuntimeCallStatus::Failure => Err(RuntimeHostError::new(runtime_error_message(&error))),
        }
    }

    fn debug_log(&mut self, message: String) -> Result<(), RuntimeHostError> {
        let mut error = RuntimeErrorBuf::new(
            RuntimeErrorPhase::RunTx,
            runtime_error_kind::HOST_INTERNAL_ERROR,
            RuntimeBytesRef::empty(),
        );
        let status = unsafe {
            (self.vtable()?.debug_log)(
                self.raw.ctx_ptr,
                RuntimeBytesRef::from_slice(message.as_bytes()),
                &mut error,
            )
        };
        match status {
            RuntimeCallStatus::Success => Ok(()),
            RuntimeCallStatus::Failure => Err(RuntimeHostError::new(runtime_error_message(&error))),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct RuntimeReadContextV1Adapter {
    raw: RuntimeReadContextV1,
}

impl RuntimeReadContextV1Adapter {
    pub const fn new(raw: RuntimeReadContextV1) -> Self {
        Self { raw }
    }

    fn vtable(&self) -> Result<&RuntimeReadVTableV1, RuntimeHostError> {
        unsafe { self.raw.vtable.as_ref() }
            .ok_or_else(|| RuntimeHostError::new("runtime read vtable is null"))
    }
}

impl BizInvariantReadContext for RuntimeReadContextV1Adapter {
    fn with_read_typed_raw(
        &self,
        record_kind: RecordKind,
        sys_id: SysId,
        f: &mut dyn FnMut(&[u8]),
    ) -> Result<bool, RuntimeHostError> {
        let mut found = false;
        let mut error = RuntimeErrorBuf::new(
            RuntimeErrorPhase::ValidateBizInvariants,
            runtime_error_kind::HOST_INTERNAL_ERROR,
            RuntimeBytesRef::empty(),
        );
        let mut visitor = BytesVisitorForward { callback: f };
        let status = unsafe {
            (self.vtable()?.with_read_typed_raw)(
                self.raw.ctx_ptr.cast_const(),
                record_kind,
                sys_id,
                (&mut visitor as *mut BytesVisitorForward<'_>).cast(),
                forward_bytes_visitor,
                &mut found,
                &mut error,
            )
        };
        match status {
            RuntimeCallStatus::Success => Ok(found),
            RuntimeCallStatus::Failure => Err(RuntimeHostError::new(runtime_error_message(&error))),
        }
    }

    fn with_read_typed_by_pk_raw(
        &self,
        record_kind: RecordKind,
        pk: &[u8],
        f: &mut dyn FnMut(&[u8]),
    ) -> Result<bool, RuntimeHostError> {
        let mut found = false;
        let mut error = RuntimeErrorBuf::new(
            RuntimeErrorPhase::ValidateBizInvariants,
            runtime_error_kind::HOST_INTERNAL_ERROR,
            RuntimeBytesRef::empty(),
        );
        let mut visitor = BytesVisitorForward { callback: f };
        let status = unsafe {
            (self.vtable()?.with_read_typed_by_pk_raw)(
                self.raw.ctx_ptr.cast_const(),
                record_kind,
                RuntimeBytesRef::from_slice(pk),
                (&mut visitor as *mut BytesVisitorForward<'_>).cast(),
                forward_bytes_visitor,
                &mut found,
                &mut error,
            )
        };
        match status {
            RuntimeCallStatus::Success => Ok(found),
            RuntimeCallStatus::Failure => Err(RuntimeHostError::new(runtime_error_message(&error))),
        }
    }

    fn for_each_record_key_raw(
        &self,
        kind: RecordKind,
        f: &mut dyn FnMut(RecordKey),
    ) -> Result<(), RuntimeHostError> {
        let mut error = RuntimeErrorBuf::new(
            RuntimeErrorPhase::ValidateBizInvariants,
            runtime_error_kind::HOST_INTERNAL_ERROR,
            RuntimeBytesRef::empty(),
        );
        let mut visitor = RecordKeyVisitorForward { callback: f };
        let status = unsafe {
            (self.vtable()?.for_each_record_key_raw)(
                self.raw.ctx_ptr.cast_const(),
                kind,
                (&mut visitor as *mut RecordKeyVisitorForward<'_>).cast(),
                forward_record_key_visitor,
                &mut error,
            )
        };
        match status {
            RuntimeCallStatus::Success => Ok(()),
            RuntimeCallStatus::Failure => Err(RuntimeHostError::new(runtime_error_message(&error))),
        }
    }
}
