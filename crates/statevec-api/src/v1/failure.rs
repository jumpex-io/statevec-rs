use statevec_model::{CommandKind, CommandSchemaError, RecordKey, RecordValueError, ReservedBytesZeroError};

/// Static input-language failure. Enum names are diagnostic metadata: the
/// command and field ordinal identify the schema, without sending a Rust string
/// or promoting plugin-controlled names to a host-side `'static` lifetime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoreSchemaError {
    UnsupportedKind { kind: CommandKind },
    FieldAccess { field: u32, required: usize, actual: usize },
    Enum { field: u32, raw: u8 },
    Boolean { field: u32, raw: u8 },
    FixedBytesLength { field: u32, length: usize, capacity: usize },
    FixedBytesPadding { field: u32, offset: usize, raw: u8 },
    TrailingBytes { consumed: usize, actual: usize },
}

impl From<&CommandSchemaError> for CoreSchemaError {
    fn from(source: &CommandSchemaError) -> Self {
        match *source {
            CommandSchemaError::UnsupportedKind { kind } => Self::UnsupportedKind { kind },
            CommandSchemaError::FieldAccess { field, source } => {
                Self::FieldAccess { field, required: source.required, actual: source.actual }
            }
            CommandSchemaError::Enum { field, source } => Self::Enum { field, raw: source.raw },
            CommandSchemaError::Boolean { field, raw } => Self::Boolean { field, raw },
            CommandSchemaError::FixedBytesLength { field, length, capacity } => {
                Self::FixedBytesLength { field, length, capacity }
            }
            CommandSchemaError::FixedBytesPadding { field, offset, raw } => {
                Self::FixedBytesPadding { field, offset, raw }
            }
            CommandSchemaError::TrailingBytes { consumed, actual } => Self::TrailingBytes { consumed, actual },
        }
    }
}

/// Stable lower storage category. Native storage diagnostic text is not an ABI
/// cause or a classifier. The native host retains its original local failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreStorageError {
    Page,
    Arena,
    Slot,
    Address,
    Kind,
    SysId,
    Payload,
    Capacity,
    Invariant,
    Snapshot,
    Io,
    Internal,
}

/// Public access failure vocabulary, with no dependency on engine internals.
/// Key names are omitted; kind/key-id and exact conflicting ids are retained.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoreAccessError {
    Storage(CoreStorageError),
    /// Deterministic output bound, not a local allocation failure.
    EventSequenceOverflow,
    PayloadLength { kind: u16, expected: usize, actual: usize },
    SchemaMissing { kind: u16 },
    UnknownUniqueKey { kind: u16, unique_key_id: u8 },
    MissingRecord { key: RecordKey },
    SysIdAlreadyUsed { sys_id: u64 },
    SysIdOutOfRange { sys_id: u64 },
    RevisionOverflow { key: RecordKey },
    ImmutableFieldUpdate { key: RecordKey, field_index: u32 },
    UniqueKeyUpdate { key: RecordKey },
    ReservedBytesInvariant(ReservedBytesZeroError),
    RecordValues(RecordValueError),
    UniqueKeyConflict { kind: u16, unique_key_id: u8, sys_id: u64, existing_sys_id: u64 },
}
