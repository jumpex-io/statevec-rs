use crate::{
    FieldDefinition, FieldType, RecordDefinition, RecordLayoutError, ReservedBytesZeroError, SchemaRegistry, Version,
    ZeroSpan,
};
use test_case::test_case;

const ID: FieldDefinition = FieldDefinition {
    name: "id",
    field_index: 1,
    offset: 0,
    ty: FieldType::U64,
    len: 8,
    rust_type_name: "u64",
    enum_type_name: None,
    decimal_scale: None,
    semantic: None,
    immutable: false,
};
const GAP_RECORD: RecordDefinition = RecordDefinition {
    kind: 91,
    name: "Gaps",
    data_size: 40,
    version: 1,
    unique_keys: &[],
    canonical_indexes: &[],
    fields: &[
        ID,
        FieldDefinition { name: "value", field_index: 2, offset: 16, ..ID },
        FieldDefinition {
            name: "tag",
            field_index: 3,
            offset: 32,
            ty: FieldType::FixedBytes,
            len: 4,
            rust_type_name: "FixedBytes<2>",
            ..ID
        },
    ],
    reserved_fields: &[FieldDefinition { name: "future", field_index: 4, offset: 24, ..ID }],
};

#[test_case(false; "static registry")]
#[test_case(true; "bounded IDL registry")]
fn checked_registry_excludes_retired_bytes_from_zero_spans_without_tightening_fixed_bytes(idl: bool) {
    let registry = SchemaRegistry::with_records(Version::new(1, 0), &[GAP_RECORD]);
    let registry = if idl {
        SchemaRegistry::from_idl_bytes_with_limits(
            &registry.to_canonical_idl_bytes(),
            crate::idl_limits::SchemaIdlLimits::default(),
        )
        .unwrap()
    } else {
        registry
    };
    let definition = registry.try_get(91).unwrap();
    assert_eq!(registry.validate_record_unused_bytes(92, &[]), None);
    assert!(matches!(
        registry.validate_record_unused_bytes(91, &[]),
        Some(Err(ReservedBytesZeroError::RecordDataTooShort { required: 40, actual: 0, .. }))
    ));
    assert_eq!(
        registry.record_zero_spans(91),
        Some(&[ZeroSpan { offset: 8, len: 8 }, ZeroSpan { offset: 36, len: 4 }][..])
    );
    let mut bytes = [0; 48];
    bytes[0] = 7;
    bytes[16] = 9;
    bytes[24..32].fill(0xa5); // Historical retired values are opaque, not zero storage.
    bytes[34] = 0xaa; // FixedBytes logical length is zero; all declared bytes remain active.
    assert_eq!(
        registry
            .validate_record_unused_bytes(definition.kind, &bytes)
            .expect("registered record kind"),
        Ok(())
    );
    assert_eq!(registry.validate_record_values(definition, &bytes), Ok(()));
    for (offset, expected) in [
        (8, ReservedBytesZeroError::UnusedSpanNonZero { kind: 91, offset: 8, len: 8 }),
        (15, ReservedBytesZeroError::UnusedSpanNonZero { kind: 91, offset: 8, len: 8 }),
        (39, ReservedBytesZeroError::UnusedSpanNonZero { kind: 91, offset: 36, len: 4 }),
        (40, ReservedBytesZeroError::PayloadTailNonZero { kind: 91, data_size: 40, payload_len: 48 }),
    ] {
        bytes[offset] = 1;
        assert_eq!(
            registry
                .validate_record_unused_bytes(definition.kind, &bytes)
                .expect("registered record kind"),
            Err(expected),
            "offset {offset}, idl {idl}"
        );
        bytes[offset] = 0;
    }
    bytes[8] = 1;
    bytes[24] = 1;
    bytes[40] = 1;
    assert!(
        matches!(
            registry
                .validate_record_unused_bytes(definition.kind, &bytes)
                .expect("registered record kind"),
            Err(ReservedBytesZeroError::PayloadTailNonZero { .. })
        ),
        "physical tail precedes unused spans"
    );
    bytes[40] = 0;
    assert!(
        matches!(
            registry
                .validate_record_unused_bytes(definition.kind, &bytes)
                .expect("registered record kind"),
            Err(ReservedBytesZeroError::UnusedSpanNonZero { offset: 8, len: 8, .. })
        ),
        "opaque retired bytes do not mask an invalid gap"
    );
}

