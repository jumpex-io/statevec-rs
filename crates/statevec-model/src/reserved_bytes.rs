// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

//! Canonical validation of reserved record spans and implicit slot padding.

use crate::{FieldDefinition, RecordDefinition, RecordKind};

/// Invariant enforced by the engine's record mutation and restore boundaries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum EngineSchemaInvariantProfile {
    ReservedBytesZeroV1,
}

/// Exact failure to establish the reserved-bytes-zero invariant for one row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReservedBytesZeroError {
    RecordDataTooShort { kind: RecordKind, required: usize, actual: usize },
    PayloadTailNonZero { kind: RecordKind, data_size: u32, payload_len: usize },
    ReservedSpanOutOfBounds { kind: RecordKind, field_index: u32, offset: u32, len: u32, data_size: u32 },
    ReservedSpanNonZero { kind: RecordKind, field_index: u32, offset: u32, len: u32 },
}

/// Validates declared reserved spans and padding beyond the schema's data size.
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
    let required = data_size as usize;
    if data.len() < required {
        return Err(ReservedBytesZeroError::RecordDataTooShort { kind, required, actual: data.len() });
    }
    if data[required..].iter().any(|byte| *byte != 0) {
        return Err(ReservedBytesZeroError::PayloadTailNonZero { kind, data_size, payload_len: data.len() });
    }
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
