// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

use crate::registry::{SchemaRegistry, field_type_tag};
use crate::{
    CanonicalIndexDefinition, CommandDefinition, EnumDefinition, EnumVariantDefinition, EventDefinition,
    FieldDefinition, FieldType, KeyBytes, MAX_CANONICAL_INDEX_KEY_BYTES, PayloadFieldDefinition, RecordDefinition,
    SemanticTag, UniqueKeyDefinition, Version,
};

fn encode_uk(data: &[u8]) -> KeyBytes {
    let mut uk = KeyBytes::new();
    uk.extend_from_slice(&data[..8]);
    uk
}

static RECORD_FIELDS_A: &[FieldDefinition] = &[FieldDefinition {
    name: "id",
    field_index: 1,
    offset: 0,
    ty: FieldType::U64,
    len: 8,
    rust_type_name: "u64",
    enum_type_name: None,
    decimal_scale: None,
    semantic: None,
    immutable: true,
}];
static RECORD_FIELDS_B: &[FieldDefinition] = &[FieldDefinition {
    name: "value",
    field_index: 1,
    offset: 0,
    ty: FieldType::U32,
    len: 4,
    rust_type_name: "u32",
    enum_type_name: None,
    decimal_scale: None,
    semantic: None,
    immutable: false,
}];
static RESERVED_RECORD_FIELDS: &[FieldDefinition] = &[FieldDefinition {
    name: "_reserved_2",
    field_index: 2,
    offset: 8,
    ty: FieldType::U32,
    len: 4,
    rust_type_name: "u32",
    enum_type_name: None,
    decimal_scale: None,
    semantic: None,
    immutable: false,
}];
static RECORD_A_INDEX_BY_ID: &[CanonicalIndexDefinition] =
    &[CanonicalIndexDefinition { id: 0, name: "by_id", encode: None, fields: &["id"] }];
static RECORD_A_INDEX_BY_RENAMED_ID: &[CanonicalIndexDefinition] =
    &[CanonicalIndexDefinition { id: 0, name: "by_renamed_id", encode: None, fields: &["id"] }];
static RECORD_A_TWO_UKS: &[UniqueKeyDefinition] = &[
    UniqueKeyDefinition { id: 0, name: "by_id", encode: Some(encode_uk), fields: &["id"] },
    UniqueKeyDefinition { id: 1, name: "by_id_again", encode: Some(encode_uk), fields: &["id"] },
];
static RECORD_A_TWO_UKS_REORDERED: &[UniqueKeyDefinition] = &[
    UniqueKeyDefinition { id: 1, name: "by_id_again", encode: Some(encode_uk), fields: &["id"] },
    UniqueKeyDefinition { id: 0, name: "by_id", encode: Some(encode_uk), fields: &["id"] },
];
static RECORD_A_TWO_INDEXES: &[CanonicalIndexDefinition] = &[
    CanonicalIndexDefinition { id: 0, name: "by_id", encode: None, fields: &["id"] },
    CanonicalIndexDefinition { id: 1, name: "by_id_again", encode: None, fields: &["id"] },
];
static RECORD_A_TWO_INDEXES_REORDERED: &[CanonicalIndexDefinition] = &[
    CanonicalIndexDefinition { id: 1, name: "by_id_again", encode: None, fields: &["id"] },
    CanonicalIndexDefinition { id: 0, name: "by_id", encode: None, fields: &["id"] },
];
static FIXED_64_FIELDS: &[FieldDefinition] = &[FieldDefinition {
    name: "blob",
    field_index: 1,
    offset: 0,
    ty: FieldType::FixedBytes,
    len: 66,
    rust_type_name: "FixedBytes<64>",
    enum_type_name: None,
    decimal_scale: None,
    semantic: None,
    immutable: false,
}];
static DECIMAL_FIELDS_SCALE_2: &[FieldDefinition] = &[FieldDefinition {
    name: "amount",
    field_index: 1,
    offset: 0,
    ty: FieldType::Decimal,
    len: 16,
    rust_type_name: "Decimal<2>",
    enum_type_name: None,
    decimal_scale: Some(2),
    semantic: None,
    immutable: true,
}];
static DECIMAL_FIELDS_SCALE_6: &[FieldDefinition] = &[FieldDefinition {
    name: "amount",
    field_index: 1,
    offset: 0,
    ty: FieldType::Decimal,
    len: 16,
    rust_type_name: "Decimal<6>",
    enum_type_name: None,
    decimal_scale: Some(6),
    semantic: None,
    immutable: true,
}];
static DECIMAL_FIELDS_MISSING_SCALE: &[FieldDefinition] =
    &[FieldDefinition { decimal_scale: None, ..DECIMAL_FIELDS_SCALE_2[0] }];
