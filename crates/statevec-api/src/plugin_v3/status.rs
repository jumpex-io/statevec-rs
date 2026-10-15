use super::{AccessFailure, SchemaFailure, StorageFailure};
use crate::BusinessRejectCode;
use statevec_model::{RecordKey, RecordValueError, ReservedBytesZeroError};

const BUSINESS: u32 = 1;
const SCHEMA: u32 = 2;
const ACCESS: u32 = 3;

/// C V3 status record. Every bit is initialized; no pointer, enum discriminant,
/// allocator-owned value or implicit padding crosses this boundary.
///
/// Public fields deliberately permit receiving arbitrary C integers. Decode
/// them with the operation's typed decoder before using their meaning. A
/// recognized error is an observation, never a retry/retirement decision.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SvStatus {
    pub tag: u32,
    pub cause: u32,
    pub detail: [u64; 3],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusDecodeFailure {
    UnknownTag { tag: u32 },
    NonzeroSuccessCause { cause: u32 },
    IllegalReturn { operation: super::CoreOperation, tag: u32 },
    UnknownOperation { raw: u32 },
    UnexpectedTag { expected: u32, actual: u32 },
    UnknownCause { tag: u32, cause: u32 },
    UnknownStorageCause { raw: u64 },
    InvalidBusinessCode { raw: u32 },
    DetailOutOfRange { slot: u8, value: u64, maximum: u64 },
    NonzeroUnusedDetail { slot: u8, value: u64 },
    /// The nested failure's fields fit their widths but contradict its cause.
    NoncanonicalNestedFailure { cause: u32 },
}

impl SvStatus {
    pub const SUCCESS: Self = Self { tag: 0, cause: 0, detail: [0; 3] };

    /// Only the all-zero record is success, not merely tag zero.
    pub fn is_success(self) -> bool {
        self == Self::SUCCESS
    }

    pub fn business_rejection(code: BusinessRejectCode) -> Self {
        Self { tag: BUSINESS, cause: u32::from(code.get()), detail: [0; 3] }
    }

    /// For a run/rule return only. Metadata, configuration and preflight must
    /// not use this decoder to authorize a business rejection.
    pub fn decode_business_rejection(self) -> Result<BusinessRejectCode, StatusDecodeFailure> {
        self.require_tag(BUSINESS)?;
        let code = u16::try_from(self.cause)
            .ok()
            .and_then(BusinessRejectCode::new)
            .ok_or(StatusDecodeFailure::InvalidBusinessCode { raw: self.cause })?;
        self.require_zero(0)?;
        self.require_zero(1)?;
        self.require_zero(2)?;
        Ok(code)
    }

    pub fn decode_schema(self) -> Result<SchemaFailure, StatusDecodeFailure> {
        self.require_tag(SCHEMA)?;
        Ok(match self.cause {
            1 => {
                self.require_zero(1)?;
                self.require_zero(2)?;
                SchemaFailure::UnsupportedKind { kind: self.u16(0)? }
            }
            2 => SchemaFailure::FieldAccess { field: self.u32(0)?, required: self.usize(1)?, actual: self.usize(2)? },
            3 | 4 => {
                self.require_zero(2)?;
                let (field, raw) = (self.u32(0)?, self.u8(1)?);
                if self.cause == 3 {
                    SchemaFailure::Enum { field, raw }
                } else {
                    SchemaFailure::Boolean { field, raw }
                }
            }
            5 => SchemaFailure::FixedBytesLength { field: self.u32(0)?, length: self.usize(1)?, capacity: self.usize(2)? },
            6 => SchemaFailure::FixedBytesPadding { field: self.u32(0)?, offset: self.usize(1)?, raw: self.u8(2)? },
            7 => {
                self.require_zero(2)?;
                SchemaFailure::TrailingBytes { consumed: self.usize(0)?, actual: self.usize(1)? }
            }
            _ => return Err(self.unknown_cause()),
        })
    }

