// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

use crate::idl::{SchemaIdlError, parse_fixed_bytes_capacity};
use crate::registry::SchemaRegistry;
use crate::{
    CanonicalIndexDefinition, CommandDefinition, EnumDefinition, EnumVariantDefinition, EventDefinition,
    FieldDefinition, FieldType, KeyBuilder, KeyBytes, PayloadFieldDefinition, RecordDefinition, SemanticTag, Version,
    read_u64_le,
};

fn encode_uk(data: &[u8]) -> KeyBytes {
    let mut uk = KeyBuilder::new();
    uk.push_u64(read_u64_le(data, 0).expect("test data should contain the u64 UK field"));
    uk.finish()
}

static TEST_ENUM: EnumDefinition = EnumDefinition {
    name: "Status",
    variants: &[
        EnumVariantDefinition { name: "Pending", discriminant: 1 },
        EnumVariantDefinition { name: "Active", discriminant: 2 },
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
        decimal_scale: None,
        semantic: None,
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
        decimal_scale: None,
        semantic: None,
        immutable: false,
    },
];

static REC_INDEXES: &[CanonicalIndexDefinition] =
    &[CanonicalIndexDefinition { id: 0, name: "by_name", encode: None, fields: &["name"] }];

static REC_DEF: RecordDefinition = RecordDefinition {
    kind: 1,
    name: "TestRec",
    data_size: 64,
    version: 1,
    unique_keys: &[crate::UniqueKeyDefinition { id: 0, name: "", encode: Some(encode_uk), fields: &["id"] }],
    canonical_indexes: REC_INDEXES,
    fields: REC_FIELDS,
    reserved_fields: &[],
};

static RESERVED_FIELD: FieldDefinition = FieldDefinition {
    name: "future_status",
    field_index: 3,
    offset: 26,
    ty: FieldType::U8,
    len: 1,
    rust_type_name: "u8",
    enum_type_name: None,
    decimal_scale: None,
    semantic: None,
    immutable: false,
};

static REC_WITH_RESERVED_DEF: RecordDefinition = RecordDefinition { reserved_fields: &[RESERVED_FIELD], ..REC_DEF };

static CMD_FIELDS: &[PayloadFieldDefinition] = &[
    PayloadFieldDefinition {
        name: "value",
        field_index: 1,
        ty: FieldType::U64,
        rust_type_name: "u64",
        enum_type_name: None,
        decimal_scale: None,
        semantic: None,
        repeated: false,
        element_count_max: 0,
        fixed_size: Some(8),
    },
    PayloadFieldDefinition {
        name: "status",
        field_index: 2,
        ty: FieldType::EnumU8,
        rust_type_name: "Status",
        enum_type_name: Some("Status"),
        decimal_scale: None,
        semantic: None,
        repeated: false,
        element_count_max: 0,
        fixed_size: Some(1),
    },
];

static CMD_DEF: CommandDefinition = CommandDefinition { kind: 1, name: "SetValue", version: 1, fields: CMD_FIELDS };

static EVT_FIELDS: &[PayloadFieldDefinition] = &[PayloadFieldDefinition {
    name: "data",
    field_index: 1,
    ty: FieldType::VarBytes,
    rust_type_name: "VarBytes",
    enum_type_name: None,
    decimal_scale: None,
    semantic: None,
    repeated: false,
    element_count_max: 0,
    fixed_size: None,
}];

static EVT_DEF: EventDefinition =
    EventDefinition { kind: 1, name: "ValueSet", version: 1, fields: EVT_FIELDS, inline_response: true };

static DECIMAL_REC_FIELDS: &[FieldDefinition] = &[FieldDefinition {
    name: "balance",
    field_index: 1,
    offset: 0,
    ty: FieldType::Decimal,
    len: 16,
    rust_type_name: "Decimal<2>",
    enum_type_name: None,
    decimal_scale: Some(2),
    semantic: None,
    immutable: false,
}];

static DECIMAL_REC_DEF: RecordDefinition = RecordDefinition {
    kind: 2,
    name: "Balance",
    data_size: 16,
    version: 1,
    unique_keys: &[],
    canonical_indexes: &[],
    fields: DECIMAL_REC_FIELDS,
    reserved_fields: &[],
};