static DECIMAL_FIELDS_TOO_LARGE_SCALE: &[FieldDefinition] =
    &[FieldDefinition { decimal_scale: Some(255), ..DECIMAL_FIELDS_SCALE_2[0] }];
static NON_DECIMAL_FIELDS_WITH_SCALE: &[FieldDefinition] =
    &[FieldDefinition { decimal_scale: Some(2), ..RECORD_FIELDS_A[0] }];
static FIXED_64_INDEX: &[CanonicalIndexDefinition] =
    &[CanonicalIndexDefinition { id: 0, name: "by_blob", encode: None, fields: &["blob"] }];
static DECIMAL_UK: &[UniqueKeyDefinition] =
    &[UniqueKeyDefinition { id: 0, name: "", encode: None, fields: &["amount"] }];
static DECIMAL_INDEX: &[CanonicalIndexDefinition] =
    &[CanonicalIndexDefinition { id: 0, name: "by_amount", encode: None, fields: &["amount"] }];
static COMMAND_FIELDS: &[PayloadFieldDefinition] = &[PayloadFieldDefinition {
    name: "amount",
    field_index: 1,
    ty: FieldType::U64,
    rust_type_name: "u64",
    enum_type_name: None,
    decimal_scale: None,
    semantic: None,
    repeated: false,
    element_count_max: 0,
    fixed_size: Some(8),
}];
static COMMAND_FIELDS_ALT: &[PayloadFieldDefinition] = &[PayloadFieldDefinition {
    name: "amount",
    field_index: 1,
    ty: FieldType::U64,
    rust_type_name: "u64",
    enum_type_name: None,
    decimal_scale: None,
    semantic: None,
    repeated: false,
    element_count_max: 0,
    fixed_size: Some(16),
}];
static EVENT_FIELDS: &[PayloadFieldDefinition] = &[PayloadFieldDefinition {
    name: "status",
    field_index: 1,
    ty: FieldType::U8,
    rust_type_name: "u8",
    enum_type_name: None,
    decimal_scale: None,
    semantic: None,
    repeated: false,
    element_count_max: 0,
    fixed_size: Some(1),
}];
static EVENT_FIELDS_REPEATED_16: &[PayloadFieldDefinition] =
    &[PayloadFieldDefinition { repeated: true, element_count_max: 16, ..COMMAND_FIELDS[0] }];
static EVENT_FIELDS_REPEATED_32: &[PayloadFieldDefinition] =
    &[PayloadFieldDefinition { repeated: true, element_count_max: 32, ..COMMAND_FIELDS[0] }];
static COMMAND_FIELDS_REPEATED: &[PayloadFieldDefinition] =
    &[PayloadFieldDefinition { repeated: true, element_count_max: 16, ..COMMAND_FIELDS[0] }];
static COMMAND_FIELDS_TIMESTAMP_SEMANTIC: &[PayloadFieldDefinition] =
    &[PayloadFieldDefinition { semantic: Some(SemanticTag::TimestampMicros), ..COMMAND_FIELDS[0] }];
static COMMAND_FIELDS_ILLEGAL_TEXT_SEMANTIC: &[PayloadFieldDefinition] =
    &[PayloadFieldDefinition { semantic: Some(SemanticTag::Text), ..COMMAND_FIELDS[0] }];

