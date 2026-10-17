use super::*;
use crate::v1::{ffi::{SvConsumeOffset, SvProjectorId}, ProjectorAbiError};
use test_case::test_case;

fn offset(projector_id: &str) -> ConsumeOffset {
    ConsumeOffset { cluster_id: "ab".repeat(32), projector_id: projector_id.into(), raft_term: u64::MAX, raft_index: u64::MAX }
}

#[test_case("x".into(); "one_byte")]
#[test_case("7".into(); "numeric_id_is_a_string")]
#[test_case("x".repeat(128); "maximum_ascii")]
#[test_case("é".repeat(64); "maximum_utf8")]
#[test_case("行情📈".into(); "multibyte_continuations")]
#[test_case("\"\\".repeat(64); "literal_quotes_and_backslashes")]
#[test_case(" =#!: value\\n\\ ".into(); "literal_separators_and_spaces")]
#[test_case("行情\u{2028}📈".into(); "unicode_line_separator_is_not_lf")]
fn offset_properties_and_wire_preserve_exact_fields(id: String) {
    let original = offset(&id);
    let bytes = original.encode_properties().expect("valid offset");
    assert!(bytes.len() <= MAX_OFFSET_PROPERTIES_BYTES);
    assert_eq!(ConsumeOffset::decode_properties(&bytes), Ok(original.clone()));
    assert_eq!(SvConsumeOffset::encode(&original).unwrap().decode(), Ok(original));
}

#[test_case("", ConsumeOffsetError::ProjectorIdLength { actual: 0 }; "empty")]
#[test_case("\0", ConsumeOffsetError::EmbeddedNul; "nul")]
#[test_case("\n", ConsumeOffsetError::ControlCharacter { scalar: 10 }; "newline")]
#[test_case("\u{1b}", ConsumeOffsetError::ControlCharacter { scalar: 27 }; "escape")]
#[test_case("\u{7f}", ConsumeOffsetError::ControlCharacter { scalar: 127 }; "del")]
#[test_case("\u{85}", ConsumeOffsetError::ControlCharacter { scalar: 133 }; "c1_nel")]
#[test_case("\u{9f}", ConsumeOffsetError::ControlCharacter { scalar: 159 }; "c1_boundary")]
fn invalid_ids_never_encode(id: &str, expected: ConsumeOffsetError) {
    assert_eq!(offset(id).encode_properties(), Err(expected));
    assert_eq!(SvProjectorId::encode(id), Err(expected));
    let mut wire = SvProjectorId { len: id.len() as u64, ..SvProjectorId::default() };
    wire.bytes[..id.len()].copy_from_slice(id.as_bytes());
    let actual = wire.decode().unwrap_err();
    match expected {
        ConsumeOffsetError::EmbeddedNul => assert_eq!(actual, ProjectorAbiError::EmbeddedNul),
        ConsumeOffsetError::ControlCharacter { scalar } => assert_eq!(actual, ProjectorAbiError::ControlCharacter { scalar }),
        ConsumeOffsetError::ProjectorIdLength { actual: n } => assert_eq!(actual, ProjectorAbiError::Length { actual: n as u64, maximum: 128 }),
        _ => unreachable!(),
    }
}

#[test]
fn limits_are_checked_before_decoding_or_allocation() {
    assert_eq!(offset(&"x".repeat(129)).validate(), Err(ConsumeOffsetError::ProjectorIdLength { actual: 129 }));
    let oversized = MAX_OFFSET_PROPERTIES_BYTES + 1;
    assert_eq!(ConsumeOffset::decode_properties(&vec![255; oversized]), Err(ConsumeOffsetError::TooLarge {
        actual: oversized, maximum: MAX_OFFSET_PROPERTIES_BYTES,
    }));
    let wire = SvProjectorId { len: u64::MAX, bytes: [255; 128] };
    assert_eq!(wire.decode(), Err(ProjectorAbiError::Length { actual: u64::MAX, maximum: 128 }));
    let mut wire = SvProjectorId::encode("a").unwrap();
    wire.bytes[127] = 1;
    assert_eq!(wire.decode(), Err(ProjectorAbiError::Reserved { actual: 1 }));
}

#[test]
fn names_are_exact_and_offsets_are_not_genesis_sentinels() {
    let mut value = offset("Trade");
    assert_eq!(value.check_source(&value.cluster_id, "trade"), Err(ConsumeOffsetError::ProjectorMismatch));
    assert_eq!(value.check_source(&"00".repeat(32), "Trade"), Err(ConsumeOffsetError::ClusterMismatch));
    value.raft_index = 0;
    assert_eq!(value.validate(), Err(ConsumeOffsetError::ZeroIndex));
    value.raft_term = 0;
    assert_eq!(value.validate(), Err(ConsumeOffsetError::ZeroTerm));
    value.cluster_id = "AB".repeat(32);
    assert_eq!(value.validate(), Err(ConsumeOffsetError::ClusterIdentity));
}

const PROPERTIES: &str = concat!(
    "cluster_id=abababababababababababababababababababababababababababababababab\n",
    "projector_id=history\n",
    "raft_term=7\n",
    "raft_index=42\n",
);