    pub fn decode_access(self) -> Result<AccessFailure, StatusDecodeFailure> {
        self.require_tag(ACCESS)?;
        Ok(match self.cause {
            1 => {
                self.require_zero(1)?;
                self.require_zero(2)?;
                AccessFailure::Storage(match self.detail[0] {
                    1 => StorageFailure::Page,
                    2 => StorageFailure::Arena,
                    3 => StorageFailure::Slot,
                    4 => StorageFailure::Address,
                    5 => StorageFailure::Kind,
                    6 => StorageFailure::SysId,
                    7 => StorageFailure::Payload,
                    8 => StorageFailure::Capacity,
                    9 => StorageFailure::Invariant,
                    10 => StorageFailure::Snapshot,
                    11 => StorageFailure::Io,
                    12 => StorageFailure::Internal,
                    raw => return Err(StatusDecodeFailure::UnknownStorageCause { raw }),
                })
            }
            2 => {
                self.require_zero(1)?;
                self.require_zero(2)?;
                AccessFailure::SchemaMissing { kind: self.u16(0)? }
            }
            3 => {
                self.require_zero(2)?;
                AccessFailure::UnknownUniqueKey { kind: self.u16(0)?, uk_id: self.u8(1)? }
            }
            4 | 6 | 8 => {
                self.require_zero(2)?;
                let key = RecordKey { kind: self.u16(0)?, sys_id: self.detail[1] };
                match self.cause {
                    4 => AccessFailure::MissingRecord { key },
                    6 => AccessFailure::RevisionOverflow { key },
                    8 => AccessFailure::UniqueKeyUpdate { key },
                    _ => unreachable!(),
                }
            }
            5 => {
                self.require_zero(1)?;
                self.require_zero(2)?;
                AccessFailure::SysIdAlreadyUsed { sys_id: self.detail[0] }
            }
            7 => AccessFailure::ImmutableFieldUpdate {
                key: RecordKey { kind: self.u16(0)?, sys_id: self.detail[1] }, field_index: self.u32(2)?,
            },
            9 => AccessFailure::ReservedBytesInvariant(ReservedBytesZeroError::RecordDataTooShort {
                kind: self.u16(0)?, required: self.usize(1)?, actual: self.usize(2)?,
            }),
            10 => AccessFailure::ReservedBytesInvariant(ReservedBytesZeroError::PayloadTailNonZero {
                kind: self.u16(0)?, data_size: self.u32(1)?, payload_len: self.usize(2)?,
            }),
            11 | 12 => {
                self.bounded(0, (1 << 48) - 1)?;
                let kind = self.detail[0] as u16;
                let field_index = (self.detail[0] >> 16) as u32;
                let offset = self.detail[1] as u32;
                let len = (self.detail[1] >> 32) as u32;
                AccessFailure::ReservedBytesInvariant(if self.cause == 11 {
                    ReservedBytesZeroError::ReservedSpanOutOfBounds { kind, field_index, offset, len, data_size: self.u32(2)? }
                } else {
                    self.require_zero(2)?;
                    ReservedBytesZeroError::ReservedSpanNonZero { kind, field_index, offset, len }
                })
            }
            13 => {
                self.bounded(0, (1 << 24) - 1)?;
                AccessFailure::UkConflict {
                    kind: self.detail[0] as u16,
                    uk_id: (self.detail[0] >> 16) as u8,
                    sys_id: self.detail[1],
                    existing_sys_id: self.detail[2],
                }
            }
            14 => {
                self.require_zero(1)?;
                self.require_zero(2)?;
                AccessFailure::SysIdOutOfRange { sys_id: self.detail[0] }
            }
            15 => AccessFailure::RecordValues(RecordValueError::PayloadTooShort {
                kind: self.u16(0)?, required: self.usize(1)?, actual: self.usize(2)?,
            }),
            16..=19 => {
                self.bounded(0, (1 << 48) - 1)?;
                let kind = self.detail[0] as u16;
                let field_index = (self.detail[0] >> 16) as u32;
                AccessFailure::RecordValues(match self.cause {
                    16 => {
                        self.require_zero(2)?;
                        RecordValueError::FieldLayout {
                            kind, field_index, offset: self.detail[1] as u32, len: (self.detail[1] >> 32) as u32,
                        }
                    }
                    17 => {
                        self.require_zero(1)?;
                        self.require_zero(2)?;
                        RecordValueError::MissingDefinition { kind, field_index }
                    }
                    18 => {
                        self.require_zero(2)?;
                        RecordValueError::InvalidValue { kind, field_index, value: self.u8(1)? }
                    }
                    19 => RecordValueError::FixedBytesLength {
                        kind, field_index, len: self.u16(1)?, capacity: self.u32(2)?,
                    },
                    _ => unreachable!(),
                })
            }
            _ => return Err(self.unknown_cause()),
        })
    }

    fn require_tag(self, expected: u32) -> Result<(), StatusDecodeFailure> {
        if self.tag == expected {
            Ok(())
        } else {
            Err(StatusDecodeFailure::UnexpectedTag { expected, actual: self.tag })
        }
    }
    pub(super) fn unknown_cause(self) -> StatusDecodeFailure {
        StatusDecodeFailure::UnknownCause { tag: self.tag, cause: self.cause }
    }
    pub(super) fn require_zero(self, slot: u8) -> Result<(), StatusDecodeFailure> {
        let value = self.detail[usize::from(slot)];
        if value == 0 {
            Ok(())
        } else {
            Err(StatusDecodeFailure::NonzeroUnusedDetail { slot, value })
        }
    }
    fn bounded(self, slot: u8, maximum: u64) -> Result<u64, StatusDecodeFailure> {
        let value = self.detail[usize::from(slot)];
        if value <= maximum {
            Ok(value)
        } else {
            Err(StatusDecodeFailure::DetailOutOfRange { slot, value, maximum })
        }
    }
    pub(super) fn u8(self, slot: u8) -> Result<u8, StatusDecodeFailure> {
        self.bounded(slot, u8::MAX.into()).map(|v| v as u8)
    }
    fn u16(self, slot: u8) -> Result<u16, StatusDecodeFailure> {
        self.bounded(slot, u16::MAX.into()).map(|v| v as u16)
    }
    pub(super) fn u32(self, slot: u8) -> Result<u32, StatusDecodeFailure> {
        self.bounded(slot, u32::MAX.into()).map(|v| v as u32)
    }
    pub(super) fn usize(self, slot: u8) -> Result<usize, StatusDecodeFailure> {
        self.bounded(slot, usize::MAX as u64).map(|v| v as usize)
    }
}