static RECORD_A: RecordDefinition = RecordDefinition {
    kind: 1,
    name: "A",
    data_size: 16,
    version: 1,
    unique_keys: &[crate::UniqueKeyDefinition { id: 0, name: "", encode: Some(encode_uk), fields: &["id"] }],
    canonical_indexes: &[],
    fields: RECORD_FIELDS_A,
    reserved_fields: &[],
};
static RECORD_B: RecordDefinition = RecordDefinition {
    kind: 2,
    name: "B",
    data_size: 16,
    version: 1,
    unique_keys: &[],
    canonical_indexes: &[],
    fields: RECORD_FIELDS_B,
    reserved_fields: &[],
};
static RECORD_A_WITH_RESERVED: RecordDefinition = RecordDefinition {
    kind: 1,
    name: "A",
    data_size: 16,
    version: 1,
    unique_keys: &[crate::UniqueKeyDefinition { id: 0, name: "", encode: Some(encode_uk), fields: &["id"] }],
    canonical_indexes: &[],
    fields: RECORD_FIELDS_A,
    reserved_fields: RESERVED_RECORD_FIELDS,
};
static RECORD_ZERO: RecordDefinition = RecordDefinition {
    kind: 0,
    name: "Zero",
    data_size: 16,
    version: 1,
    unique_keys: &[],
    canonical_indexes: &[],
    fields: RECORD_FIELDS_A,
    reserved_fields: &[],
};
static RECORD_USER_MAX: RecordDefinition = RecordDefinition {
    kind: crate::USER_KIND_MAX,
    name: "UserMax",
    data_size: 16,
    version: 1,
    unique_keys: &[],
    canonical_indexes: &[],
    fields: RECORD_FIELDS_A,
    reserved_fields: &[],
};
static RECORD_SYSTEM_MIN: RecordDefinition = RecordDefinition {
    kind: crate::SYSTEM_KIND_MIN,
    name: "SystemMin",
    data_size: 16,
    version: 1,
    unique_keys: &[],
    canonical_indexes: &[],
    fields: RECORD_FIELDS_A,
    reserved_fields: &[],
};
static COMMAND_A: CommandDefinition = CommandDefinition { kind: 10, name: "CmdA", version: 1, fields: COMMAND_FIELDS };
static COMMAND_A_ALT: CommandDefinition =
    CommandDefinition { kind: 10, name: "CmdA", version: 1, fields: COMMAND_FIELDS_ALT };
static COMMAND_ZERO: CommandDefinition = CommandDefinition { kind: 0, name: "CmdZero", version: 1, fields: &[] };
static COMMAND_SYSTEM_MIN: CommandDefinition =
    CommandDefinition { kind: crate::SYSTEM_KIND_MIN, name: "CmdSystemMin", version: 1, fields: &[] };
static EVENT_A: EventDefinition =
    EventDefinition { kind: 20, name: "EvtA", version: 1, fields: EVENT_FIELDS, inline_response: false };
static EVENT_A_REPEATED_16: EventDefinition = EventDefinition { fields: EVENT_FIELDS_REPEATED_16, ..EVENT_A };
static EVENT_A_REPEATED_32: EventDefinition = EventDefinition { fields: EVENT_FIELDS_REPEATED_32, ..EVENT_A };
static COMMAND_A_REPEATED: CommandDefinition = CommandDefinition { fields: COMMAND_FIELDS_REPEATED, ..COMMAND_A };
static COMMAND_A_TIMESTAMP_SEMANTIC: CommandDefinition =
    CommandDefinition { fields: COMMAND_FIELDS_TIMESTAMP_SEMANTIC, ..COMMAND_A };
static COMMAND_A_ILLEGAL_TEXT_SEMANTIC: CommandDefinition =
    CommandDefinition { fields: COMMAND_FIELDS_ILLEGAL_TEXT_SEMANTIC, ..COMMAND_A };
static EVENT_ZERO: EventDefinition =
    EventDefinition { kind: 0, name: "EvtZero", version: 1, fields: &[], inline_response: false };
