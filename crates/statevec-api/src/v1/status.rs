use super::{CoreAccessError, CoreSchemaError, CoreStorageError};
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
pub enum CoreStatusDecodeError {
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
    pub fn decode_business_rejection(self) -> Result<BusinessRejectCode, CoreStatusDecodeError> {
        self.require_tag(BUSINESS)?;
        let code = u16::try_from(self.cause)
            .ok()
            .and_then(BusinessRejectCode::new)
            .ok_or(CoreStatusDecodeError::InvalidBusinessCode { raw: self.cause })?;
        self.require_zero(0)?;
        self.require_zero(1)?;
        self.require_zero(2)?;
        Ok(code)
    }

    pub fn decode_schema(self) -> Result<CoreSchemaError, CoreStatusDecodeError> {
        self.require_tag(SCHEMA)?;
        Ok(match self.cause {
            1 => {
                self.require_zero(1)?;
                self.require_zero(2)?;
                CoreSchemaError::UnsupportedKind { kind: self.u16(0)? }
            }
            2 => CoreSchemaError::FieldAccess { field: self.u32(0)?, required: self.usize(1)?, actual: self.usize(2)? },
            3 | 4 => {
                self.require_zero(2)?;
                let (field, raw) = (self.u32(0)?, self.u8(1)?);
                if self.cause == 3 {
                    CoreSchemaError::Enum { field, raw }
                } else {
                    CoreSchemaError::Boolean { field, raw }
                }
            }
            5 => CoreSchemaError::FixedBytesLength { field: self.u32(0)?, length: self.usize(1)?, capacity: self.usize(2)? },
            6 => CoreSchemaError::FixedBytesPadding { field: self.u32(0)?, offset: self.usize(1)?, raw: self.u8(2)? },
            7 => {
                self.require_zero(2)?;
                CoreSchemaError::TrailingBytes { consumed: self.usize(0)?, actual: self.usize(1)? }
            }
            _ => return Err(self.unknown_cause()),
        })
    }

