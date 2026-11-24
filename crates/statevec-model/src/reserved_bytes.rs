// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

//! Checked record layout and zero-byte validation. Retired fields retain opaque bytes.

use crate::{FieldDefinition, RecordDefinition, RecordKind};

/// Invariant enforced by the engine's record mutation and restore boundaries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum EngineSchemaInvariantProfile {
    ReservedBytesZeroV1,
    UnusedBytesZeroV2,
    /// Only unallocated gaps and padding are zero; reserved fields are opaque.
    UnusedBytesZero,
}

impl EngineSchemaInvariantProfile {
    pub const fn to_wire(self) -> u16 {
        match self {
            Self::ReservedBytesZeroV1 => 1,
            Self::UnusedBytesZeroV2 => 2,
            Self::UnusedBytesZero => 3,
        }
    }

    pub const fn from_wire(value: u16) -> Option<Self> {
        match value {
            1 => Some(Self::ReservedBytesZeroV1),
            2 => Some(Self::UnusedBytesZeroV2),
            3 => Some(Self::UnusedBytesZero),
            _ => None,
        }
    }
}

/// Exact row-validation failure. The historical name and reserved-span causes
/// remain for ABI stability and legacy diagnostics; V3 checks only unused bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReservedBytesZeroError {
    RecordDataTooShort { kind: RecordKind, required: usize, actual: usize },
    PayloadTailNonZero { kind: RecordKind, data_size: u32, payload_len: usize },
    ReservedSpanOutOfBounds { kind: RecordKind, field_index: u32, offset: u32, len: u32, data_size: u32 },
    ReservedSpanNonZero { kind: RecordKind, field_index: u32, offset: u32, len: u32 },
    UnusedSpanNonZero { kind: RecordKind, offset: u32, len: u32 },
}

/// Read-only zero interval outside both active and reserved field descriptors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ZeroSpan {
    pub offset: u32,
    pub len: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordLayoutError {
    DuplicateFieldIndex { kind: RecordKind, field_index: u32 },
    DuplicateFieldName { kind: RecordKind, name: String },
    SpanOutOfBounds { kind: RecordKind, field_index: u32, reserved: bool, offset: u32, len: u32, data_size: u32 },
    Overlap { kind: RecordKind, first_field: u32, second_field: u32, first_reserved: bool, second_reserved: bool },
}

pub(crate) fn zero_spans(definition: &RecordDefinition) -> Result<Box<[ZeroSpan]>, RecordLayoutError> {
    let mut intervals = Vec::with_capacity(definition.fields.len() + definition.reserved_fields.len());
    let mut ids = std::collections::BTreeSet::new();
    let mut names = std::collections::BTreeSet::new();
    for (fields, reserved) in [(definition.fields, false), (definition.reserved_fields, true)] {
        for field in fields {
            if !ids.insert(field.field_index) {
                return Err(RecordLayoutError::DuplicateFieldIndex {
                    kind: definition.kind,
                    field_index: field.field_index,
                });
            }
            if !names.insert(field.name) {
                return Err(RecordLayoutError::DuplicateFieldName {
                    kind: definition.kind,
                    name: field.name.to_owned(),
                });
            }
            let end = field
                .offset
                .checked_add(field.len)
                .filter(|end| *end <= definition.data_size)
                .ok_or(RecordLayoutError::SpanOutOfBounds {
                    kind: definition.kind,
                    field_index: field.field_index,
                    reserved,
                    offset: field.offset,
                    len: field.len,
                    data_size: definition.data_size,
                })?;
            if field.len != 0 {
                intervals.push((field.offset, end, field.field_index, reserved));
            }
        }
    }
    intervals.sort_unstable();
    for pair in intervals.windows(2) {
        if pair[0].1 > pair[1].0 {
            return Err(RecordLayoutError::Overlap {
                kind: definition.kind,
                first_field: pair[0].2,
                second_field: pair[1].2,
                first_reserved: pair[0].3,
                second_reserved: pair[1].3,
            });
        }
    }
    let mut spans = Vec::with_capacity(intervals.len() + 1);
    let mut end = 0;
    for (offset, next, _, _) in intervals {
        if end < offset {
            spans.push(ZeroSpan { offset: end, len: offset - end });
        }
        end = next;
    }
    if end < definition.data_size {
        spans.push(ZeroSpan { offset: end, len: definition.data_size - end });
    }
    Ok(spans.into_boxed_slice())
}

