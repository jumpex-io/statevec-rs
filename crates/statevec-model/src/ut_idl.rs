// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

use crate::idl::parse_fixed_bytes_capacity;
use crate::registry::SchemaRegistry;
use crate::{
    CommandDefinition, EnumDefinition, EnumVariantDefinition, EventDefinition, FieldDefinition,
    FieldType, PayloadFieldDefinition, PkBuilder, PkBytes, RecordDefinition, Version, read_u64_le,
};

fn encode_pk(data: &[u8]) -> PkBytes {
    let mut pk = PkBuilder::new();
    pk.push_u64(read_u64_le(data, 0).expect("test data should contain the u64 PK field"));
    pk.finish()
}

static TEST_ENUM: EnumDefinition = EnumDefinition {
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

static REC_FIELDS: &[FieldDefinition] = &[
    FieldDefinition {
        name: "id",
        field_index: 1,
        offset: 0,
        ty: FieldType::U64,
        len: 8,
        rust_type_name: "u64",
        enum_type_name: None,
        immutable: true,
    },
    FieldDefinition {
        name: "name",
        field_index: 2,
        offset: 8,
        ty: FieldType::FixedBytes,
        len: 18,
        rust_type_name: "FixedBytes < 16 >",
        enum_type_name: None,
        immutable: false,
    },
];

static REC_DEF: RecordDefinition = RecordDefinition {
    kind: 1,
    name: "TestRec",
    is_pk_idx: true,
    support_range_scan: false,
    data_size: 64,
    version: 1,
    pk_encode: Some(encode_pk),
    fields: REC_FIELDS,
    reserved_fields: &[],
    pk_fields: &["id"],
};

static CMD_FIELDS: &[PayloadFieldDefinition] = &[
    PayloadFieldDefinition {
        name: "value",
        field_index: 1,
        ty: FieldType::U64,
        rust_type_name: "u64",
        enum_type_name: None,
        fixed_size: Some(8),
    },
    PayloadFieldDefinition {
        name: "status",
        field_index: 2,
        ty: FieldType::EnumU8,
        rust_type_name: "Status",
        enum_type_name: Some("Status"),
        fixed_size: Some(1),
    },
];

static CMD_DEF: CommandDefinition = CommandDefinition {
    kind: 1,
    name: "SetValue",
    version: 1,
    fields: CMD_FIELDS,
};

static EVT_FIELDS: &[PayloadFieldDefinition] = &[PayloadFieldDefinition {
    name: "data",
    field_index: 1,
    ty: FieldType::VarBytes,
    rust_type_name: "VarBytes",
    enum_type_name: None,
    fixed_size: None,
}];

static EVT_DEF: EventDefinition = EventDefinition {
    kind: 1,
    name: "ValueSet",
    version: 1,
    fields: EVT_FIELDS,
};

#[test]
fn idl_json_contains_expected_structure() {
    let registry = SchemaRegistry::new(
        Version::new(1, 0),
        &[REC_DEF],
        &[CMD_DEF],
        &[EVT_DEF],
        &[TEST_ENUM],
    );
    let json = registry.to_idl_json();

    assert!(json.contains("\"idlVersion\": \"1.0\""));
    assert!(json.contains("\"main\": 1"));
    assert!(json.contains("\"minor\": 0"));
    assert!(json.contains("\"records\""));
    assert!(json.contains("\"commands\""));
    assert!(json.contains("\"events\""));
    assert!(json.contains("\"types\""));
    assert!(json.contains("\"name\": \"Status\""));
    assert!(json.contains("\"kind\": \"enumU8\""));
    assert!(json.contains("\"name\": \"Pending\""));
    assert!(json.contains("\"value\": 1"));
    assert!(json.contains("\"name\": \"Active\""));
    assert!(json.contains("\"value\": 2"));
    assert!(json.contains("\"name\": \"TestRec\""));
    assert!(json.contains("\"dataSize\": 64"));
    assert!(json.contains("\"hasPk\": true"));
    assert!(json.contains("\"pkFields\""));
    assert!(json.contains("\"immutable\": true"));
    assert!(json.contains("\"name\": \"SetValue\""));
    assert!(json.contains("{\"defined\": \"Status\"}"));
    assert!(json.contains("\"name\": \"ValueSet\""));
    assert!(json.contains("\"varBytes\""));
    assert!(json.contains("{\"fixedBytes\": 16}"));
    assert!(json.contains("\"fingerprints\""));
    assert!(json.contains("\"types\""));
}

#[test]
fn idl_json_empty_registry() {
    let registry = SchemaRegistry::new(Version::new(2, 3), &[], &[], &[], &[]);
    let json = registry.to_idl_json();
    assert!(json.contains("\"main\": 2"));
    assert!(json.contains("\"minor\": 3"));
    assert!(json.contains("\"records\": [\n  ]"));
    assert!(json.contains("\"commands\": [\n  ]"));
    assert!(json.contains("\"events\": [\n  ]"));
    assert!(json.contains("\"types\": [\n  ]"));
}

#[test]
fn parse_fixed_bytes_capacity_works() {
    assert_eq!(parse_fixed_bytes_capacity("FixedBytes<16>"), Some(16));
    assert_eq!(parse_fixed_bytes_capacity("FixedBytes < 32 >"), Some(32));
    assert_eq!(parse_fixed_bytes_capacity("u64"), None);
}

#[test]
fn idl_json_round_trips_into_host_owned_registry() {
    let registry = SchemaRegistry::new(
        Version::new(1, 0),
        &[REC_DEF],
        &[CMD_DEF],
        &[EVT_DEF],
        &[TEST_ENUM],
    );
    let json = registry.to_idl_json();
    let record_bytes = [
        0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, // id
        0x04, 0x00, b'T', b'E', b'S', b'T', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, // FixedBytes<16>
    ];

    let imported = SchemaRegistry::from_idl_json(&json).expect("schema IDL should round-trip");

    assert_eq!(imported.schema_version(), registry.schema_version());
    assert_eq!(
        imported.record_schema_fingerprint(),
        registry.record_schema_fingerprint()
    );
    assert_eq!(
        imported.command_schema_fingerprint(),
        registry.command_schema_fingerprint()
    );
    assert_eq!(
        imported.event_schema_fingerprint(),
        registry.event_schema_fingerprint()
    );
    assert_eq!(
        imported.types_schema_fingerprint(),
        registry.types_schema_fingerprint()
    );

    let record = imported
        .try_get(REC_DEF.kind)
        .expect("record definition should exist");
    assert_eq!(record.name, REC_DEF.name);
    assert_eq!(record.pk_fields, REC_DEF.pk_fields);
    assert_eq!(
        imported
            .encode_pk(REC_DEF.kind, &record_bytes)
            .expect("imported registry should provide generic PK encoding"),
        registry
            .encode_pk(REC_DEF.kind, &record_bytes)
            .expect("baseline registry should encode PK"),
    );
}
