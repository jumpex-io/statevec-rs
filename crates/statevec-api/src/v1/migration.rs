//! Scoped migration access: immutable old-layout reads, separate target writes.
use super::{CoreAbiError, CoreExecutionError, SvStatus, borrow::*, ffi::*, tx::{KeyCall, key_visit}};
use statevec_model::RecordKey;
use std::ffi::c_void;

/// One deterministic migration call. No instance, records or loan can escape.
///
/// ```compile_fail
/// use statevec_api::v1::MigrationTx;
/// fn retain<'call>(tx: MigrationTx<'call>) -> MigrationTx<'static> { tx }
/// ```
///
/// ```compile_fail
/// use statevec_api::v1::MigrationTx;
/// fn send(tx: MigrationTx<'_>) {
///     std::thread::scope(|scope| { scope.spawn(move || drop(tx)); });
/// }
/// ```
pub struct MigrationTx<'call> {
    table: &'call SvMigrationV4,
    pub(super) failure: CallFailure,
}

pub struct SourceRecords<'call> {
    table: &'call SvMigrationV4,
    failure: &'call CallFailure,
}

pub struct TargetRecords<'call> {
    table: &'call SvMigrationV4,
    failure: &'call CallFailure,
}

impl<'call> MigrationTx<'call> {
    /// # Safety
    /// This exact major-4 host table and all callback storage stay live for the
    /// call, never unwind across C or retain/concurrently invoke visitors.
    pub(super) unsafe fn from_raw(raw: *const SvMigrationV4) -> Result<Self, CoreAbiError> {
        // SAFETY: forward the caller's scoped host-table obligation.
        let table = unsafe { table(raw, CORE_MIGRATION_KIND, CORE_BASE_CAPABILITIES)? };
        if table.context.is_null()
            || table.time_ns.is_none() || table.tx_seq.is_none()
            || table.source_read.is_none() || table.source_keys.is_none()
            || table.preserve_record.is_none() || table.create_record.is_none()
            || table.emit_event.is_none()
        {
            return Err(CoreAbiError::NullPointer);
        }
        Ok(Self { table, failure: CallFailure::default() })
    }

    /// Fixed committed/proposed position, not an independently advancing clock.
    pub fn tx_seq(&self) -> Result<u64, CoreExecutionError> {
        self.scalar(self.table.tx_seq.unwrap())
    }

    /// Replicated transaction reference time. No wall clock is exposed.
    pub fn ref_tx_time_ns(&self) -> Result<u64, CoreExecutionError> {
        self.scalar(self.table.time_ns.unwrap())
    }

    fn scalar(&self, callback: unsafe extern "C" fn(*const c_void, *mut u64) -> SvStatus) -> Result<u64, CoreExecutionError> {
        self.failure.current()?;
        let mut out = 0;
        // SAFETY: checked scoped context and writable output.
        self.failure.check(unsafe { callback(self.table.context, &mut out) })?;
        Ok(out)
    }

    /// Borrow disjoint old and new record sets. Source reads can surround a
    /// target write, but there is no mutable source view or target-as-source cast.
    pub fn records(&mut self) -> (SourceRecords<'_>, TargetRecords<'_>) {
        (SourceRecords { table: self.table, failure: &self.failure },
         TargetRecords { table: self.table, failure: &self.failure })
    }
}

impl SourceRecords<'_> {
    pub fn with_record<T>(&self, record: RecordKey, visit: impl FnOnce(&[u8]) -> T) -> Result<Option<T>, CoreExecutionError> {
        self.failure.current()?;
        let mut call = ReadCall::new(visit);
        let mut found = 0;
        // SAFETY: shared source visitor/context stay live synchronously, with
        // the same checked invocation-count contract as ordinary record reads.
        let status = unsafe {
            (self.table.source_read.unwrap())(self.table.context, wire_key(record), call.callback(),
                (&mut call as *mut ReadCall<_, T>).cast(), &mut found)
        };
        call.finish(status, found, self.failure)
    }

    pub fn for_each_key(&self, kind: u16, visit: &mut dyn FnMut(RecordKey)) -> Result<(), CoreExecutionError> {
        self.failure.current()?;
        let mut call = KeyCall { kind, visit, count: 0, failure: None };
        let mut out = SvWalk { tag: 0, reserved: 0, count: 0 };
        // SAFETY: scoped shared source traversal; visitor may read source or
        // write the separate target, never mutate source or escape this call.
        let status = unsafe {
            (self.table.source_keys.unwrap())(self.table.context, kind, Some(key_visit),
                (&mut call as *mut KeyCall<'_>).cast(), &mut out)
        };
        call.finish(status, out, self.failure)
    }
}

impl TargetRecords<'_> {
    /// Carry a source sys_id into the complete target set, with revision + 1.
    /// Duplicate IDs, absent source records and revision overflow refuse.
    pub fn preserve_record(&mut self, source: RecordKey, target_kind: u16, bytes: &[u8]) -> Result<RecordKey, CoreExecutionError> {
        self.failure.current()?;
        let mut out = wire_key(RecordKey { kind: 0, sys_id: 0 });
        // SAFETY: source identity, borrowed payload and out live until return.
        let status = unsafe {
            (self.table.preserve_record.unwrap())(self.table.context, wire_key(source), target_kind,
                SvBytes::borrowed(bytes), &mut out)
        };
        self.result(status, out, target_kind, Some(source.sys_id))
    }

    /// Allocate a new ID from the host's monotonic source cursor, revision 1.
    pub fn create_record(&mut self, kind: u16, bytes: &[u8]) -> Result<RecordKey, CoreExecutionError> {
        self.failure.current()?;
        let mut out = wire_key(RecordKey { kind: 0, sys_id: 0 });
        // SAFETY: scoped checked callback, borrowed payload and writable output.
        let status = unsafe {
            (self.table.create_record.unwrap())(self.table.context, kind, SvBytes::borrowed(bytes), &mut out)
        };
        self.result(status, out, kind, None)
    }

    fn result(&self, status: SvStatus, out: SvKey, kind: u16, expected: Option<u64>) -> Result<RecordKey, CoreExecutionError> {
        self.failure.check(status)?;
        let result = key(out).map_err(abi).and_then(|key| {
            if key.kind != kind || key.sys_id == 0 || expected.is_some_and(|id| id != key.sys_id) {
                Err(abi(CoreAbiError::InvalidHandle))
            } else {
                Ok(key)
            }
        });
        result.inspect_err(|source| self.failure.remember(source))
    }

    pub fn emit_event(&mut self, kind: u16, bytes: &[u8]) -> Result<(), CoreExecutionError> {
        self.failure.current()?;
        // SAFETY: scoped borrowed event bytes, no pointer retained by the host.
        self.failure.check(unsafe {
            (self.table.emit_event.unwrap())(self.table.context, kind, SvBytes::borrowed(bytes))
        })
    }
}

#[cfg(test)]
#[path = "../ut_core_migration.rs"]
mod tests;