pub(crate) fn validate_record_padding_zero(
    definition: &RecordDefinition,
    data: &[u8],
) -> Result<(), ReservedBytesZeroError> {
    let kind = definition.kind;
    let data_size = definition.data_size;
    let required = data_size as usize;
    if data.len() < required {
        return Err(ReservedBytesZeroError::RecordDataTooShort { kind, required, actual: data.len() });
    }
    if data[required..].iter().any(|byte| *byte != 0) {
        return Err(ReservedBytesZeroError::PayloadTailNonZero { kind, data_size, payload_len: data.len() });
    }
    Ok(())
}

/// Legacy V1/V2 diagnostic: validates reserved spans and physical padding.
/// Current execution uses `SchemaRegistry::validate_record_unused_bytes`, which
/// deliberately permits arbitrary bytes in reserved fields under V3.
///
/// A rounded physical slot may contain extra bytes, but restore requires that
/// implicit tail to remain zero. Work is proportional to reserved/padding bytes;
/// this performs no table scan and accepts no caller-provided offsets.
pub fn validate_reserved_bytes_zero(definition: &RecordDefinition, data: &[u8]) -> Result<(), ReservedBytesZeroError> {
    let RecordDefinition {
        kind,
        name: _,
        data_size,
        version: _,
        unique_keys: _,
        canonical_indexes: _,
        fields: _,
        reserved_fields,
    } = *definition;
    validate_record_padding_zero(definition, data)?;
    for field in reserved_fields {
        let FieldDefinition {
            name: _,
            field_index,
            offset,
            ty: _,
            len,
            rust_type_name: _,
            enum_type_name: _,
            decimal_scale: _,
            semantic: _,
            immutable: _,
        } = *field;
        let Some(end) = offset.checked_add(len) else {
            return Err(ReservedBytesZeroError::ReservedSpanOutOfBounds { kind, field_index, offset, len, data_size });
        };
        if end > data_size {
            return Err(ReservedBytesZeroError::ReservedSpanOutOfBounds { kind, field_index, offset, len, data_size });
        }
        if data[offset as usize..end as usize].iter().any(|byte| *byte != 0) {
            return Err(ReservedBytesZeroError::ReservedSpanNonZero { kind, field_index, offset, len });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FieldDefinition, FieldType};

    static RESERVED: FieldDefinition = FieldDefinition {
        name: "future",
        field_index: 2,
        offset: 8,
        ty: FieldType::U64,
        len: 8,
        rust_type_name: "u64",
        enum_type_name: None,
        decimal_scale: None,
        semantic: None,
        immutable: false,
    };
    static RECORD: RecordDefinition = RecordDefinition {
        kind: 1,
        name: "Account",
        data_size: 16,
        version: 1,
        unique_keys: &[],
        canonical_indexes: &[],
        fields: &[],
        reserved_fields: &[RESERVED],
    };

    #[test]
    fn validates_only_declared_reserved_span() {
        let mut data = [0u8; 16];
        data[0] = 7;
        assert_eq!(validate_reserved_bytes_zero(&RECORD, &data), Ok(()));

        data[15] = 1;
        assert_eq!(
            validate_reserved_bytes_zero(&RECORD, &data),
            Err(ReservedBytesZeroError::ReservedSpanNonZero { kind: 1, field_index: 2, offset: 8, len: 8 })
        );
    }

    #[test]
    fn rejects_short_data_before_slicing() {
        assert_eq!(
            validate_reserved_bytes_zero(&RECORD, &[0; 8]),
            Err(ReservedBytesZeroError::RecordDataTooShort { kind: 1, required: 16, actual: 8 })
        );
    }

    #[test_case::test_case(16; "first_padding_byte")]
    #[test_case::test_case(39; "last_padding_byte")]
    fn rejects_nonzero_slot_tail_even_without_declared_reserved_fields(offset: usize) {
        let definition = RecordDefinition { reserved_fields: &[], ..RECORD };
        let mut data = [0; 40];
        data[0] = 7;
        assert_eq!(validate_reserved_bytes_zero(&definition, &data), Ok(()));
        assert_eq!(validate_reserved_bytes_zero(&definition, &data[..16]), Ok(()));

        data[offset] = 0x5a;

        assert_eq!(
            validate_reserved_bytes_zero(&definition, &data),
            Err(ReservedBytesZeroError::PayloadTailNonZero { kind: 1, data_size: 16, payload_len: 40 })
        );
    }
}
