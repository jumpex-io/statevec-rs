// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

use crate::registry::SchemaRegistry;
use crate::{
    CommandDefinition, EnumDefinition, EnumVariantDefinition, EventDefinition, FieldDefinition,
    FieldType, PayloadFieldDefinition, PkBytes, RecordDefinition, Version,
};

fn encode_pk(data: &[u8]) -> PkBytes {
    let mut pk = PkBytes::new();
    pk.extend_from_slice(&data[..8]);
    pk
}

static RECORD_FIELDS_A: &[FieldDefinition] = &[FieldDefinition {
    name: "id",
    field_index: 1,
    offset: 0,
    ty: FieldType::U64,
    len: 8,
    rust_type_name: "u64",
    enum_type_name: None,
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
    immutable: false,
}];
static COMMAND_FIELDS: &[PayloadFieldDefinition] = &[PayloadFieldDefinition {
    name: "amount",
    field_index: 1,
    ty: FieldType::U64,
    rust_type_name: "u64",
    enum_type_name: None,
    fixed_size: Some(8),
}];
static COMMAND_FIELDS_ALT: &[PayloadFieldDefinition] = &[PayloadFieldDefinition {
    name: "amount",
    field_index: 1,
    ty: FieldType::U64,
    rust_type_name: "u64",
    enum_type_name: None,
    fixed_size: Some(16),
}];
static EVENT_FIELDS: &[PayloadFieldDefinition] = &[PayloadFieldDefinition {
    name: "status",
    field_index: 1,
    ty: FieldType::U8,
    rust_type_name: "u8",
    enum_type_name: None,
    fixed_size: Some(1),
}];

static RECORD_A: RecordDefinition = RecordDefinition {
    kind: 1,
    name: "A",
    is_pk_idx: true,
    support_range_scan: true,
    data_size: 16,
    version: 1,
    pk_encode: Some(encode_pk),
    fields: RECORD_FIELDS_A,
    reserved_fields: &[],
    pk_fields: &["id"],
};
static RECORD_B: RecordDefinition = RecordDefinition {
    kind: 2,
    name: "B",
    is_pk_idx: false,
    support_range_scan: false,
    data_size: 16,
    version: 1,
    pk_encode: None,
    fields: RECORD_FIELDS_B,
    reserved_fields: &[],
    pk_fields: &[],
};
static RECORD_A_WITH_RESERVED: RecordDefinition = RecordDefinition {
    kind: 1,
    name: "A",
    is_pk_idx: true,
    support_range_scan: true,
    data_size: 16,
    version: 1,
    pk_encode: Some(encode_pk),
    fields: RECORD_FIELDS_A,
    reserved_fields: RESERVED_RECORD_FIELDS,
    pk_fields: &["id"],
};
static RECORD_ZERO: RecordDefinition = RecordDefinition {
    kind: 0,
    name: "Zero",
    is_pk_idx: false,
    support_range_scan: false,
    data_size: 16,
    version: 1,
    pk_encode: None,
    fields: RECORD_FIELDS_A,
    reserved_fields: &[],
    pk_fields: &[],
};
static COMMAND_A: CommandDefinition = CommandDefinition {
    kind: 10,
    name: "CmdA",
    version: 1,
    fields: COMMAND_FIELDS,
};
static COMMAND_A_ALT: CommandDefinition = CommandDefinition {
    kind: 10,
    name: "CmdA",
    version: 1,
    fields: COMMAND_FIELDS_ALT,
};
static EVENT_A: EventDefinition = EventDefinition {
    kind: 20,
    name: "EvtA",
    version: 1,
    fields: EVENT_FIELDS,
};
static ENUM_A: EnumDefinition = EnumDefinition {
    name: "Status",
    variants: &[
        EnumVariantDefinition {
            name: "Pending",
            discriminant: 1,
        },
        EnumVariantDefinition {
            name: "Active",
            discriminant: 2,
        },
    ],
};
static ENUM_B: EnumDefinition = EnumDefinition {
    name: "Status",
    variants: &[
        EnumVariantDefinition {
            name: "Pending",
            discriminant: 1,
        },
        EnumVariantDefinition {
            name: "Closed",
            discriminant: 3,
        },
    ],
};

