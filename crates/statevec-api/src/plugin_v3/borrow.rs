//! Checked call-scoped foreign views. These checks do not validate arbitrary
//! native addresses: allocation validity remains the trusted host obligation.
use super::{
    AbiFailure, CoreFailure, CoreOperation, ExecutionFailure, SvStatus, TxFailure, ffi::*,
};
use std::cell::Cell;
use std::ffi::c_void;
use std::mem::{align_of, size_of};
use std::panic::{AssertUnwindSafe, catch_unwind};

pub(super) fn abi(source: AbiFailure) -> ExecutionFailure {
    ExecutionFailure::AbiContract(source)
}

pub(super) fn pointer<T>(pointer: *const T) -> Result<(), AbiFailure> {
    if pointer.is_null() {
        return Err(AbiFailure::NullPointer);
    }
    if !pointer.addr().is_multiple_of(align_of::<T>()) {
        return Err(AbiFailure::Alignment {
            required: align_of::<T>(),
        });
    }
    Ok(())
}

pub(super) unsafe fn slice<'a, T>(
    pointer: *const T,
    len: u64,
    maximum: u64,
) -> Result<&'a [T], AbiFailure> {
    let maximum = maximum.min((isize::MAX as usize / size_of::<T>().max(1)) as u64);
    if len > maximum {
        return Err(AbiFailure::Length {
            actual: len,
            maximum,
        });
    }
    if len == 0 {
        return Ok(&[]);
    }
    self::pointer(pointer)?;
    if pointer
        .addr()
        .checked_add(len as usize * size_of::<T>())
        .is_none()
    {
        return Err(AbiFailure::Length {
            actual: len,
            maximum: ((usize::MAX - pointer.addr()) / size_of::<T>().max(1)) as u64,
        });
    }
    // SAFETY: caller supplies a valid immutable allocation for this scope;
    // non-null, alignment and complete byte extent were checked above.
    Ok(unsafe { std::slice::from_raw_parts(pointer, len as usize) })
}

pub(super) unsafe fn bytes<'a>(view: SvBytes) -> Result<&'a [u8], AbiFailure> {
    // SAFETY: forwarding the caller's scoped allocation/lifetime obligation.
    unsafe { slice(view.ptr, view.len, isize::MAX as u64) }
}

pub(super) fn key(key: SvKey) -> Result<statevec_model::RecordKey, AbiFailure> {
    let reserved = u64::from(key.reserved16) | u64::from(key.reserved32);
    if reserved != 0 {
        return Err(AbiFailure::Reserved { actual: reserved });
    }
    Ok(statevec_model::RecordKey {
        kind: key.kind,
        sys_id: key.sys_id,
    })
}
pub(super) fn wire_key(key: statevec_model::RecordKey) -> SvKey {
    SvKey {
        kind: key.kind,
        sys_id: key.sys_id,
        reserved16: 0,
        reserved32: 0,
    }
}

pub(super) fn flag(value: u32) -> Result<bool, ExecutionFailure> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        actual => Err(abi(AbiFailure::Flag { actual })),
    }
}

pub(super) fn caught(f: impl FnOnce() -> SvStatus) -> SvStatus {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(status) => status,
        Err(payload) => {
            // A user-supplied panic payload may itself panic in Drop.
            std::mem::forget(payload);
            SvStatus::from(&ExecutionFailure::PluginPanicked)
        }
    }
}

/// Mechanical first unusability/void-callback failure for one accepted call.
/// The native owner, not this latch or a plugin tag, decides terminal disposition.
#[derive(Default)]
pub(super) struct CallFailure(Cell<Option<SvStatus>>);
impl CallFailure {
    pub(super) fn current(&self) -> Result<(), ExecutionFailure> {
        self.0.get().map_or(Ok(()), |status| decode(status))
    }
    pub(super) fn check(&self, status: SvStatus) -> Result<(), ExecutionFailure> {
        self.current()?;
        let result = decode(status);
        if let Err(
            source @ (ExecutionFailure::PluginPanicked
            | ExecutionFailure::HostPanicked
            | ExecutionFailure::AbiContract(_)),
        ) = &result
        {
            self.remember(source);
        }
        result
    }
    pub(super) fn remember(&self, source: &ExecutionFailure) {
        if self.0.get().is_none() {
            self.0.set(Some(source.into()));
        }
    }
    pub(super) fn finish(&self, result: Result<(), TxFailure>) -> SvStatus {
        self.finish_status(result.map_or_else(|source| (&source).into(), |()| SvStatus::SUCCESS))
    }
    pub(super) fn finish_status(&self, status: SvStatus) -> SvStatus {
        match self.0.get() {
            Some(first) if matches!(status.tag, 7..=9) && !matches!(first.tag, 7..=9) => status,
            Some(first) => first,
            None => status,
        }
    }
}

