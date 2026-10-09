//! Call-scoped safe transaction facade over a checked host table.
use super::{AbiFailure, ExecutionFailure, SvStatus, TxFailure, borrow::*, ffi::*};
use crate::{
    CanonicalIndexCount, ReferenceTimeUnavailable, TxPositionUnavailable, TxReadContext,
    TxSysIdCreateContext, TxUkContext, TxWriteContext,
};
use statevec_model::RecordKey;
use std::ffi::c_void;

/// Supplied by the SDK exporter for one run_tx call. It cannot outlive that
/// call or move to a service thread. Reads support shared reentry; mutable
/// visitors require &mut self and cannot call back into this transaction.
///
/// ```compile_fail
/// use statevec_api::plugin_v3::PluginTx;
/// fn retain<'call>(tx: PluginTx<'call>) -> PluginTx<'static> { tx }
/// ```
///
/// ```compile_fail
/// use statevec_api::plugin_v3::PluginTx;
/// fn move_to_thread(tx: PluginTx<'_>) {
///     std::thread::scope(|scope| { scope.spawn(move || drop(tx)); });
/// }
/// ```
pub struct PluginTx<'call> {
    table: &'call SvTxV3,
    pub(super) failure: CallFailure,
}

impl<'call> PluginTx<'call> {
    /// # Safety
    /// Host table, context and callback storage are live for 'call, have their
    /// V3 C signatures and never unwind, retain loans or run callbacks in
    /// parallel. The table header must be readable even when incompatible.
    pub(super) unsafe fn from_raw(raw: *const SvTxV3) -> Result<Self, AbiFailure> {
        // SAFETY: forwarding the caller's header/table lifetime obligation.
        let table = unsafe { table(raw, 3, CORE_BASE_CAPABILITIES)? };
        if table.context.is_null()
            || table.time_ns.is_none()
            || table.tx_seq.is_none()
            || table.read.is_none()
            || table.read_uk.is_none()
            || table.resolve_uk.is_none()
            || table.for_each_key.is_none()
            || table.count_prefix.is_none()
            || table.create.is_none()
            || table.update.is_none()
            || table.update_uk.is_none()
            || table.delete_record.is_none()
            || table.delete_uk.is_none()
            || table.emit_event.is_none()
        {
            return Err(AbiFailure::NullPointer);
        }
        Ok(Self {
            table,
            failure: CallFailure::default(),
        })
    }

    fn scalar(
        &self,
        callback: unsafe extern "C" fn(*const c_void, *mut u64) -> SvStatus,
    ) -> Result<u64, ExecutionFailure> {
        self.failure.current()?;
        let mut out = 0;
        // SAFETY: validated table, scoped context and initialized output.
        self.failure
            .check(unsafe { callback(self.table.context, &mut out) })?;
        Ok(out)
    }

    fn create(
        &mut self,
        kind: u16,
        requested: Option<u64>,
        data: Vec<u8>,
    ) -> Result<RecordKey, ExecutionFailure> {
        self.failure.current()?;
        let mut out = wire_key(RecordKey { kind: 0, sys_id: 0 });
        // SAFETY: the exact input lives until this synchronous call returns.
        // The host copies through native create; this facade does not invent
        // another allocation-cursor or padding policy.
        let status = unsafe {
            (self.table.create.unwrap())(
                self.table.context,
                kind,
                u32::from(requested.is_some()),
                requested.unwrap_or(0),
                SvBytes::borrowed(&data),
                &mut out,
            )
        };
        self.failure.check(status)?;
        let out = key(out)
            .map_err(abi)
            .inspect_err(|source| self.failure.remember(source))?;
        if out.kind != kind || requested.is_some_and(|id| id != out.sys_id) {
            let source = abi(AbiFailure::InvalidHandle);
            self.failure.remember(&source);
            return Err(source);
        }
        Ok(out)
    }

    pub(super) fn finish(&self, result: Result<(), TxFailure>) -> SvStatus {
        self.failure.finish(result)
    }
}