#[test]
fn schema_registry_fingerprints_are_stable_across_definition_order() {
    let first = SchemaRegistry::new(
        Version::new(1, 0),
        &[RECORD_A, RECORD_B],
        &[COMMAND_A],
        &[EVENT_A],
        &[ENUM_A],
    );
    let second = SchemaRegistry::new(
        Version::new(1, 0),
        &[RECORD_B, RECORD_A],
        &[COMMAND_A],
        &[EVENT_A],
        &[ENUM_A],
    );

    assert_eq!(first.schema_version(), Version::new(1, 0));
    assert_eq!(
        first.record_schema_fingerprint(),
        second.record_schema_fingerprint()
    );
    assert_eq!(
        first.command_schema_fingerprint(),
        second.command_schema_fingerprint()
    );
    assert_eq!(
        first.event_schema_fingerprint(),
        second.event_schema_fingerprint()
    );
    assert_eq!(
        first.types_schema_fingerprint(),
        second.types_schema_fingerprint()
    );
    assert_eq!(first.try_get_command(10).unwrap().name, "CmdA");
    assert_eq!(first.try_get_event(20).unwrap().name, "EvtA");
}

#[test]
fn schema_registry_types_fingerprint_changes_with_enum_definition() {
    let first = SchemaRegistry::new(
        Version::new(1, 0),
        &[RECORD_A],
        &[COMMAND_A],
        &[EVENT_A],
        &[ENUM_A],
    );
    let second = SchemaRegistry::new(
        Version::new(1, 0),
        &[RECORD_A],
        &[COMMAND_A],
        &[EVENT_A],
        &[ENUM_B],
    );

    assert_ne!(
        first.types_schema_fingerprint(),
        second.types_schema_fingerprint()
    );
}

#[test]
fn schema_registry_record_fingerprint_changes_with_reserved_fields() {
    let first = SchemaRegistry::new(
        Version::new(1, 0),
        &[RECORD_A],
        &[COMMAND_A],
        &[EVENT_A],
        &[ENUM_A],
    );
    let second = SchemaRegistry::new(
        Version::new(1, 0),
        &[RECORD_A_WITH_RESERVED],
        &[COMMAND_A],
        &[EVENT_A],
        &[ENUM_A],
    );

    assert_ne!(
        first.record_schema_fingerprint(),
        second.record_schema_fingerprint()
    );
}

#[test]
fn schema_registry_command_fingerprint_changes_with_fixed_size() {
    let first = SchemaRegistry::new(
        Version::new(1, 0),
        &[RECORD_A],
        &[COMMAND_A],
        &[EVENT_A],
        &[ENUM_A],
    );
    let second = SchemaRegistry::new(
        Version::new(1, 0),
        &[RECORD_A],
        &[COMMAND_A_ALT],
        &[EVENT_A],
        &[ENUM_A],
    );

    assert_ne!(
        first.command_schema_fingerprint(),
        second.command_schema_fingerprint()
    );
}

#[test]
#[should_panic(expected = "record kind 0 is reserved and cannot be registered")]
fn schema_registry_rejects_record_kind_zero() {
    let _ = SchemaRegistry::new(Version::new(1, 0), &[RECORD_ZERO], &[], &[], &[]);
}

#[test]
#[should_panic(expected = "duplicate record kind registered")]
fn schema_registry_rejects_duplicate_record_kind() {
    let _ = SchemaRegistry::new(
        Version::new(1, 0),
        &[RECORD_A, RECORD_A_WITH_RESERVED],
        &[],
        &[],
        &[],
    );
}

#[test]
#[should_panic(expected = "duplicate command kind registered")]
fn schema_registry_rejects_duplicate_command_kind() {
    let _ = SchemaRegistry::new(
        Version::new(1, 0),
        &[],
        &[COMMAND_A, COMMAND_A_ALT],
        &[],
        &[],
    );
}

#[test]
#[should_panic(expected = "duplicate event kind registered")]
fn schema_registry_rejects_duplicate_event_kind() {
    let event_alt = EventDefinition {
        kind: EVENT_A.kind,
        name: "EvtAAlt",
        version: EVENT_A.version,
        fields: EVENT_A.fields,
    };
    let _ = SchemaRegistry::new(Version::new(1, 0), &[], &[], &[EVENT_A, event_alt], &[]);
}

#[test]
#[should_panic(expected = "duplicate enum definition registered")]
fn schema_registry_rejects_duplicate_enum_name() {
    let _ = SchemaRegistry::new(Version::new(1, 0), &[], &[], &[], &[ENUM_A, ENUM_B]);
}