#[test_case(4, 8, false; "active overlap")]
#[test_case(4, 8, true; "reserved active overlap")]
#[test_case(39, 8, false; "active out of bounds")]
#[test_case(u32::MAX, 8, false; "active end overflow")]
#[test_case(39, 8, true; "reserved out of bounds")]
#[test_case(u32::MAX, 8, true; "reserved end overflow")]
fn malformed_intervals_refuse_at_registry_construction(offset: u32, len: u32, reserved: bool) {
    let field = FieldDefinition { name: "other", field_index: 2, offset, len, ..ID };
    let fields = Box::leak(Box::new([ID, field]));
    let reserved_fields = Box::leak(Box::new([field]));
    let definition = RecordDefinition {
        fields: if reserved { &[ID] } else { fields },
        reserved_fields: if reserved { reserved_fields } else { &[] },
        ..GAP_RECORD
    };
    let expected = if offset == 4 {
        RecordLayoutError::Overlap {
            kind: 91,
            first_field: 1,
            second_field: 2,
            first_reserved: false,
            second_reserved: reserved,
        }
    } else {
        RecordLayoutError::SpanOutOfBounds { kind: 91, field_index: 2, reserved, offset, len, data_size: 40 }
    };
    let error = SchemaRegistry::try_new(Version::new(1, 0), &[definition], &[], &[], &[]).unwrap_err();
    assert_eq!(error, crate::idl::SchemaIdlError::RecordLayout(expected));
}

#[test_case(false, false; "duplicate active id")]
#[test_case(true, false; "duplicate active name")]
#[test_case(false, true; "reserved id remains occupied")]
#[test_case(true, true; "reserved name remains occupied")]
fn identifiers_are_unique_across_active_and_reserved_fields(name: bool, reserved: bool) {
    let mut active = GAP_RECORD.fields.to_vec();
    let mut retired = GAP_RECORD.reserved_fields.to_vec();
    let field = if reserved { &mut retired[0] } else { &mut active[1] };
    let expected = if name {
        field.name = ID.name;
        RecordLayoutError::DuplicateFieldName { kind: 91, name: ID.name.to_owned() }
    } else {
        field.field_index = ID.field_index;
        RecordLayoutError::DuplicateFieldIndex { kind: 91, field_index: ID.field_index }
    };
    let definition = RecordDefinition {
        fields: Box::leak(active.into_boxed_slice()),
        reserved_fields: Box::leak(retired.into_boxed_slice()),
        ..GAP_RECORD
    };
    assert_eq!(
        SchemaRegistry::try_new(Version::new(1, 0), &[definition], &[], &[], &[]).unwrap_err(),
        crate::idl::SchemaIdlError::RecordLayout(expected.clone())
    );
    let original = SchemaRegistry::with_records(Version::new(1, 0), &[GAP_RECORD]);
    let mut idl: serde_json::Value = serde_json::from_slice(&original.to_canonical_idl_bytes()).unwrap();
    let field =
        if reserved { &mut idl["records"][0]["reservedFields"][0] } else { &mut idl["records"][0]["fields"][1] };
    if name {
        field["name"] = ID.name.into();
    } else {
        field["index"] = ID.field_index.into();
    }
    let error = SchemaRegistry::from_idl_bytes_with_limits(
        &serde_json::to_vec(&idl).unwrap(),
        crate::idl_limits::SchemaIdlLimits::default(),
    )
    .unwrap_err();
    assert!(
        matches!(error, crate::idl_limits::BoundedSchemaIdlError::Schema(crate::idl::SchemaIdlError::RecordLayout(actual)) if actual == expected)
    );
}
