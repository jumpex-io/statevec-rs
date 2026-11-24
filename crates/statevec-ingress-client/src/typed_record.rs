//! Typed decoding of a read answer's records. Stateless: it holds no owner
//! state and decides nothing about a read.
use crate::ReadRecord;

/// A found record that is not the requested generated record type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum TypedRecordError {
    Kind { expected: u16, observed: u16 },
    DataLength { expected: usize, observed: usize },
}

/// View a found record through its generated accessor. The kind and the
/// complete data length are checked first: the engine answers a record's
/// schema data bytes exactly, and the accessor reads fixed offsets.
pub fn typed_record<R: statevec_model::GeneratedRecordAccess>(
    record: &ReadRecord,
) -> Result<R::Access<'_>, TypedRecordError> {
    if record.kind != R::KIND {
        return Err(TypedRecordError::Kind { expected: R::KIND, observed: record.kind });
    }
    if record.data.len() != R::DATA_LEN {
        return Err(TypedRecordError::DataLength { expected: R::DATA_LEN, observed: record.data.len() });
    }
    Ok(R::wrap(&record.data))
}
