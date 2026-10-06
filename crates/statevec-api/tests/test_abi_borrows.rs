// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

use std::mem::MaybeUninit;

use statevec_api::{
    BizInvariantReadContext, RuntimeBytesRef, RuntimeBytesVisitor, RuntimeCallStatus, RuntimeErrorBuf,
    RuntimeErrorPhase, RuntimeReadContextV1, RuntimeReadContextV1Adapter, RuntimeReadVTableV1, RuntimeRecordKeyView,
    RuntimeRecordKeyVisitor, clear_runtime_error, runtime_error_kind, runtime_error_text, write_runtime_error,
};

#[test]
fn legacy_host_time_refusal_never_dereferences_the_abi_vtable() {
    use statevec_api::{ReferenceTimeUnavailable, RuntimeHostContext, RuntimeHostContextV1,
        RuntimeHostContextV1Adapter, TypedTxContext};
    let mut raw = RuntimeHostContextV1 { ctx_ptr: std::ptr::null_mut(), vtable: std::ptr::null() };
    // A null vtable is explicitly allowed by from_raw. Time is not in V1 and
    // must refuse locally, without invoking any foreign callback.
    let adapter = unsafe { RuntimeHostContextV1Adapter::from_raw(&mut raw) };
    assert_eq!(adapter.ref_tx_time_ns_raw(), Err(ReferenceTimeUnavailable));
    let typed: &dyn RuntimeHostContext = &adapter;
    assert_eq!(typed.ref_tx_time_ns(), Err(ReferenceTimeUnavailable));
}

#[test]
fn error_output_initializes_uninitialized_storage_and_clear_resets_it() {
    let mut storage = MaybeUninit::<RuntimeErrorBuf>::uninit();

    // ABI output storage is aligned and exclusively writable, but need not
    // already contain an initialized Rust value.
    unsafe {
        write_runtime_error(
            storage.as_mut_ptr(),
            RuntimeErrorPhase::RunTx,
            runtime_error_kind::INVALID_COMMAND,
            "invalid command",
        );
    }
    let mut error = unsafe { storage.assume_init() };
    assert_eq!(error.phase, RuntimeErrorPhase::RunTx);
    assert_eq!(error.kind, runtime_error_kind::INVALID_COMMAND);
    assert_eq!(runtime_error_text(&error), "invalid command");

    unsafe {
        clear_runtime_error(&mut error);
    }
    assert_eq!(error.phase, RuntimeErrorPhase::Load);
    assert_eq!(error.kind, runtime_error_kind::HOST_INTERNAL_ERROR);
    assert_eq!(error.message_len, 0);
    assert!(error.message_buf.iter().all(|byte| *byte == 0));
}

#[test]
fn read_adapter_borrows_a_live_vtable_and_synchronous_callback() {
    let value = 42u64;
    let raw = RuntimeReadContextV1 { ctx_ptr: std::ptr::from_ref(&value).cast_mut().cast(), vtable: &READ_VTABLE };
    // The stack context and static vtable outlive this adapter. Callbacks are
    // invoked synchronously over live stack bytes and never retained.
    let adapter = unsafe { RuntimeReadContextV1Adapter::from_raw(&raw) };
    let mut observed = None;

    let found = adapter
        .with_read_typed_raw(1, 7, &mut |bytes| {
            observed = Some(u64::from_le_bytes(bytes.try_into().unwrap()));
        })
        .unwrap();

    assert!(found);
    assert_eq!(observed, Some(value));
    let mut keys = Vec::new();
    adapter.for_each_record_key_raw(1, &mut |key| keys.push(key)).unwrap();
    assert_eq!(keys, [statevec_api::RecordKey { kind: 1, sys_id: 7 }]);
}

static READ_VTABLE: RuntimeReadVTableV1 = RuntimeReadVTableV1 {
    with_read_typed_raw: read,
    with_read_typed_by_uk_raw: read_by_uk,
    for_each_record_key_raw: for_each,
};

unsafe extern "C" fn read(
    ctx: *const std::ffi::c_void,
    _: u16,
    _: u64,
    visitor_ctx: *mut std::ffi::c_void,
    visitor: RuntimeBytesVisitor,
    found: *mut bool,
    _: *mut RuntimeErrorBuf,
) -> RuntimeCallStatus {
    let bytes = unsafe { *ctx.cast::<u64>() }.to_le_bytes();
    unsafe {
        visitor(visitor_ctx, RuntimeBytesRef::from_slice(&bytes));
        *found = true;
    }
    RuntimeCallStatus::Success
}

unsafe extern "C" fn read_by_uk(
    ctx: *const std::ffi::c_void,
    kind: u16,
    _: RuntimeBytesRef,
    visitor_ctx: *mut std::ffi::c_void,
    visitor: RuntimeBytesVisitor,
    found: *mut bool,
    error: *mut RuntimeErrorBuf,
) -> RuntimeCallStatus {
    unsafe { read(ctx, kind, 7, visitor_ctx, visitor, found, error) }
}

unsafe extern "C" fn for_each(
    _: *const std::ffi::c_void,
    kind: u16,
    visitor_ctx: *mut std::ffi::c_void,
    visitor: RuntimeRecordKeyVisitor,
    _: *mut RuntimeErrorBuf,
) -> RuntimeCallStatus {
    unsafe {
        visitor(visitor_ctx, RuntimeRecordKeyView { kind, sys_id: 7 });
    }
    RuntimeCallStatus::Success
}

#[test]
fn error_output_accepts_null_and_truncates_only_at_a_utf8_boundary() {
    unsafe {
        clear_runtime_error(std::ptr::null_mut());
        write_runtime_error(
            std::ptr::null_mut(),
            RuntimeErrorPhase::RunTx,
            runtime_error_kind::INVALID_COMMAND,
            "optional output",
        );
    }
    let mut storage = MaybeUninit::<RuntimeErrorBuf>::uninit();
    let text = "界".repeat(200);
    unsafe {
        write_runtime_error(storage.as_mut_ptr(), RuntimeErrorPhase::RunTx, runtime_error_kind::INVALID_COMMAND, &text);
    }
    let error = unsafe { storage.assume_init() };
    let expected_len = error.message_buf.len() / "界".len() * "界".len();
    assert_eq!(error.message_len, expected_len);
    assert_eq!(runtime_error_text(&error), text[..expected_len]);
}
