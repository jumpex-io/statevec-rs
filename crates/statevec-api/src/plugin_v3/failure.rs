use statevec_model::{CommandKind, CommandSchemaFailure, RecordKey, RecordValueError, ReservedBytesZeroError};

/// Static input-language failure. Enum names are diagnostic metadata: the
/// command and field ordinal identify the schema, without sending a Rust string
/// or promoting plugin-controlled names to a host-side `'static` lifetime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchemaFailure {
    UnsupportedKind { kind: CommandKind },
    FieldAccess { field: u32, required: usize, actual: usize },
    Enum { field: u32, raw: u8 },
    Boolean { field: u32, raw: u8 },
    FixedBytesLength { field: u32, length: usize, capacity: usize },
    FixedBytesPadding { field: u32, offset: usize, raw: u8 },
    TrailingBytes { consumed: usize, actual: usize },
}

impl From<&CommandSchemaFailure> for SchemaFailure {
    fn from(source: &CommandSchemaFailure) -> Self {
        match *source {
            CommandSchemaFailure::UnsupportedKind { kind } => Self::UnsupportedKind { kind },
            CommandSchemaFailure::FieldAccess { field, source } => {
                Self::FieldAccess { field, required: source.required, actual: source.actual }
            }
            CommandSchemaFailure::Enum { field, source } => Self::Enum { field, raw: source.raw },
            CommandSchemaFailure::Boolean { field, raw } => Self::Boolean { field, raw },
            CommandSchemaFailure::FixedBytesLength { field, length, capacity } => {
                Self::FixedBytesLength { field, length, capacity }
            }
            CommandSchemaFailure::FixedBytesPadding { field, offset, raw } => {
                Self::FixedBytesPadding { field, offset, raw }
            }
            CommandSchemaFailure::TrailingBytes { consumed, actual } => Self::TrailingBytes { consumed, actual },
        }
    }
}

/// Stable lower storage category. Native storage diagnostic text is not an ABI
/// cause or a classifier. The native host retains its original local failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageFailure {
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
pub enum AccessFailure {
    Storage(StorageFailure),
    SchemaMissing { kind: u16 },
    UnknownUniqueKey { kind: u16, uk_id: u8 },
    MissingRecord { key: RecordKey },
    SysIdAlreadyUsed { sys_id: u64 },
    SysIdOutOfRange { sys_id: u64 },
    RevisionOverflow { key: RecordKey },
    ImmutableFieldUpdate { key: RecordKey, field_index: u32 },
    UniqueKeyUpdate { key: RecordKey },
    ReservedBytesInvariant(ReservedBytesZeroError),
    RecordValues(RecordValueError),
    UkConflict { kind: u16, uk_id: u8, sys_id: u64, existing_sys_id: u64 },
}
