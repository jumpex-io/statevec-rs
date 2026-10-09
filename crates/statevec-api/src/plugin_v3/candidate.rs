use super::{AbiFailure, ExecutionFailure, SvStatus, author::*, borrow::*, ffi::*};
use crate::ReferenceTimeUnavailable;
use statevec_model::RecordKey;
use std::ffi::c_void;

pub(super) struct Candidate<'call> {
    table: &'call SvCandidateV3,
    pub failure: CallFailure,
}
impl<'call> Candidate<'call> {
    /// # Safety
    /// The host supplies a live immutable table/context for this call, implements
    /// its C signatures, and loans callback bytes synchronously without unwind.
    pub unsafe fn from_raw(raw: *const SvCandidateV3) -> Result<Self, AbiFailure> {
        // SAFETY: forwarding the host's readable header and scoped allocation.
        let table = unsafe { table(raw, 4, CORE_CANDIDATE_CAPABILITY | 1)? };
        if table.context.is_null()
            || table.time_ns.is_none()
            || table.read.is_none()
            || table.resolve_uk.is_none()
            || table.changes.is_none()
        {
            return Err(AbiFailure::NullPointer);
        }
        Ok(Self {
            table,
            failure: CallFailure::default(),
        })
    }
    fn read(
        &self,
        view: u32,
        key: RecordKey,
        visit: &mut dyn FnMut(&[u8]),
    ) -> Result<bool, ExecutionFailure> {
        self.failure.current()?;
        let mut call = ReadCall::new(|bytes: &[u8]| visit(bytes));
        let mut found = 0;
        // SAFETY: visitor and its state are live until this synchronous shared
        // read returns; callback bytes cannot escape the closure's lifetime.
        let status = unsafe {
            (self.table.read.unwrap())(
                self.table.context,
                view,
                wire_key(key),
                call.callback(),
                (&mut call as *mut ReadCall<_, ()>).cast(),
                &mut found,
            )
        };
        call.finish(status, found, &self.failure)
            .map(|value| value.is_some())
    }
    fn resolve(
        &self,
        view: u32,
        kind: u16,
        uk_id: u8,
        key: &[u8],
    ) -> Result<Option<u64>, ExecutionFailure> {
        self.failure.current()?;
        let (mut found, mut id) = (0, 0);
        // SAFETY: validated callbacks, borrowed key and initialized outputs.
        let status = unsafe {
            (self.table.resolve_uk.unwrap())(
                self.table.context,
                view,
                kind,
                uk_id,
                SvBytes::borrowed(key),
                &mut found,
                &mut id,
            )
        };
        self.failure.check(status)?;
        flag(found)
            .map(|found| found.then_some(id))
            .inspect_err(|source| self.failure.remember(source))
    }
    fn changes(&self, visitor: ChangeVisitor<'_>) -> Result<(), ExecutionFailure> {
        self.failure.current()?;
        let after_only = u32::from(matches!(visitor, ChangeVisitor::After(_)));
        let mut call = ChangeCall {
            visitor,
            count: 0,
            failure: None,
        };
        let mut count = 0;
        // SAFETY: synchronous shared visitor; all loaned views stay in the
        // callback and the host reports the actually delivered member count.
        let status = unsafe {
            (self.table.changes.unwrap())(
                self.table.context,
                after_only,
                Some(change_visit),
                (&mut call as *mut ChangeCall<'_>).cast(),
                &mut count,
            )
        };
        self.failure.check(call.failure.unwrap_or(status))?;
        self.failure.check(status)?;
        if count != call.count {
            let source = abi(AbiFailure::CallbackCount {
                expected: call.count,
                actual: count,
            });
            self.failure.remember(&source);
            return Err(source);
        }
        Ok(())
    }
}
impl CandidateRead for Candidate<'_> {
    fn ref_tx_time_ns(&self) -> Result<u64, ReferenceTimeUnavailable> {
        let result = (|| {
            self.failure.current()?;
            let mut value = 0;
            // SAFETY: validated host table with initialized output.
            self.failure
                .check(unsafe { (self.table.time_ns.unwrap())(self.table.context, &mut value) })?;
            Ok(value)
        })();
        result.map_err(|source| {
            if !matches!(source, ExecutionFailure::ReferenceTimeUnavailable) {
                self.failure.remember(&source);
            }
            ReferenceTimeUnavailable
        })
    }
    fn read_before(
        &self,
        key: RecordKey,
        visit: &mut dyn FnMut(&[u8]),
    ) -> Result<bool, ExecutionFailure> {
        self.read(0, key, visit)
    }
    fn read_after(
        &self,
        key: RecordKey,
        visit: &mut dyn FnMut(&[u8]),
    ) -> Result<bool, ExecutionFailure> {
        self.read(1, key, visit)
    }
    fn resolve_uk_before(
        &self,
        kind: u16,
        uk_id: u8,
        key: &[u8],
    ) -> Result<Option<u64>, ExecutionFailure> {
        self.resolve(0, kind, uk_id, key)
    }
    fn resolve_uk_after(
        &self,
        kind: u16,
        uk_id: u8,
        key: &[u8],
    ) -> Result<Option<u64>, ExecutionFailure> {
        self.resolve(1, kind, uk_id, key)
    }
    fn for_each_change(
        &self,
        visitor: &mut dyn FnMut(CandidateChange<'_>),
    ) -> Result<(), ExecutionFailure> {
        self.changes(ChangeVisitor::Full(visitor))
    }
    fn for_each_after_change(
        &self,
        visitor: &mut dyn FnMut(CandidateAfterChange<'_>),
    ) -> Result<(), ExecutionFailure> {
        self.changes(ChangeVisitor::After(visitor))
    }
}

enum ChangeVisitor<'a> {
    Full(&'a mut dyn FnMut(CandidateChange<'_>)),
    After(&'a mut dyn FnMut(CandidateAfterChange<'_>)),
}
struct ChangeCall<'a> {
    visitor: ChangeVisitor<'a>,
    count: u64,
    failure: Option<SvStatus>,
}
unsafe extern "C" fn change_visit(
    context: *mut c_void,
    value: *const SvCandidateChange,
) -> SvStatus {
    // SAFETY: adapter-owned synchronous callback state. The host may not
    // concurrently invoke or retain it after the callback returns.
    let call = unsafe { &mut *context.cast::<ChangeCall<'_>>() };
    if let Some(status) = call.failure {
        return status;
    }
    let status = caught(|| {
        if let Err(source) = pointer(value) {
            return (&source).into();
        }
        // SAFETY: host supplies one initialized, aligned change descriptor.
        let value = unsafe { &*value };
        let after_only = matches!(call.visitor, ChangeVisitor::After(_));
        let expected_before = !after_only && matches!(value.operation, 1 | 2);
        let expected_after = matches!(value.operation, 0 | 1);
        if value.reserved != 0 {
            return (&AbiFailure::Reserved {
                actual: value.reserved.into(),
            })
                .into();
        }
        if value.operation > 2 {
            return (&AbiFailure::Flag {
                actual: value.operation,
            })
                .into();
        }
        if value.has_before != u32::from(expected_before) {
            return (&AbiFailure::Flag {
                actual: value.has_before,
            })
                .into();
        }
        if value.has_after != u32::from(expected_after) {
            return (&AbiFailure::Flag {
                actual: value.has_after,
            })
                .into();
        }
        let key = match key(value.key) {
            Ok(value) => value,
            Err(source) => return (&source).into(),
        };
        // Reject spurious absent views before following their pointers.
        for (present, bytes) in [
            (expected_before, value.before),
            (expected_after, value.after),
        ] {
            if !present && (bytes.len != 0 || !bytes.ptr.is_null()) {
                return (&AbiFailure::InvalidHandle).into();
            }
        }
        // SAFETY: checked presence; the host loans only the declared views for
        // this callback. After-only never dereferences a before view.
        let before = match unsafe { bytes(value.before) } {
            Ok(value) => value,
            Err(source) => return (&source).into(),
        };
        let after = match unsafe { bytes(value.after) } {
            Ok(value) => value,
            Err(source) => return (&source).into(),
        };
        match &mut call.visitor {
            ChangeVisitor::Full(visit) => visit(match value.operation {
                0 => CandidateChange::New { key, after },
                1 => CandidateChange::Update { key, before, after },
                _ => CandidateChange::Delete { key, before },
            }),
            ChangeVisitor::After(visit) => visit(match value.operation {
                0 => CandidateAfterChange::New { key, after },
                1 => CandidateAfterChange::Update { key, after },
                _ => CandidateAfterChange::Delete { key },
            }),
        }
        call.count = call.count.saturating_add(1);
        SvStatus::SUCCESS
    });
    if !status.is_success() {
        call.failure = Some(status);
    }
    status
}

#[cfg(test)]
#[path = "../ut_plugin_v3_candidate.rs"]
mod tests;