impl TxReadContext for PluginTx<'_> {
    type Error = ExecutionFailure;
    fn ref_tx_time_ns_raw(&self) -> Result<u64, ReferenceTimeUnavailable> {
        self.scalar(self.table.time_ns.unwrap()).map_err(|source| {
            if !matches!(source, ExecutionFailure::ReferenceTimeUnavailable) {
                self.failure.remember(&source);
            }
            ReferenceTimeUnavailable
        })
    }
    fn tx_seq_raw(&self) -> Result<u64, TxPositionUnavailable> {
        self.scalar(self.table.tx_seq.unwrap()).map_err(|source| {
            if !matches!(source, ExecutionFailure::TxPositionUnavailable) {
                self.failure.remember(&source);
            }
            TxPositionUnavailable
        })
    }
    fn with_read_raw<T>(
        &self,
        key: RecordKey,
        visit: impl FnOnce(&[u8]) -> T,
    ) -> Result<Option<T>, Self::Error> {
        self.failure.current()?;
        let mut call = ReadCall::new(visit);
        let mut found = 0;
        // SAFETY: callback state lives for this call, checked host lends bytes
        // only during the callback, and read reentry holds shared borrows only.
        let status = unsafe {
            (self.table.read.unwrap())(
                self.table.context,
                wire_key(key),
                call.callback(),
                (&mut call as *mut ReadCall<_, T>).cast(),
                &mut found,
            )
        };
        call.finish(status, found, &self.failure)
    }
    fn for_each_record_key(
        &self,
        kind: u16,
        visit: &mut dyn FnMut(RecordKey),
    ) -> Result<(), Self::Error> {
        self.failure.current()?;
        let mut call = KeyCall {
            kind,
            visit,
            count: 0,
            failure: None,
        };
        let mut out = SvWalk {
            tag: 0,
            reserved: 0,
            count: 0,
        };
        // SAFETY: synchronous shared traversal; call/visitor remain alive and
        // the host may not retain or concurrently reenter this visitor itself.
        let status = unsafe {
            (self.table.for_each_key.unwrap())(
                self.table.context,
                kind,
                Some(key_visit),
                (&mut call as *mut KeyCall<'_>).cast(),
                &mut out,
            )
        };
        self.failure.check(call.failure.unwrap_or(status))?;
        self.failure.check(status)?;
        let checked = if out.reserved != 0 {
            Err(abi(AbiFailure::Reserved {
                actual: out.reserved.into(),
            }))
        } else if out.tag != 0 {
            Err(abi(AbiFailure::Flag { actual: out.tag }))
        } else if out.count != call.count {
            Err(abi(AbiFailure::CallbackCount {
                expected: call.count,
                actual: out.count,
            }))
        } else {
            Ok(())
        };
        checked.inspect_err(|source| self.failure.remember(source))
    }
    fn count_index_prefix_capped_raw(
        &self,
        kind: u16,
        index_id: u8,
        prefix: &[u8],
        cap: usize,
    ) -> Result<CanonicalIndexCount, Self::Error> {
        self.failure.current()?;
        let mut out = SvWalk {
            tag: 0,
            reserved: 0,
            count: 0,
        };
        // SAFETY: checked callback and call-scoped prefix/output.
        let status = unsafe {
            (self.table.count_prefix.unwrap())(
                self.table.context,
                kind,
                index_id,
                SvBytes::borrowed(prefix),
                cap as u64,
                &mut out,
            )
        };
        self.failure.check(status)?;
        let result = if out.reserved != 0 {
            Err(abi(AbiFailure::Reserved {
                actual: out.reserved.into(),
            }))
        } else if out.count > cap as u64 {
            Err(abi(AbiFailure::Length {
                actual: out.count,
                maximum: cap as u64,
            }))
        } else {
            match out.tag {
                0 if out.count < cap as u64 => Ok(CanonicalIndexCount::Exact(out.count as usize)),
                1 if out.count == cap as u64 => Ok(CanonicalIndexCount::AtLeast(cap)),
                actual => Err(abi(AbiFailure::Flag { actual })),
            }
        };
        result.inspect_err(|source| self.failure.remember(source))
    }
}

impl TxUkContext for PluginTx<'_> {
    fn resolve_uk(&self, kind: u16, uk: &[u8]) -> Result<Option<u64>, Self::Error> {
        self.resolve_uk_id(kind, 0, uk)
    }
    fn resolve_uk_id(&self, kind: u16, uk_id: u8, uk: &[u8]) -> Result<Option<u64>, Self::Error> {
        self.failure.current()?;
        let (mut found, mut id) = (0, 0);
        // SAFETY: checked callback, scoped immutable key and initialized outputs.
        let status = unsafe {
            (self.table.resolve_uk.unwrap())(
                self.table.context,
                kind,
                uk_id,
                SvBytes::borrowed(uk),
                &mut found,
                &mut id,
            )
        };
        self.failure.check(status)?;
        flag(found)
            .map(|found| found.then_some(id))
            .inspect_err(|source| self.failure.remember(source))
    }
}