static EVENT_SYSTEM_MIN: EventDefinition = EventDefinition {
    kind: crate::SYSTEM_KIND_MIN,
    name: "EvtSystemMin",
    version: 1,
    fields: &[],
    inline_response: false,
};
static ENUM_A: EnumDefinition = EnumDefinition {
    name: "Status",
    variants: &[
        EnumVariantDefinition { name: "Pending", discriminant: 1 },
        EnumVariantDefinition { name: "Active", discriminant: 2 },
    ],
};
static ENUM_B: EnumDefinition = EnumDefinition {
    name: "Status",
    variants: &[
        EnumVariantDefinition { name: "Pending", discriminant: 1 },
        EnumVariantDefinition { name: "Closed", discriminant: 3 },
    ],
};

#[test]
fn decimal_field_type_tag_is_append_only() {
    assert_eq!(field_type_tag(FieldType::Decimal), 12);
}

#[test]
fn schema_registry_fingerprints_are_stable_across_definition_order() {
    let first = SchemaRegistry::new(Version::new(1, 0), &[RECORD_A, RECORD_B], &[COMMAND_A], &[EVENT_A], &[ENUM_A]);
    let second = SchemaRegistry::new(Version::new(1, 0), &[RECORD_B, RECORD_A], &[COMMAND_A], &[EVENT_A], &[ENUM_A]);

    assert_eq!(first.schema_version(), Version::new(1, 0));
    assert_eq!(first.record_schema_fingerprint(), second.record_schema_fingerprint());
    assert_eq!(first.command_schema_fingerprint(), second.command_schema_fingerprint());
    assert_eq!(first.event_schema_fingerprint(), second.event_schema_fingerprint());
    assert_eq!(first.types_schema_fingerprint(), second.types_schema_fingerprint());
    assert_eq!(first.try_get_command(10).unwrap().name, "CmdA");
    assert_eq!(first.try_get_event(20).unwrap().name, "EvtA");
}

#[test]
fn schema_registry_types_fingerprint_changes_with_enum_definition() {
    let first = SchemaRegistry::new(Version::new(1, 0), &[RECORD_A], &[COMMAND_A], &[EVENT_A], &[ENUM_A]);
    let second = SchemaRegistry::new(Version::new(1, 0), &[RECORD_A], &[COMMAND_A], &[EVENT_A], &[ENUM_B]);

    assert_ne!(first.types_schema_fingerprint(), second.types_schema_fingerprint());
}

#[test]
fn schema_registry_record_fingerprint_changes_with_reserved_fields() {
    let first = SchemaRegistry::new(Version::new(1, 0), &[RECORD_A], &[COMMAND_A], &[EVENT_A], &[ENUM_A]);
    let second =
        SchemaRegistry::new(Version::new(1, 0), &[RECORD_A_WITH_RESERVED], &[COMMAND_A], &[EVENT_A], &[ENUM_A]);

    assert_ne!(first.record_schema_fingerprint(), second.record_schema_fingerprint());
}

#[test]
fn schema_registry_record_fingerprint_changes_with_decimal_scale() {
    let scale_2 = RecordDefinition {
        kind: 3,
        name: "DecimalRecord",
        data_size: 16,
        version: 1,
        unique_keys: &[],
        canonical_indexes: &[],
        fields: DECIMAL_FIELDS_SCALE_2,
        reserved_fields: &[],
    };
    let scale_6 = RecordDefinition { fields: DECIMAL_FIELDS_SCALE_6, ..scale_2 };

    let first = SchemaRegistry::new(Version::new(1, 0), &[scale_2], &[], &[], &[]);
    let second = SchemaRegistry::new(Version::new(1, 0), &[scale_6], &[], &[], &[]);

    assert_ne!(first.record_schema_fingerprint(), second.record_schema_fingerprint());
}

#[test]
fn schema_registry_fingerprint_ignores_semantic_metadata() {
    let plain = SchemaRegistry::new(Version::new(1, 0), &[RECORD_A], &[COMMAND_A], &[EVENT_A], &[ENUM_A]);
    let semantic =
        SchemaRegistry::new(Version::new(1, 0), &[RECORD_A], &[COMMAND_A_TIMESTAMP_SEMANTIC], &[EVENT_A], &[ENUM_A]);

    assert_eq!(plain.command_schema_fingerprint(), semantic.command_schema_fingerprint());
    assert_eq!(plain.identity(), semantic.identity());
}