static DECIMAL_CMD_FIELDS: &[PayloadFieldDefinition] = &[PayloadFieldDefinition {
    name: "amount",
    field_index: 1,
    ty: FieldType::Decimal,
    rust_type_name: "Decimal<6>",
    enum_type_name: None,
    decimal_scale: Some(6),
    semantic: None,
    repeated: false,
    element_count_max: 0,
    fixed_size: Some(16),
}];

static DECIMAL_CMD_DEF: CommandDefinition =
    CommandDefinition { kind: 2, name: "SetAmount", version: 1, fields: DECIMAL_CMD_FIELDS };

static SEMANTIC_REC_FIELDS: &[FieldDefinition] = &[FieldDefinition {
    name: "created_at",
    field_index: 1,
    offset: 0,
    ty: FieldType::U64,
    len: 8,
    rust_type_name: "u64",
    enum_type_name: None,
    decimal_scale: None,
    semantic: Some(SemanticTag::TimestampMicros),
    immutable: false,
}];

static SEMANTIC_REC_DEF: RecordDefinition = RecordDefinition {
    kind: 3,
    name: "AuditRecord",
    data_size: 8,
    version: 1,
    unique_keys: &[],
    canonical_indexes: &[],
    fields: SEMANTIC_REC_FIELDS,
    reserved_fields: &[],
};

static PLAIN_SEMANTIC_REC_FIELDS: &[FieldDefinition] = &[FieldDefinition { semantic: None, ..SEMANTIC_REC_FIELDS[0] }];

static PLAIN_SEMANTIC_REC_DEF: RecordDefinition =
    RecordDefinition { fields: PLAIN_SEMANTIC_REC_FIELDS, ..SEMANTIC_REC_DEF };

static SEMANTIC_EVT_FIELDS: &[PayloadFieldDefinition] = &[PayloadFieldDefinition {
    name: "message",
    field_index: 1,
    ty: FieldType::VarBytes,
    rust_type_name: "VarBytes",
    enum_type_name: None,
    decimal_scale: None,
    semantic: Some(SemanticTag::Text),
    repeated: false,
    element_count_max: 0,
    fixed_size: None,
}];

static SEMANTIC_EVT_DEF: EventDefinition = EventDefinition {
    kind: 3,
    name: "MessageEmitted",
    version: 1,
    fields: SEMANTIC_EVT_FIELDS,
    inline_response: false,
};

static PLAIN_SEMANTIC_EVT_FIELDS: &[PayloadFieldDefinition] =
    &[PayloadFieldDefinition { semantic: None, ..SEMANTIC_EVT_FIELDS[0] }];

static PLAIN_SEMANTIC_EVT_DEF: EventDefinition =
    EventDefinition { fields: PLAIN_SEMANTIC_EVT_FIELDS, ..SEMANTIC_EVT_DEF };

static REPEATED_EVT_FIELDS: &[PayloadFieldDefinition] = &[PayloadFieldDefinition {
    name: "ids",
    field_index: 1,
    ty: FieldType::U64,
    rust_type_name: "u64",
    enum_type_name: None,
    decimal_scale: None,
    semantic: None,
    repeated: true,
    element_count_max: 16,
    fixed_size: Some(8),
}];

static REPEATED_EVT_DEF: EventDefinition =
    EventDefinition { kind: 4, name: "IdsEmitted", version: 1, fields: REPEATED_EVT_FIELDS, inline_response: false };