impl TxWriteContext for PluginTx<'_> {
    fn create_raw(&mut self, kind: u16, data: Vec<u8>) -> Result<RecordKey, Self::Error> {
        self.create(kind, None, data)
    }
    fn update_raw<T>(
        &mut self,
        key: RecordKey,
        visit: impl FnOnce(&mut [u8]) -> T,
    ) -> Result<Option<T>, Self::Error> {
        self.failure.current()?;
        let mut call = WriteCall::new(|actual, bytes: &mut [u8]| {
            if actual != key {
                return Err(abi(AbiFailure::InvalidHandle));
            }
            Ok(visit(bytes))
        });
        let mut found = 0;
        // SAFETY: &mut self excludes another safe transaction use during this
        // exclusive visitor; the checked host must enforce loans across raw C.
        let status = unsafe {
            (self.table.update.unwrap())(
                self.table.context,
                wire_key(key),
                call.callback(),
                (&mut call as *mut WriteCall<_, T>).cast(),
                &mut found,
            )
        };
        call.finish(status, found, &self.failure)
    }
    fn delete_raw(&mut self, key: RecordKey) -> Result<bool, Self::Error> {
        self.failure.current()?;
        let mut found = 0;
        // SAFETY: checked table and one initialized synchronous output.
        let status = unsafe {
            (self.table.delete_record.unwrap())(self.table.context, wire_key(key), &mut found)
        };
        self.failure.check(status)?;
        flag(found).inspect_err(|source| self.failure.remember(source))
    }
    fn emit_event_raw(&mut self, kind: u16, payload: Vec<u8>) {
        if self.failure.current().is_err() {
            return;
        }
        // SAFETY: the event bytes live for the complete synchronous callback.
        let status = unsafe {
            (self.table.emit_event.unwrap())(self.table.context, kind, SvBytes::borrowed(&payload))
        };
        if let Err(source) = self.failure.check(status) {
            self.failure.remember(&source);
        }
    }
}
impl TxSysIdCreateContext for PluginTx<'_> {
    fn create_with_sys_id_raw(
        &mut self,
        kind: u16,
        sys_id: u64,
        data: Vec<u8>,
    ) -> Result<RecordKey, Self::Error> {
        self.create(kind, Some(sys_id), data)
    }
}

struct WriteCall<F, T>(ReadCall<F, T>);
impl<F, T> WriteCall<F, T> {
    fn new(visit: F) -> Self {
        Self(ReadCall::new(visit))
    }
    fn callback(&self) -> SvWriteVisitor
    where
        F: FnOnce(RecordKey, &mut [u8]) -> Result<T, ExecutionFailure>,
    {
        Some(write_visit::<F, T>)
    }
    fn finish(
        self,
        status: SvStatus,
        found: u32,
        scope: &CallFailure,
    ) -> Result<Option<T>, ExecutionFailure> {
        self.0.finish(status, found, scope)
    }
}
unsafe extern "C" fn write_visit<
    F: FnOnce(RecordKey, &mut [u8]) -> Result<T, ExecutionFailure>,
    T,
>(
    context: *mut c_void,
    actual: SvKey,
    value: SvMutBytes,
) -> SvStatus {
    // SAFETY: adapter-owned synchronous callback record, never retained by host.
    let call = &mut unsafe { &mut *context.cast::<WriteCall<F, T>>() }.0;
    if let Some(status) = call.failure {
        return status;
    }
    let status = caught(|| {
        call.calls = call.calls.saturating_add(1);
        let Some(visit) = call.visit.take() else {
            return (&AbiFailure::CallbackCount {
                expected: 1,
                actual: call.calls,
            })
                .into();
        };
        let actual = match key(actual) {
            Ok(key) => key,
            Err(source) => return (&source).into(),
        };
        if value.len > isize::MAX as u64 {
            return (&AbiFailure::Length {
                actual: value.len,
                maximum: isize::MAX as u64,
            })
                .into();
        }
        if value.ptr.addr().checked_add(value.len as usize).is_none() {
            return (&AbiFailure::Length {
                actual: value.len,
                maximum: (usize::MAX - value.ptr.addr()) as u64,
            })
                .into();
        }
        let bytes = if value.len == 0 {
            &mut []
        } else {
            if let Err(source) = pointer(value.ptr) {
                return (&source).into();
            }
            // SAFETY: host lends exclusive writable schema bytes for this one
            // visitor; nonempty pointer and bounded extent were checked first.
            unsafe { std::slice::from_raw_parts_mut(value.ptr, value.len as usize) }
        };
        match visit(actual, bytes) {
            Ok(value) => {
                call.value = Some(value);
                SvStatus::SUCCESS
            }
            Err(source) => (&source).into(),
        }
    });
    if !status.is_success() {
        call.failure = Some(status);
    }
    status
}

struct KeyCall<'a> {
    kind: u16,
    visit: &'a mut dyn FnMut(RecordKey),
    count: u64,
    failure: Option<SvStatus>,
}
unsafe extern "C" fn key_visit(context: *mut c_void, value: SvKey, flow: *mut u32) -> SvStatus {
    // SAFETY: adapter-supplied callback storage lives until traversal returns.
    let call = unsafe { &mut *context.cast::<KeyCall<'_>>() };
    if let Some(status) = call.failure {
        return status;
    }
    let status = caught(|| {
        if let Err(source) = pointer(flow) {
            return (&source).into();
        }
        // SAFETY: checked writable flow output provided by the host callback.
        unsafe { flow.write(0) };
        let key = match key(value) {
            Ok(key) => key,
            Err(source) => return (&source).into(),
        };
        if key.kind != call.kind {
            return (&AbiFailure::InvalidHandle).into();
        }
        (call.visit)(key);
        call.count = call.count.saturating_add(1);
        SvStatus::SUCCESS
    });
    if !status.is_success() {
        call.failure = Some(status);
    }
    status
}

#[cfg(test)]
#[path = "../ut_plugin_v3_tx.rs"]
pub(super) mod tests;