#[test]
fn schema_registry_event_fingerprint_includes_repeated_metadata() {
    let plain = SchemaRegistry::new(Version::new(1, 0), &[RECORD_A], &[COMMAND_A], &[EVENT_A], &[ENUM_A]);
    let repeated_16 = SchemaRegistry::new(Version::new(1, 0), &[RECORD_A], &[COMMAND_A], &[EVENT_A_REPEATED_16], &[]);
    let repeated_32 = SchemaRegistry::new(Version::new(1, 0), &[RECORD_A], &[COMMAND_A], &[EVENT_A_REPEATED_32], &[]);

    assert_ne!(plain.event_schema_fingerprint(), repeated_16.event_schema_fingerprint());
    assert_ne!(repeated_16.event_schema_fingerprint(), repeated_32.event_schema_fingerprint());
}

#[test]
#[should_panic(expected = "command field repeated metadata mismatch")]
fn schema_registry_rejects_repeated_command_payload_fields() {
    let _ = SchemaRegistry::new(Version::new(1, 0), &[RECORD_A], &[COMMAND_A_REPEATED], &[EVENT_A], &[]);
}

#[test]
#[should_panic(expected = "command field semantic/type mismatch")]
fn schema_registry_rejects_manual_illegal_semantic_metadata() {
    let _ = SchemaRegistry::new(Version::new(1, 0), &[RECORD_A], &[COMMAND_A_ILLEGAL_TEXT_SEMANTIC], &[EVENT_A], &[]);
}

#[test]
#[should_panic(expected = "record field decimal scale/type mismatch")]
fn schema_registry_rejects_decimal_field_missing_scale() {
    let record = RecordDefinition { fields: DECIMAL_FIELDS_MISSING_SCALE, ..RECORD_A };
    let _ = SchemaRegistry::new(Version::new(1, 0), &[record], &[COMMAND_A], &[EVENT_A], &[]);
}

#[test]
#[should_panic(expected = "record field decimal scale/type mismatch")]
fn schema_registry_rejects_decimal_field_too_large_scale() {
    let record = RecordDefinition { fields: DECIMAL_FIELDS_TOO_LARGE_SCALE, ..RECORD_A };
    let _ = SchemaRegistry::new(Version::new(1, 0), &[record], &[COMMAND_A], &[EVENT_A], &[]);
}

#[test]
#[should_panic(expected = "record field decimal scale/type mismatch")]
fn schema_registry_rejects_non_decimal_field_with_scale() {
    let record = RecordDefinition { fields: NON_DECIMAL_FIELDS_WITH_SCALE, ..RECORD_A };
    let _ = SchemaRegistry::new(Version::new(1, 0), &[record], &[COMMAND_A], &[EVENT_A], &[]);
}

#[test]
fn decimal_field_is_not_generic_unique_key_eligible() {
    let record = RecordDefinition {
        kind: 3,
        name: "DecimalRecord",
        data_size: 16,
        version: 1,
        unique_keys: DECIMAL_UK,
        canonical_indexes: &[],
        fields: DECIMAL_FIELDS_SCALE_2,
        reserved_fields: &[],
    };
    let registry = SchemaRegistry::new(Version::new(1, 0), &[record], &[], &[], &[]);

    assert!(!registry.supports_uk_encoding(record.kind));
    assert!(registry.encode_uk(record.kind, &[0u8; 16]).is_none());
}

#[test]
#[should_panic(expected = "unsupported or oversized key fields")]
fn decimal_field_is_rejected_from_canonical_index() {
    let record = RecordDefinition {
        kind: 3,
        name: "DecimalRecord",
        data_size: 16,
        version: 1,
        unique_keys: &[],
        canonical_indexes: DECIMAL_INDEX,
        fields: DECIMAL_FIELDS_SCALE_2,
        reserved_fields: &[],
    };
    let _ = SchemaRegistry::new(Version::new(1, 0), &[record], &[], &[], &[]);
}

