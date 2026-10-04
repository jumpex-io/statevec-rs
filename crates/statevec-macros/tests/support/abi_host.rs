// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

use statevec_api::*;
use std::{cell::Cell, ffi::c_void};

/// Physical ABI fixture: returns configured real callback observations, then the
/// requested completion. It does not model plugin or engine decisions.
pub struct Host {
    pub completion: RuntimeCallStatus,
    pub visits: Cell<usize>,
    pub returned_callbacks: Cell<usize>,
    pub last_byte: Cell<u8>,
    pub keys: Vec<u64>,
}

impl Host {
    pub fn new(completion: RuntimeCallStatus) -> Self {
        Self {
            completion,
            visits: Cell::new(0),
            returned_callbacks: Cell::new(0),
            last_byte: Cell::new(0),
            keys: vec![7],
        }
    }

    pub fn raw(&mut self) -> RuntimeHostContextV1 {
        RuntimeHostContextV1 { ctx_ptr: std::ptr::from_mut(self).cast(), vtable: &HOST_VTABLE }
    }

    pub fn read_raw(&self) -> RuntimeReadContextV1 {
        RuntimeReadContextV1 { ctx_ptr: std::ptr::from_ref(self).cast_mut().cast(), vtable: &READ_VTABLE }
    }
}

unsafe extern "C" fn iterate(
    ctx: *const c_void,
    kind: u16,
    visitor_ctx: *mut c_void,
    visitor: RuntimeRecordKeyVisitor,
    error: *mut RuntimeErrorBuf,
) -> RuntimeCallStatus {
    let host = unsafe { &*ctx.cast::<Host>() };
    for &sys_id in &host.keys {
        host.visits.set(host.visits.get() + 1);
        unsafe {
            visitor(visitor_ctx, RuntimeRecordKeyView { kind, sys_id });
        }
        host.returned_callbacks.set(host.returned_callbacks.get() + 1);
    }
    if host.completion == RuntimeCallStatus::Failure {
        unsafe {
            write_runtime_error(
                error,
                RuntimeErrorPhase::RunTx,
                runtime_error_kind::HOST_INTERNAL_ERROR,
                "iteration failed",
            );
        }
    }
    host.completion
}

unsafe extern "C" fn read(
    ctx: *const c_void,
    _: u16,
    _: u64,
    visitor_ctx: *mut c_void,
    visitor: RuntimeBytesVisitor,
    found: *mut bool,
    _: *mut RuntimeErrorBuf,
) -> RuntimeCallStatus {
    let host = unsafe { &*ctx.cast::<Host>() };
    host.visits.set(host.visits.get() + 1);
    unsafe {
        visitor(visitor_ctx, RuntimeBytesRef::from_slice(&[42]));
        *found = true;
    }
    host.returned_callbacks.set(host.returned_callbacks.get() + 1);
    RuntimeCallStatus::Success
}

unsafe extern "C" fn read_uk(
    ctx: *const c_void,
    kind: u16,
    _: RuntimeBytesRef,
    visitor_ctx: *mut c_void,
    visitor: RuntimeBytesVisitor,
    found: *mut bool,
    error: *mut RuntimeErrorBuf,
) -> RuntimeCallStatus {
    unsafe { read(ctx, kind, 7, visitor_ctx, visitor, found, error) }
}

unsafe extern "C" fn create(
    ctx: *mut c_void,
    kind: u16,
    visitor_ctx: *mut c_void,
    visitor: RuntimeBytesMutVisitor,
    key: *mut RuntimeRecordKeyView,
    _: *mut RuntimeErrorBuf,
) -> RuntimeCallStatus {
    let host = unsafe { &*ctx.cast::<Host>() };
    host.visits.set(host.visits.get() + 1);
    let mut bytes = [0u8];
    unsafe {
        visitor(visitor_ctx, RuntimeBytesMutRef::from_slice(&mut bytes));
        *key = RuntimeRecordKeyView { kind, sys_id: 7 };
    }
    host.last_byte.set(bytes[0]);
    host.returned_callbacks.set(host.returned_callbacks.get() + 1);
    RuntimeCallStatus::Success
}

unsafe extern "C" fn update(
    ctx: *mut c_void,
    kind: u16,
    _: RuntimeBytesRef,
    visitor_ctx: *mut c_void,
    visitor: RuntimeBytesMutVisitor,
    found: *mut bool,
    error: *mut RuntimeErrorBuf,
) -> RuntimeCallStatus {
    let mut key = RuntimeRecordKeyView { kind, sys_id: 0 };
    let status = unsafe { create(ctx, kind, visitor_ctx, visitor, &mut key, error) };
    unsafe {
        *found = true;
    }
    status
}

unsafe extern "C" fn delete(
    _: *mut c_void,
    _: u16,
    _: RuntimeBytesRef,
    _: *mut bool,
    _: *mut RuntimeErrorBuf,
) -> RuntimeCallStatus {
    RuntimeCallStatus::Failure
}

unsafe extern "C" fn emit(_: *mut c_void, _: u16, _: RuntimeBytesRef, _: *mut RuntimeErrorBuf) -> RuntimeCallStatus {
    RuntimeCallStatus::Failure
}

unsafe extern "C" fn count(
    _: *const c_void,
    _: u16,
    _: u8,
    _: RuntimeBytesRef,
    _: usize,
    _: *mut RuntimeCanonicalIndexCountView,
    _: *mut RuntimeErrorBuf,
) -> RuntimeCallStatus {
    RuntimeCallStatus::Failure
}

unsafe extern "C" fn debug(_: *mut c_void, _: RuntimeBytesRef, _: *mut RuntimeErrorBuf) -> RuntimeCallStatus {
    RuntimeCallStatus::Failure
}

static HOST_VTABLE: RuntimeHostVTableV1 = RuntimeHostVTableV1 {
    with_read_typed_raw: read,
    with_read_typed_by_uk_raw: read_uk,
    create_typed_raw: create,
    update_typed_by_uk_raw: update,
    delete_by_uk_raw: delete,
    emit_typed_event_raw: emit,
    for_each_record_key_raw: iterate,
    count_index_prefix_capped_raw: count,
    debug_log: debug,
};

static READ_VTABLE: RuntimeReadVTableV1 = RuntimeReadVTableV1 {
    with_read_typed_raw: read,
    with_read_typed_by_uk_raw: read_uk,
    for_each_record_key_raw: iterate,
};