    pub fn decode_access(self) -> Result<CoreAccessError, CoreStatusDecodeError> {
        self.require_tag(ACCESS)?;
        Ok(match self.cause {
            1 => {
                self.require_zero(1)?;
                self.require_zero(2)?;
                CoreAccessError::Storage(match self.detail[0] {
                    1 => CoreStorageError::Page,
                    2 => CoreStorageError::Arena,
                    3 => CoreStorageError::Slot,
                    4 => CoreStorageError::Address,
                    5 => CoreStorageError::Kind,
                    6 => CoreStorageError::SysId,
                    7 => CoreStorageError::Payload,
                    8 => CoreStorageError::Capacity,
                    9 => CoreStorageError::Invariant,
                    10 => CoreStorageError::Snapshot,
                    11 => CoreStorageError::Io,
                    12 => CoreStorageError::Internal,
                    raw => return Err(CoreStatusDecodeError::UnknownStorageCause { raw }),
                })
            }
            2 => {
                self.require_zero(1)?;
                self.require_zero(2)?;
                CoreAccessError::SchemaMissing { kind: self.u16(0)? }
            }
            3 => {
                self.require_zero(2)?;
                CoreAccessError::UnknownUniqueKey { kind: self.u16(0)?, unique_key_id: self.u8(1)? }
            }
            4 | 6 | 8 => {
                self.require_zero(2)?;
                let key = RecordKey { kind: self.u16(0)?, sys_id: self.detail[1] };
                match self.cause {
                    4 => CoreAccessError::MissingRecord { key },
                    6 => CoreAccessError::RevisionOverflow { key },
                    8 => CoreAccessError::UniqueKeyUpdate { key },
                    _ => unreachable!(),
                }
            }
            5 => {
                self.require_zero(1)?;
                self.require_zero(2)?;
                CoreAccessError::SysIdAlreadyUsed { sys_id: self.detail[0] }
            }
            7 => CoreAccessError::ImmutableFieldUpdate {
                key: RecordKey { kind: self.u16(0)?, sys_id: self.detail[1] }, field_index: self.u32(2)?,
            },
            9 => CoreAccessError::ReservedBytesInvariant(ReservedBytesZeroError::RecordDataTooShort {
                kind: self.u16(0)?, required: self.usize(1)?, actual: self.usize(2)?,
            }),
            10 => CoreAccessError::ReservedBytesInvariant(ReservedBytesZeroError::PayloadTailNonZero {
                kind: self.u16(0)?, data_size: self.u32(1)?, payload_len: self.usize(2)?,
            }),
            11 | 12 => {
                self.bounded(0, (1 << 48) - 1)?;
                let kind = self.detail[0] as u16;
                let field_index = (self.detail[0] >> 16) as u32;
                let offset = self.detail[1] as u32;
                let len = (self.detail[1] >> 32) as u32;
                CoreAccessError::ReservedBytesInvariant(if self.cause == 11 {
                    ReservedBytesZeroError::ReservedSpanOutOfBounds { kind, field_index, offset, len, data_size: self.u32(2)? }
                } else {
                    self.require_zero(2)?;
                    ReservedBytesZeroError::ReservedSpanNonZero { kind, field_index, offset, len }
                })
            }
            13 => {
                self.bounded(0, (1 << 24) - 1)?;
                CoreAccessError::UniqueKeyConflict {
                    kind: self.detail[0] as u16,
                    unique_key_id: (self.detail[0] >> 16) as u8,
                    sys_id: self.detail[1],
                    existing_sys_id: self.detail[2],
                }
            }
            14 => {
                self.require_zero(1)?;
                self.require_zero(2)?;
                CoreAccessError::SysIdOutOfRange { sys_id: self.detail[0] }
            }
            15 => CoreAccessError::RecordValues(RecordValueError::PayloadTooShort {
                kind: self.u16(0)?, required: self.usize(1)?, actual: self.usize(2)?,
            }),
            16..=19 => {
                self.bounded(0, (1 << 48) - 1)?;
                let kind = self.detail[0] as u16;
                let field_index = (self.detail[0] >> 16) as u32;
                CoreAccessError::RecordValues(match self.cause {
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
            20 => {
                self.require_zero(0)?;
                self.require_zero(1)?;
                self.require_zero(2)?;
                CoreAccessError::EventSequenceOverflow
            }
            21 => CoreAccessError::PayloadLength {
                kind: self.u16(0)?,
                expected: self.usize(1)?,
                actual: self.usize(2)?,
            },
            _ => return Err(self.unknown_cause()),
        })
    }

    fn require_tag(self, expected: u32) -> Result<(), CoreStatusDecodeError> {
        if self.tag == expected {
            Ok(())
        } else {
            Err(CoreStatusDecodeError::UnexpectedTag { expected, actual: self.tag })
        }
    }
    pub(super) fn unknown_cause(self) -> CoreStatusDecodeError {
        CoreStatusDecodeError::UnknownCause { tag: self.tag, cause: self.cause }
    }
    pub(super) fn require_zero(self, slot: u8) -> Result<(), CoreStatusDecodeError> {
        let value = self.detail[usize::from(slot)];
        if value == 0 {
            Ok(())
        } else {
            Err(CoreStatusDecodeError::NonzeroUnusedDetail { slot, value })
        }
    }
    fn bounded(self, slot: u8, maximum: u64) -> Result<u64, CoreStatusDecodeError> {
        let value = self.detail[usize::from(slot)];
        if value <= maximum {
            Ok(value)
        } else {
            Err(CoreStatusDecodeError::DetailOutOfRange { slot, value, maximum })
        }
    }
    pub(super) fn u8(self, slot: u8) -> Result<u8, CoreStatusDecodeError> {
        self.bounded(slot, u8::MAX.into()).map(|v| v as u8)
    }
    fn u16(self, slot: u8) -> Result<u16, CoreStatusDecodeError> {
        self.bounded(slot, u16::MAX.into()).map(|v| v as u16)
    }
    pub(super) fn u32(self, slot: u8) -> Result<u32, CoreStatusDecodeError> {
        self.bounded(slot, u32::MAX.into()).map(|v| v as u32)
    }
    pub(super) fn usize(self, slot: u8) -> Result<usize, CoreStatusDecodeError> {
        self.bounded(slot, usize::MAX as u64).map(|v| v as usize)
    }
}

impl From<&CoreSchemaError> for SvStatus {
    fn from(source: &CoreSchemaError) -> Self {
        let (cause, detail) = match *source {
            CoreSchemaError::UnsupportedKind { kind } => (1, [kind.into(), 0, 0]),
            CoreSchemaError::FieldAccess { field, required, actual } => (2, [field.into(), required as u64, actual as u64]),
            CoreSchemaError::Enum { field, raw } => (3, [field.into(), raw.into(), 0]),
            CoreSchemaError::Boolean { field, raw } => (4, [field.into(), raw.into(), 0]),
            CoreSchemaError::FixedBytesLength { field, length, capacity } => (5, [field.into(), length as u64, capacity as u64]),
            CoreSchemaError::FixedBytesPadding { field, offset, raw } => (6, [field.into(), offset as u64, raw.into()]),
            CoreSchemaError::TrailingBytes { consumed, actual } => (7, [consumed as u64, actual as u64, 0]),
        };
        Self { tag: SCHEMA, cause, detail }
    }
}

impl From<&CoreAccessError> for SvStatus {
    fn from(source: &CoreAccessError) -> Self {
        let (cause, detail) = match *source {
            CoreAccessError::Storage(source) => {
                let category = match source {
                    CoreStorageError::Page => 1,
                    CoreStorageError::Arena => 2,
                    CoreStorageError::Slot => 3,
                    CoreStorageError::Address => 4,
                    CoreStorageError::Kind => 5,
                    CoreStorageError::SysId => 6,
                    CoreStorageError::Payload => 7,
                    CoreStorageError::Capacity => 8,
                    CoreStorageError::Invariant => 9,
                    CoreStorageError::Snapshot => 10,
                    CoreStorageError::Io => 11,
                    CoreStorageError::Internal => 12,
                };
                (1, [category, 0, 0])
            }
            CoreAccessError::EventSequenceOverflow => (20, [0, 0, 0]),
            CoreAccessError::PayloadLength { kind, expected, actual } => {
                (21, [kind.into(), expected as u64, actual as u64])
            }
            CoreAccessError::SchemaMissing { kind } => (2, [kind.into(), 0, 0]),
            CoreAccessError::UnknownUniqueKey { kind, unique_key_id } => (3, [kind.into(), unique_key_id.into(), 0]),
            CoreAccessError::MissingRecord { key } => (4, [key.kind.into(), key.sys_id, 0]),
            CoreAccessError::SysIdAlreadyUsed { sys_id } => (5, [sys_id, 0, 0]),
            CoreAccessError::SysIdOutOfRange { sys_id } => (14, [sys_id, 0, 0]),
            CoreAccessError::RevisionOverflow { key } => (6, [key.kind.into(), key.sys_id, 0]),
            CoreAccessError::ImmutableFieldUpdate { key, field_index } => (7, [key.kind.into(), key.sys_id, field_index.into()]),
            CoreAccessError::UniqueKeyUpdate { key } => (8, [key.kind.into(), key.sys_id, 0]),
            CoreAccessError::ReservedBytesInvariant(ref source) => match *source {
                ReservedBytesZeroError::RecordDataTooShort { kind, required, actual } => (9, [kind.into(), required as u64, actual as u64]),
                ReservedBytesZeroError::PayloadTailNonZero { kind, data_size, payload_len } => (10, [kind.into(), data_size.into(), payload_len as u64]),
                ReservedBytesZeroError::ReservedSpanOutOfBounds { kind, field_index, offset, len, data_size } =>
                    (11, [pack_kind_field(kind, field_index), pack_u32_pair(offset, len), data_size.into()]),
                ReservedBytesZeroError::ReservedSpanNonZero { kind, field_index, offset, len } =>
                    (12, [pack_kind_field(kind, field_index), pack_u32_pair(offset, len), 0]),
            },
            CoreAccessError::UniqueKeyConflict { kind, unique_key_id, sys_id, existing_sys_id } =>
                (13, [pack_kind_field(kind, unique_key_id.into()), sys_id, existing_sys_id]),
            CoreAccessError::RecordValues(ref source) => match *source {
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