#[test]
fn schema_registry_record_fingerprint_changes_with_canonical_indexes() {
    let first = SchemaRegistry::new(Version::new(1, 0), &[RECORD_A], &[COMMAND_A], &[EVENT_A], &[ENUM_A]);
    let with_index = RecordDefinition { canonical_indexes: RECORD_A_INDEX_BY_ID, ..RECORD_A };
    let renamed_index = RecordDefinition { canonical_indexes: RECORD_A_INDEX_BY_RENAMED_ID, ..RECORD_A };
    let second = SchemaRegistry::new(Version::new(1, 0), &[with_index], &[COMMAND_A], &[EVENT_A], &[ENUM_A]);
    let third = SchemaRegistry::new(Version::new(1, 0), &[renamed_index], &[COMMAND_A], &[EVENT_A], &[ENUM_A]);

    assert_ne!(first.record_schema_fingerprint(), second.record_schema_fingerprint());
    assert_ne!(second.record_schema_fingerprint(), third.record_schema_fingerprint());
}

#[test]
fn schema_registry_record_fingerprint_uses_explicit_canonical_index_ids() {
    let first = RecordDefinition { canonical_indexes: RECORD_A_TWO_INDEXES, ..RECORD_A };
    let second = RecordDefinition { canonical_indexes: RECORD_A_TWO_INDEXES_REORDERED, ..RECORD_A };
    let first_registry = SchemaRegistry::new(Version::new(1, 0), &[first], &[COMMAND_A], &[EVENT_A], &[ENUM_A]);
    let second_registry = SchemaRegistry::new(Version::new(1, 0), &[second], &[COMMAND_A], &[EVENT_A], &[ENUM_A]);

    assert_eq!(first_registry.record_schema_fingerprint(), second_registry.record_schema_fingerprint());
}

#[test]
fn canonical_index_generic_encoder_accepts_64_byte_key_boundary() {
    let record = RecordDefinition {
        kind: 3,
        name: "Fixed64",
        data_size: 66,
        version: 1,
        unique_keys: &[],
        canonical_indexes: FIXED_64_INDEX,
        fields: FIXED_64_FIELDS,
        reserved_fields: &[],
    };
    let registry = SchemaRegistry::new(Version::new(1, 0), &[record], &[], &[], &[]);
    let mut data = vec![0u8; 66];
    data[0..2].copy_from_slice(&(MAX_CANONICAL_INDEX_KEY_BYTES as u16).to_le_bytes());
    for (idx, byte) in data[2..].iter_mut().enumerate() {
        *byte = idx as u8;
    }

    let key = registry
        .encode_canonical_index(record.kind, 0, &data)
        .expect("encode canonical index");

    assert_eq!(key.len(), MAX_CANONICAL_INDEX_KEY_BYTES);
    assert_eq!(key.as_slice(), &data[2..66]);
}

#[test]
fn schema_registry_record_fingerprint_uses_explicit_unique_key_ids() {
    let first = RecordDefinition { unique_keys: RECORD_A_TWO_UKS, ..RECORD_A };
    let second = RecordDefinition { unique_keys: RECORD_A_TWO_UKS_REORDERED, ..RECORD_A };
    let first_registry = SchemaRegistry::new(Version::new(1, 0), &[first], &[COMMAND_A], &[EVENT_A], &[ENUM_A]);
    let second_registry = SchemaRegistry::new(Version::new(1, 0), &[second], &[COMMAND_A], &[EVENT_A], &[ENUM_A]);

    assert_eq!(first_registry.record_schema_fingerprint(), second_registry.record_schema_fingerprint());
}

#[test]
fn schema_registry_command_fingerprint_changes_with_fixed_size() {
    let first = SchemaRegistry::new(Version::new(1, 0), &[RECORD_A], &[COMMAND_A], &[EVENT_A], &[ENUM_A]);
    let second = SchemaRegistry::new(Version::new(1, 0), &[RECORD_A], &[COMMAND_A_ALT], &[EVENT_A], &[ENUM_A]);

    assert_ne!(first.command_schema_fingerprint(), second.command_schema_fingerprint());
}