pub(super) fn decode(status: SvStatus) -> Result<(), ExecutionFailure> {
    match status.decode_for(CoreOperation::HostCallback) {
        Ok(None) => Ok(()),
        Ok(Some(CoreFailure::Transaction(TxFailure::Execution(source)))) => Err(source),
        Ok(Some(_)) => Err(abi(AbiFailure::Status(
            super::StatusDecodeFailure::IllegalReturn {
                operation: CoreOperation::HostCallback,
                tag: status.tag,
            },
        ))),
        Err(source) => Err(abi(AbiFailure::Status(source))),
    }
}

pub(super) unsafe fn table<'a, T>(
    raw: *const T,
    kind: u32,
    mandatory: u64,
) -> Result<&'a T, AbiFailure> {
    pointer(raw)?;
    // SAFETY: host promises a readable header even for an incompatible table.
    let header = unsafe { raw.cast::<SvHeader>().read() };
    if header.abi_major != CORE_ABI_MAJOR {
        return Err(AbiFailure::Version {
            expected: CORE_ABI_MAJOR,
            actual: header.abi_major,
        });
    }
    if header.kind != kind {
        return Err(AbiFailure::Kind {
            expected: kind,
            actual: header.kind,
        });
    }
    if header.table_bytes != size_of::<T>() as u64 {
        return Err(AbiFailure::TableSize {
            expected: size_of::<T>() as u64,
            actual: header.table_bytes,
        });
    }
    let unsupported = (header.required_caps | header.provided_caps) & !CORE_CAPABILITIES;
    let missing = mandatory & !header.provided_caps;
    if unsupported | missing != 0 {
        return Err(AbiFailure::UnsupportedCapabilities {
            required: unsupported | missing,
        });
    }
    // SAFETY: complete extent/type representation checked; caller promises the
    // immutable table remains valid for the accepted call, not beyond it.
    Ok(unsafe { &*raw })
}

pub(super) struct ReadCall<F, T> {
    pub visit: Option<F>,
    pub value: Option<T>,
    pub calls: u64,
    pub failure: Option<SvStatus>,
}
impl<F, T> ReadCall<F, T> {
    pub fn new(visit: F) -> Self {
        Self {
            visit: Some(visit),
            value: None,
            calls: 0,
            failure: None,
        }
    }
    pub fn callback(&self) -> SvReadVisitor
    where
        F: FnOnce(&[u8]) -> T,
    {
        Some(read_visit::<F, T>)
    }
    pub fn finish(
        self,
        status: SvStatus,
        found: u32,
        scope: &CallFailure,
    ) -> Result<Option<T>, ExecutionFailure> {
        scope.check(self.failure.unwrap_or(status))?;
        scope.check(status)?;
        let found = flag(found).inspect_err(|source| scope.remember(source))?;
        if self.calls != u64::from(found) {
            let source = abi(AbiFailure::CallbackCount {
                expected: u64::from(found),
                actual: self.calls,
            });
            scope.remember(&source);
            return Err(source);
        }
        Ok(self.value)
    }
}

pub(super) unsafe extern "C" fn read_visit<F: FnOnce(&[u8]) -> T, T>(
    context: *mut c_void,
    value: SvBytes,
) -> SvStatus {
    // SAFETY: only the adapter supplies this pointer to a synchronous callback;
    // the host must not retain, forge or concurrently invoke it.
    let call = unsafe { &mut *context.cast::<ReadCall<F, T>>() };
    if let Some(status) = call.failure {
        return status;
    }
    let status = caught(|| {
        call.calls = call.calls.saturating_add(1);
        let Some(visit) = call.visit.take() else {
            return SvStatus::from(&AbiFailure::CallbackCount {
                expected: 1,
                actual: call.calls,
            });
        };
        // SAFETY: host loans immutable record bytes for exactly this callback.
        let value = match unsafe { bytes(value) } {
            Ok(value) => value,
            Err(source) => return (&source).into(),
        };
        call.value = Some(visit(value));
        SvStatus::SUCCESS
    });
    if !status.is_success() {
        call.failure = Some(status);
    }
    status
}
