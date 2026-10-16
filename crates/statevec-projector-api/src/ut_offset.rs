use super::*;
use crate::plugin_v2::{ffi::{SvConsumeOffset, SvProjectorId}, ProjectorAbiFailure};
use test_case::test_case;

fn offset(projector_id: &str) -> ConsumeOffset {
    ConsumeOffset { cluster_id: "ab".repeat(32), projector_id: projector_id.into(), raft_term: u64::MAX, raft_index: u64::MAX }
}

#[test_case("x".into(); "one_byte")]
#[test_case("x".repeat(128); "maximum_ascii")]
#[test_case("é".repeat(64); "maximum_utf8")]
#[test_case("行情📈".into(); "multibyte_continuations")]
#[test_case("\"\\".repeat(64); "escaped_maximum")]
fn offset_json_and_wire_preserve_exact_fields(id: String) {
    let original = offset(&id);
    let json = original.encode_json().expect("valid offset");
    assert!(json.len() <= MAX_OFFSET_JSON_BYTES);
    assert_eq!(ConsumeOffset::decode_json(&json), Ok(original.clone()));
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
    assert_eq!(offset(id).encode_json(), Err(expected));
    assert_eq!(SvProjectorId::encode(id), Err(expected));
    let mut wire = SvProjectorId { len: id.len() as u64, ..SvProjectorId::default() };
    wire.bytes[..id.len()].copy_from_slice(id.as_bytes());
    let actual = wire.decode().unwrap_err();
    match expected {
        ConsumeOffsetError::EmbeddedNul => assert_eq!(actual, ProjectorAbiFailure::EmbeddedNul),
        ConsumeOffsetError::ControlCharacter { scalar } => assert_eq!(actual, ProjectorAbiFailure::ControlCharacter { scalar }),
        ConsumeOffsetError::ProjectorIdLength { actual: n } => assert_eq!(actual, ProjectorAbiFailure::Length { actual: n as u64, maximum: 128 }),
        _ => unreachable!(),
    }
}

#[test]
fn limits_are_checked_before_decoding_or_allocation() {
    assert_eq!(offset(&"x".repeat(129)).validate(), Err(ConsumeOffsetError::ProjectorIdLength { actual: 129 }));
    assert_eq!(ConsumeOffset::decode_json(&vec![b' '; 1025]), Err(ConsumeOffsetError::TooLarge { actual: 1025, maximum: 1024 }));
    let wire = SvProjectorId { len: u64::MAX, bytes: [255; 128] };
    assert_eq!(wire.decode(), Err(ProjectorAbiFailure::Length { actual: u64::MAX, maximum: 128 }));
    let mut wire = SvProjectorId::encode("a").unwrap();
    wire.bytes[127] = 1;
    assert_eq!(wire.decode(), Err(ProjectorAbiFailure::Reserved { actual: 1 }));
}

#[test_case("\"raft_index\":1,\"raft_index\":2"; "duplicate_index")]
#[test_case("\"raft_index\":1,\"extra\":0"; "unknown_field")]
#[test_case("\"raft_index\":18446744073709551616"; "overflow")]
#[test_case("\"raft_index\":1.0"; "float")]
#[test_case("\"raft_index\":-1"; "negative")]
#[test_case("\"raft_index\":\"1\""; "string_index")]
fn malformed_offset_shape_is_typed_without_echoing_input(tail: &str) {
    let json = format!("{{\"cluster_id\":\"{}\",\"projector_id\":\"secret\",\"raft_term\":1,{tail}}}", "ab".repeat(32));
    let error = ConsumeOffset::decode_json(json.as_bytes()).unwrap_err();
    assert!(matches!(error, ConsumeOffsetError::Json { category: OffsetJsonCategory::Data, .. }), "{error:?}");
    assert!(!format!("{error:?}").contains("secret"));
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

#[test]
fn offset_json_requires_an_object_not_a_positional_array() {
    let json = format!("[\"{}\",\"history\",1,2]", "ab".repeat(32));
    assert!(matches!(ConsumeOffset::decode_json(json.as_bytes()),
        Err(ConsumeOffsetError::Json { category: OffsetJsonCategory::Data, .. })));
    let canonical = offset("history").encode_json().unwrap();
    assert_eq!(ConsumeOffset::decode_json(&canonical), Ok(offset("history")));
}

#[test]
fn numeric_projector_id_and_duplicate_identity_are_not_accepted() {
    let valid = String::from_utf8(offset("a").encode_json().unwrap()).unwrap();
    for json in [valid.replace("\"projector_id\":\"a\"", "\"projector_id\":7"), valid.replace("\"projector_id\":\"a\"", "\"projector_id\":\"a\",\"projector_id\":\"b\"")] {
        assert!(matches!(ConsumeOffset::decode_json(json.as_bytes()), Err(ConsumeOffsetError::Json { category: OffsetJsonCategory::Data, .. })));
    }
}