#[test]
fn properties_have_independent_golden_bytes_and_accept_reordered_fields() {
    let value = ConsumeOffset { raft_term: 7, raft_index: 42, ..offset("history") };
    assert_eq!(value.encode_properties().unwrap(), PROPERTIES.as_bytes());
    assert_eq!(ConsumeOffset::decode_properties(PROPERTIES.as_bytes()), Ok(value.clone()));
    let reordered = format!("raft_index=42\nraft_term=7\nprojector_id=history\ncluster_id={}\n", value.cluster_id);
    assert_eq!(ConsumeOffset::decode_properties(reordered.as_bytes()), Ok(value));
}

#[test_case("cluster_id", OffsetField::ClusterId; "cluster")]
#[test_case("projector_id", OffsetField::ProjectorId; "projector")]
#[test_case("raft_term", OffsetField::RaftTerm; "term")]
#[test_case("raft_index", OffsetField::RaftIndex; "index")]
fn every_field_is_required_exactly_once(key: &str, field: OffsetField) {
    let line = PROPERTIES.split_inclusive('\n').find(|line| line.split_once('=').unwrap().0 == key).unwrap();
    let missing = PROPERTIES.replace(line, "");
    assert_eq!(ConsumeOffset::decode_properties(missing.as_bytes()), Err(ConsumeOffsetError::MissingProperty { field }));
    let duplicate = format!("{PROPERTIES}{line}");
    assert_eq!(ConsumeOffset::decode_properties(duplicate.as_bytes()), Err(ConsumeOffsetError::DuplicateProperty { field }));
}

#[test_case(""; "empty")]
#[test_case("18446744073709551616"; "overflow")]
#[test_case("1.0"; "float")]
#[test_case("-1"; "negative")]
#[test_case("+1"; "plus")]
#[test_case("01"; "leading_zero")]
#[test_case("\"1\""; "quoted")]
#[test_case("1e2"; "exponent")]
#[test_case(" 1"; "leading_space")]
#[test_case("1 "; "trailing_space")]
#[test_case("١"; "unicode_digit")]
fn coordinates_require_canonical_u64(value: &str) {
    for (key, original, field) in [
        ("raft_term", "7", OffsetField::RaftTerm),
        ("raft_index", "42", OffsetField::RaftIndex),
    ] {
        let bytes = PROPERTIES.replace(&format!("{key}={original}\n"), &format!("{key}={value}\n"));
        assert_eq!(ConsumeOffset::decode_properties(bytes.as_bytes()), Err(ConsumeOffsetError::InvalidInteger { field }));
    }
}

#[test_case("raft_index=0\n", ConsumeOffsetError::ZeroIndex; "zero_index")]
#[test_case("raft_index=42", ConsumeOffsetError::PropertySyntax { line: 4 }; "missing_final_lf")]
#[test_case("raft_index=42\n\n", ConsumeOffsetError::PropertySyntax { line: 5 }; "blank_line")]
#[test_case("raft_index=42\n#secret\n", ConsumeOffsetError::PropertySyntax { line: 5 }; "comment")]
#[test_case("raft_index=42\nsecret=value\n", ConsumeOffsetError::UnknownProperty { line: 5 }; "unknown_key")]
#[test_case("raft_index:42\n", ConsumeOffsetError::PropertySyntax { line: 4 }; "colon_separator")]
#[test_case(" raft_index=42\n", ConsumeOffsetError::UnknownProperty { line: 4 }; "no_key_trimming")]
#[test_case("raft_index=42\r\n", ConsumeOffsetError::InvalidInteger { field: OffsetField::RaftIndex }; "crlf")]
fn malformed_properties_preserve_typed_causes_without_echoing_input(tail: &str, expected: ConsumeOffsetError) {
    let bytes = PROPERTIES.replace("raft_index=42\n", tail);
    let error = ConsumeOffset::decode_properties(bytes.as_bytes()).unwrap_err();
    assert_eq!(error, expected);
    assert!(!format!("{error:?}").contains("secret"));
}

#[test]
fn decoded_fields_keep_identity_validation_and_reject_legacy_json() {
    for (from, to, expected) in [
        ("raft_term=7", "raft_term=0", ConsumeOffsetError::ZeroTerm),
        ("projector_id=history", "projector_id=", ConsumeOffsetError::ProjectorIdLength { actual: 0 }),
        ("projector_id=history", "projector_id=his\u{85}tory", ConsumeOffsetError::ControlCharacter { scalar: 133 }),
        ("cluster_id=ab", "cluster_id=AB", ConsumeOffsetError::ClusterIdentity),
    ] {
        let bytes = PROPERTIES.replace(from, to);
        assert_eq!(ConsumeOffset::decode_properties(bytes.as_bytes()), Err(expected));
    }
    assert_eq!(ConsumeOffset::decode_properties(&[255]), Err(ConsumeOffsetError::Utf8 { valid_up_to: 0 }));
    let bom = format!("\u{feff}{PROPERTIES}");
    assert_eq!(ConsumeOffset::decode_properties(bom.as_bytes()), Err(ConsumeOffsetError::UnknownProperty { line: 1 }));
    for json in [r#"["cluster","history",7,42]"#, r#"{"cluster_id":"cluster","projector_id":"history","raft_term":7,"raft_index":42}"#] {
        assert_eq!(ConsumeOffset::decode_properties(json.as_bytes()), Err(ConsumeOffsetError::PropertySyntax { line: 1 }));
    }
}