#[test]
fn schema_registry_event_fingerprint_ignores_inline_response_flag() {
    let inline_event = EventDefinition { inline_response: true, ..EVENT_A };
    let first = SchemaRegistry::new(Version::new(1, 0), &[RECORD_A], &[COMMAND_A], &[EVENT_A], &[ENUM_A]);
    let second = SchemaRegistry::new(Version::new(1, 0), &[RECORD_A], &[COMMAND_A], &[inline_event], &[ENUM_A]);

    assert_eq!(first.event_schema_fingerprint(), second.event_schema_fingerprint());
}

#[test]
#[should_panic(expected = "record kind 0 is reserved and cannot be registered")]
fn schema_registry_rejects_record_kind_zero() {
    let _ = SchemaRegistry::new(Version::new(1, 0), &[RECORD_ZERO], &[], &[], &[]);
}

#[test]
fn schema_registry_accepts_max_user_record_kind() {
    let registry = SchemaRegistry::new(Version::new(1, 0), &[RECORD_USER_MAX], &[], &[], &[]);
    assert!(registry.try_get(crate::USER_KIND_MAX).is_some());
}

#[test]
#[should_panic(expected = "record kind 61440 is reserved for StateVec system metadata")]
fn schema_registry_rejects_system_record_kind() {
    let _ = SchemaRegistry::new(Version::new(1, 0), &[RECORD_SYSTEM_MIN], &[], &[], &[]);
}

#[test]
#[should_panic(expected = "duplicate record kind registered")]
fn schema_registry_rejects_duplicate_record_kind() {
    let _ = SchemaRegistry::new(Version::new(1, 0), &[RECORD_A, RECORD_A_WITH_RESERVED], &[], &[], &[]);
}

#[test]
#[should_panic(expected = "duplicate command kind registered")]
fn schema_registry_rejects_duplicate_command_kind() {
    let _ = SchemaRegistry::new(Version::new(1, 0), &[], &[COMMAND_A, COMMAND_A_ALT], &[], &[]);
}

#[test]
#[should_panic(expected = "command kind 0 is reserved and cannot be registered")]
fn schema_registry_rejects_command_kind_zero() {
    let _ = SchemaRegistry::new(Version::new(1, 0), &[], &[COMMAND_ZERO], &[], &[]);
}

#[test]
#[should_panic(expected = "command kind 61440 is reserved for StateVec system commands")]
fn schema_registry_rejects_system_command_kind() {
    let _ = SchemaRegistry::new(Version::new(1, 0), &[], &[COMMAND_SYSTEM_MIN], &[], &[]);
}

#[test]
#[should_panic(expected = "duplicate event kind registered")]
fn schema_registry_rejects_duplicate_event_kind() {
    let event_alt = EventDefinition {
        kind: EVENT_A.kind,
        name: "EvtAAlt",
        version: EVENT_A.version,
        fields: EVENT_A.fields,
        inline_response: false,
    };
    let _ = SchemaRegistry::new(Version::new(1, 0), &[], &[], &[EVENT_A, event_alt], &[]);
}

#[test]
#[should_panic(expected = "event kind 0 is reserved and cannot be registered")]
fn schema_registry_rejects_event_kind_zero() {
    let _ = SchemaRegistry::new(Version::new(1, 0), &[], &[], &[EVENT_ZERO], &[]);
}

#[test]
#[should_panic(expected = "event kind 61440 is reserved for StateVec system events")]
fn schema_registry_rejects_system_event_kind() {
    let _ = SchemaRegistry::new(Version::new(1, 0), &[], &[], &[EVENT_SYSTEM_MIN], &[]);
}

#[test]
#[should_panic(expected = "duplicate enum definition registered")]
fn schema_registry_rejects_duplicate_enum_name() {
    let _ = SchemaRegistry::new(Version::new(1, 0), &[], &[], &[], &[ENUM_A, ENUM_B]);
}