impl From<&SchemaFailure> for SvStatus {
    fn from(source: &SchemaFailure) -> Self {
        let (cause, detail) = match *source {
            SchemaFailure::UnsupportedKind { kind } => (1, [kind.into(), 0, 0]),
            SchemaFailure::FieldAccess { field, required, actual } => (2, [field.into(), required as u64, actual as u64]),
            SchemaFailure::Enum { field, raw } => (3, [field.into(), raw.into(), 0]),
            SchemaFailure::Boolean { field, raw } => (4, [field.into(), raw.into(), 0]),
            SchemaFailure::FixedBytesLength { field, length, capacity } => (5, [field.into(), length as u64, capacity as u64]),
            SchemaFailure::FixedBytesPadding { field, offset, raw } => (6, [field.into(), offset as u64, raw.into()]),
            SchemaFailure::TrailingBytes { consumed, actual } => (7, [consumed as u64, actual as u64, 0]),
        };
        Self { tag: SCHEMA, cause, detail }
    }
}

impl From<&AccessFailure> for SvStatus {
    fn from(source: &AccessFailure) -> Self {
        let (cause, detail) = match *source {
            AccessFailure::Storage(source) => {
                let category = match source {
                    StorageFailure::Page => 1,
                    StorageFailure::Arena => 2,
                    StorageFailure::Slot => 3,
                    StorageFailure::Address => 4,
                    StorageFailure::Kind => 5,
                    StorageFailure::SysId => 6,
                    StorageFailure::Payload => 7,
                    StorageFailure::Capacity => 8,
                    StorageFailure::Invariant => 9,
                    StorageFailure::Snapshot => 10,
                    StorageFailure::Io => 11,
                    StorageFailure::Internal => 12,
                };
                (1, [category, 0, 0])
            }
            AccessFailure::SchemaMissing { kind } => (2, [kind.into(), 0, 0]),
            AccessFailure::UnknownUniqueKey { kind, uk_id } => (3, [kind.into(), uk_id.into(), 0]),
            AccessFailure::MissingRecord { key } => (4, [key.kind.into(), key.sys_id, 0]),
            AccessFailure::SysIdAlreadyUsed { sys_id } => (5, [sys_id, 0, 0]),
            AccessFailure::SysIdOutOfRange { sys_id } => (14, [sys_id, 0, 0]),
            AccessFailure::RevisionOverflow { key } => (6, [key.kind.into(), key.sys_id, 0]),
            AccessFailure::ImmutableFieldUpdate { key, field_index } => (7, [key.kind.into(), key.sys_id, field_index.into()]),
            AccessFailure::UniqueKeyUpdate { key } => (8, [key.kind.into(), key.sys_id, 0]),
            AccessFailure::ReservedBytesInvariant(ref source) => match *source {
                ReservedBytesZeroError::RecordDataTooShort { kind, required, actual } => (9, [kind.into(), required as u64, actual as u64]),
                ReservedBytesZeroError::PayloadTailNonZero { kind, data_size, payload_len } => (10, [kind.into(), data_size.into(), payload_len as u64]),
                ReservedBytesZeroError::ReservedSpanOutOfBounds { kind, field_index, offset, len, data_size } =>
                    (11, [pack_kind_field(kind, field_index), pack_u32_pair(offset, len), data_size.into()]),
                ReservedBytesZeroError::ReservedSpanNonZero { kind, field_index, offset, len } =>
                    (12, [pack_kind_field(kind, field_index), pack_u32_pair(offset, len), 0]),
            },
            AccessFailure::UkConflict { kind, uk_id, sys_id, existing_sys_id } =>
                (13, [pack_kind_field(kind, uk_id.into()), sys_id, existing_sys_id]),
            AccessFailure::RecordValues(ref source) => match *source {
                RecordValueError::PayloadTooShort { kind, required, actual } =>
                    (15, [kind.into(), required as u64, actual as u64]),
                RecordValueError::FieldLayout { kind, field_index, offset, len } =>
                    (16, [pack_kind_field(kind, field_index), pack_u32_pair(offset, len), 0]),
                RecordValueError::MissingDefinition { kind, field_index } =>
                    (17, [pack_kind_field(kind, field_index), 0, 0]),
                RecordValueError::InvalidValue { kind, field_index, value } =>
                    (18, [pack_kind_field(kind, field_index), value.into(), 0]),
                RecordValueError::FixedBytesLength { kind, field_index, len, capacity } =>
                    (19, [pack_kind_field(kind, field_index), len.into(), capacity.into()]),
            },
        };
        Self { tag: ACCESS, cause, detail }
    }
}

fn pack_kind_field(kind: u16, field: u32) -> u64 {
    u64::from(kind) | (u64::from(field) << 16)
}
fn pack_u32_pair(low: u32, high: u32) -> u64 {
    u64::from(low) | (u64::from(high) << 32)
}