#[test]
fn idl_json_contains_expected_structure() {
    let registry = SchemaRegistry::new(Version::new(1, 0), &[REC_DEF], &[CMD_DEF], &[EVT_DEF], &[TEST_ENUM]);
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
    assert!(json.contains("\"uniqueKeys\""));
    assert!(json.contains("\"id\": 0"));
    assert!(json.contains("\"canonicalIndexes\""));
    assert!(json.contains("\"name\": \"by_name\""));
    assert!(json.contains("\"immutable\": true"));
    assert!(json.contains("\"name\": \"SetValue\""));
    assert!(json.contains("{\"defined\": \"Status\"}"));
    assert!(json.contains("\"name\": \"ValueSet\""));
    assert!(json.contains("\"inlineResponse\": true"));
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
fn canonical_idl_bytes_are_compact_and_hashable() {
    let registry = SchemaRegistry::new(Version::new(1, 0), &[REC_DEF], &[CMD_DEF], &[EVT_DEF], &[TEST_ENUM]);
    let canonical = registry.to_canonical_idl_bytes();
    let value: serde_json::Value = serde_json::from_slice(&canonical).expect("canonical IDL should be JSON");

    assert!(!canonical.contains(&b'\n'));
    assert_eq!(value["idlVersion"], "1.0");

    let first = registry.canonical_idl_sha256();
    let second = registry.canonical_idl_sha256();
    assert_eq!(first, second);
    assert_ne!(first, [0u8; 32]);
}

#[test]
fn canonical_idl_bytes_are_stable_across_definition_order() {
    let first = SchemaRegistry::new(Version::new(1, 0), &[REC_DEF], &[CMD_DEF], &[EVT_DEF], &[TEST_ENUM]);
    let second = SchemaRegistry::new(Version::new(1, 0), &[REC_DEF], &[CMD_DEF], &[EVT_DEF], &[TEST_ENUM]);

    assert_eq!(first.to_canonical_idl_bytes(), second.to_canonical_idl_bytes());
    assert_eq!(first.canonical_idl_sha256(), second.canonical_idl_sha256());
}

#[test]
fn canonical_idl_bytes_ignore_inline_response_projection_metadata() {
    let inline_event = EventDefinition { inline_response: true, ..EVT_DEF };
    let plain_event = EventDefinition { inline_response: false, ..EVT_DEF };
    let inline = SchemaRegistry::new(Version::new(1, 0), &[REC_DEF], &[CMD_DEF], &[inline_event], &[TEST_ENUM]);
    let plain = SchemaRegistry::new(Version::new(1, 0), &[REC_DEF], &[CMD_DEF], &[plain_event], &[TEST_ENUM]);

    assert_ne!(inline.to_idl_json(), plain.to_idl_json());
    assert_eq!(inline.to_canonical_idl_bytes(), plain.to_canonical_idl_bytes());
    assert_eq!(inline.canonical_idl_sha256(), plain.canonical_idl_sha256());
}

#[test]
fn idl_json_preserves_semantic_tags_but_canonical_hash_ignores_them() {
    let semantic = SchemaRegistry::new(Version::new(1, 0), &[SEMANTIC_REC_DEF], &[], &[SEMANTIC_EVT_DEF], &[]);
    let plain = SchemaRegistry::new(Version::new(1, 0), &[PLAIN_SEMANTIC_REC_DEF], &[], &[PLAIN_SEMANTIC_EVT_DEF], &[]);

    let json = semantic.to_idl_json();
    assert!(json.contains("\"semantic\": \"timestampMicros\""));
    assert!(json.contains("\"semantic\": \"text\""));

    let imported = SchemaRegistry::from_idl_json(&json).expect("semantic IDL should parse");
    assert_eq!(imported.try_get(SEMANTIC_REC_DEF.kind).unwrap().fields[0].semantic, Some(SemanticTag::TimestampMicros));
    assert_eq!(imported.try_get_event(SEMANTIC_EVT_DEF.kind).unwrap().fields[0].semantic, Some(SemanticTag::Text));

    assert_ne!(semantic.to_idl_json(), plain.to_idl_json());
    assert_eq!(semantic.to_canonical_idl_bytes(), plain.to_canonical_idl_bytes());
    assert_eq!(semantic.canonical_idl_sha256(), plain.canonical_idl_sha256());
}

#[test]
fn reserved_semantic_tags_are_display_metadata_not_canonical_schema() {
    let semantic_record = RecordDefinition { fields: &[], reserved_fields: SEMANTIC_REC_FIELDS, ..SEMANTIC_REC_DEF };
    let plain_record = RecordDefinition { fields: &[], reserved_fields: PLAIN_SEMANTIC_REC_FIELDS, ..SEMANTIC_REC_DEF };
    let semantic = SchemaRegistry::with_records(Version::new(1, 0), &[semantic_record]);
    let plain = SchemaRegistry::with_records(Version::new(1, 0), &[plain_record]);

    let display = semantic.to_idl_json();
    assert_ne!(display, plain.to_idl_json());
    let imported = SchemaRegistry::from_idl_json(&display).expect("display IDL");
    assert_eq!(
        imported.try_get(SEMANTIC_REC_DEF.kind).unwrap().reserved_fields[0].semantic,
        Some(SemanticTag::TimestampMicros)
    );
    assert_eq!(semantic.identity(), plain.identity());
    let canonical = SchemaRegistry::from_idl_bytes(&semantic.to_canonical_idl_bytes()).expect("canonical IDL");
    let record = canonical.try_get(SEMANTIC_REC_DEF.kind).unwrap();
    assert!(record.fields.is_empty(), "canonicalization cannot activate reserved storage");
    assert_eq!(record.reserved_fields, PLAIN_SEMANTIC_REC_FIELDS, "only the display tag may be removed");
    assert_eq!(semantic.canonical_idl_sha256(), plain.canonical_idl_sha256());
    assert!(
        semantic.to_canonical_idl_bytes() == plain.to_canonical_idl_bytes(),
        "display-only tags cannot change canonical IDL bytes"
    );
}

#[test]
fn idl_json_rejects_illegal_semantic_type_pairing() {
    let registry = SchemaRegistry::new(Version::new(1, 0), &[REC_DEF], &[], &[], &[]);
    let mut value: serde_json::Value = serde_json::from_str(&registry.to_idl_json()).unwrap();
    value["records"][0]["fields"][0]["semantic"] = serde_json::Value::String("text".to_string());
    let json = serde_json::to_string(&value).unwrap();

    let err = SchemaRegistry::from_idl_json(&json).expect_err("text semantic on u64 should be rejected");
    assert!(matches!(
        err,
        SchemaIdlError::InvalidSchema(message)
            if message.contains("semantic text is not valid")
    ));
}

#[test]
fn idl_json_round_trips_event_repeated_metadata() {
    let registry = SchemaRegistry::new(Version::new(1, 0), &[], &[], &[REPEATED_EVT_DEF], &[]);
    let json = registry.to_idl_json();
    assert!(json.contains("\"repeated\": {\"type\": \"u64\", \"max\": 16}"));

    let imported = SchemaRegistry::from_idl_json(&json).expect("repeated event IDL should parse");
    let field = &imported.try_get_event(REPEATED_EVT_DEF.kind).unwrap().fields[0];
    assert_eq!(field.ty, FieldType::U64);
    assert!(field.repeated);
    assert_eq!(field.element_count_max, 16);
    assert_eq!(field.fixed_size, Some(8));
    assert_eq!(imported.event_schema_fingerprint(), registry.event_schema_fingerprint());
}

#[test]
fn idl_json_rejects_repeated_command_payload_fields() {
    let registry = SchemaRegistry::new(Version::new(1, 0), &[], &[CMD_DEF], &[], &[]);
    let mut value: serde_json::Value = serde_json::from_str(&registry.to_idl_json()).unwrap();
    value["commands"][0]["fields"][0]["type"] = serde_json::json!({
        "repeated": { "type": "u64", "max": 8 }
    });
    let json = serde_json::to_string(&value).unwrap();

    let err = SchemaRegistry::from_idl_json(&json).expect_err("repeated command payload should be rejected");
    assert!(matches!(
        err,
        SchemaIdlError::InvalidSchema(message)
            if message.contains("repeated command payload fields are not supported")
    ));
}

#[test]
fn idl_json_rejects_inline_response_on_commands() {
    let registry = SchemaRegistry::new(Version::new(1, 0), &[REC_DEF], &[CMD_DEF], &[EVT_DEF], &[TEST_ENUM]);
    let mut value: serde_json::Value = serde_json::from_str(&registry.to_idl_json()).unwrap();
    value["commands"][0]["inlineResponse"] = serde_json::Value::Bool(true);
    let json = serde_json::to_string(&value).unwrap();

    let err = SchemaRegistry::from_idl_json(&json).expect_err("inlineResponse on commands should be rejected");
    assert!(matches!(
        err,
        SchemaIdlError::InvalidSchema(message)
            if message.contains("inlineResponse is only supported on event definitions")
    ));
}

#[test]
fn idl_json_decimal_round_trips_with_scale_metadata() {
    let registry = SchemaRegistry::new(Version::new(1, 0), &[DECIMAL_REC_DEF], &[DECIMAL_CMD_DEF], &[], &[]);
    let json = registry.to_idl_json();

    assert!(json.contains("\"decimal\": {\"scale\": 2}"));
    assert!(json.contains("\"decimal\": {\"scale\": 6}"));

    let imported = SchemaRegistry::from_idl_json(&json).expect("decimal IDL should round-trip");
    assert_eq!(imported.record_schema_fingerprint(), registry.record_schema_fingerprint());
    assert_eq!(imported.command_schema_fingerprint(), registry.command_schema_fingerprint());

    let rec = imported.try_get(DECIMAL_REC_DEF.kind).unwrap();
    assert_eq!(rec.fields[0].ty, FieldType::Decimal);
    assert_eq!(rec.fields[0].decimal_scale, Some(2));
    assert_eq!(rec.fields[0].len, 16);

    let cmd = imported.try_get_command(DECIMAL_CMD_DEF.kind).unwrap();
    assert_eq!(cmd.fields[0].ty, FieldType::Decimal);
    assert_eq!(cmd.fields[0].decimal_scale, Some(6));
    assert_eq!(cmd.fields[0].fixed_size, Some(16));
}

#[test]
fn idl_json_rejects_unknown_field_type_object() {
    let registry = SchemaRegistry::new(Version::new(1, 0), &[DECIMAL_REC_DEF], &[], &[], &[]);
    let mut value: serde_json::Value = serde_json::from_str(&registry.to_idl_json()).unwrap();
    value["records"][0]["fields"][0]["type"] = serde_json::json!({ "decimal": { "scale": 2 }, "extra": true });
    let json = serde_json::to_string(&value).unwrap();

    let err = SchemaRegistry::from_idl_json(&json).expect_err("unknown field type object should be rejected");
    assert!(matches!(err, SchemaIdlError::Json(_) | SchemaIdlError::InvalidSchema(_)));
}

#[test]
fn parse_fixed_bytes_capacity_works() {
    assert_eq!(parse_fixed_bytes_capacity("FixedBytes<16>"), Some(16));
    assert_eq!(parse_fixed_bytes_capacity("FixedBytes < 32 >"), Some(32));
    assert_eq!(parse_fixed_bytes_capacity("u64"), None);
}

#[test]
fn idl_json_round_trips_into_host_owned_registry() {
    let registry = SchemaRegistry::new(Version::new(1, 0), &[REC_DEF], &[CMD_DEF], &[EVT_DEF], &[TEST_ENUM]);
    let json = registry.to_idl_json();
    let record_bytes = [
        0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, // id
        0x04, 0x00, b'T', b'E', b'S', b'T', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // FixedBytes<16>
    ];

    let imported = SchemaRegistry::from_idl_json(&json).expect("schema IDL should round-trip");

    assert_eq!(imported.schema_version(), registry.schema_version());
    assert_eq!(imported.record_schema_fingerprint(), registry.record_schema_fingerprint());
    assert_eq!(imported.command_schema_fingerprint(), registry.command_schema_fingerprint());
    assert_eq!(imported.event_schema_fingerprint(), registry.event_schema_fingerprint());
    assert_eq!(imported.types_schema_fingerprint(), registry.types_schema_fingerprint());

    let record = imported.try_get(REC_DEF.kind).expect("record definition should exist");
    assert_eq!(record.name, REC_DEF.name);
    assert_eq!(record.unique_keys.len(), 1);
    assert_eq!(record.unique_keys[0].fields, &["id"]);
    assert_eq!(record.canonical_indexes.len(), 1);
    assert_eq!(record.canonical_indexes[0].name, "by_name");
    assert_eq!(record.canonical_indexes[0].fields, &["name"]);
    assert_eq!(
        imported
            .encode_uk(REC_DEF.kind, &record_bytes)
            .expect("imported registry should provide generic UK encoding"),
        registry
            .encode_uk(REC_DEF.kind, &record_bytes)
            .expect("baseline registry should encode UK"),
    );
    assert_eq!(
        imported.encode_canonical_index(REC_DEF.kind, 0, &record_bytes).unwrap(),
        registry.encode_canonical_index(REC_DEF.kind, 0, &record_bytes).unwrap(),
    );
    assert!(imported.try_get_event(EVT_DEF.kind).unwrap().inline_response);
}

#[test_case::test_case("/records/0/fields/0/type", serde_json::json!("i64"); "record_type")]
#[test_case::test_case("/commands/0/fields/0/type", serde_json::json!("i64"); "command_type")]
#[test_case::test_case("/events/0/fields/0/name", serde_json::json!("changed_field"); "event_field")]
#[test_case::test_case("/types/0/variants/0/value", serde_json::json!(3); "enum_discriminant")]
fn idl_rejects_changed_definitions_with_stale_fingerprints(pointer: &str, replacement: serde_json::Value) {
    let registry = SchemaRegistry::new(Version::new(1, 0), &[REC_DEF], &[CMD_DEF], &[EVT_DEF], &[TEST_ENUM]);
    let mut document: serde_json::Value = serde_json::from_str(&registry.to_idl_json()).unwrap();
    *document.pointer_mut(pointer).unwrap() = replacement;

    let error = SchemaRegistry::from_idl_json(&document.to_string()).unwrap_err();
    assert!(
        matches!(error, SchemaIdlError::FingerprintMismatch { declared, computed }
            if declared == registry.identity() && computed != declared),
        "{pointer}: changed definitions must not inherit the supplied old fingerprint: {error:?}"
    );
}

#[test]
fn canonical_idl_round_trip_preserves_reserved_field_roles() {
    let registry = SchemaRegistry::with_records(Version::new(1, 0), &[REC_WITH_RESERVED_DEF]);
    let decoded = SchemaRegistry::from_idl_bytes(&registry.to_canonical_idl_bytes()).unwrap();
    let record = decoded.try_get(1).unwrap();

    assert_eq!(record.fields.len(), REC_FIELDS.len());
    assert_eq!(record.reserved_fields.len(), 1);
    assert_eq!(record.reserved_fields[0].name, "future_status");
    assert_eq!(record.reserved_fields[0].field_index, 3);
}

#[test_case::test_case("records"; "record_kind")]
#[test_case::test_case("commands"; "command_kind")]
#[test_case::test_case("events"; "event_kind")]
#[test_case::test_case("types"; "enum_name")]
fn idl_duplicate_definitions_return_invalid_schema(section: &str) {
    let registry = SchemaRegistry::new(Version::new(1, 0), &[REC_DEF], &[CMD_DEF], &[EVT_DEF], &[TEST_ENUM]);
    let original = registry.to_idl_json();
    let mut document: serde_json::Value = serde_json::from_str(&original).unwrap();
    let definitions = document[section].as_array_mut().unwrap();
    definitions.push(definitions[0].clone());

    let result = SchemaRegistry::from_idl_json(&document.to_string());

    assert!(matches!(result, Err(SchemaIdlError::InvalidSchema(_))), "{section}: {result:?}");
    assert_eq!(SchemaRegistry::from_idl_json(&original).unwrap().identity(), registry.identity());
}

#[test_case::test_case("/records/0/kind", serde_json::json!(0); "zero_record")]
#[test_case::test_case("/records/0/kind", serde_json::json!(crate::SYSTEM_KIND_MIN); "system_record")]
#[test_case::test_case("/commands/0/kind", serde_json::json!(0); "zero_command")]
#[test_case::test_case("/commands/0/kind", serde_json::json!(crate::SYSTEM_KIND_MIN); "system_command")]
#[test_case::test_case("/events/0/kind", serde_json::json!(0); "zero_event")]
#[test_case::test_case("/events/0/kind", serde_json::json!(crate::SYSTEM_KIND_MIN); "system_event")]
#[test_case::test_case("/records/0/uniqueKeys/0/id", serde_json::json!(1); "unique_key_id_gap")]
#[test_case::test_case("/records/0/uniqueKeys/0/id", serde_json::json!(crate::MAX_UNIQUE_KEYS_PER_RECORD); "unique_key_id_limit")]
#[test_case::test_case("/records/0/canonicalIndexes/0/id", serde_json::json!(1); "index_id_gap")]
#[test_case::test_case("/records/0/canonicalIndexes/0/id", serde_json::json!(crate::MAX_CANONICAL_INDEXES_PER_RECORD); "index_id_limit")]
#[test_case::test_case("/records/0/canonicalIndexes/0/name", serde_json::json!(""); "empty_index_name")]
#[test_case::test_case("/records/0/canonicalIndexes/0/fields", serde_json::json!(["absent"]); "unknown_index_field")]
fn idl_invalid_registration_returns_invalid_schema(pointer: &str, replacement: serde_json::Value) {
    let registry = SchemaRegistry::new(Version::new(1, 0), &[REC_DEF], &[CMD_DEF], &[EVT_DEF], &[TEST_ENUM]);
    let original = registry.to_idl_json();
    let mut document: serde_json::Value = serde_json::from_str(&original).unwrap();
    *document.pointer_mut(pointer).unwrap() = replacement;

    let result = SchemaRegistry::from_idl_bytes(document.to_string().as_bytes());

    assert!(matches!(result, Err(SchemaIdlError::InvalidSchema(_))), "{pointer}: {result:?}");
    assert_eq!(SchemaRegistry::from_idl_json(&original).unwrap().identity(), registry.identity());
}
