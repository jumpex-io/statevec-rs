//! Public-client payload persistence keeps its JSON representation and original
//! binary intent. Use the client's existing serde test dependencies, not a new
//! dependency on a JSON format in the frame crate.
use statevec_frame::{ClientStreamBatch, EngineCommandPayload, InputRef, PayloadCodec};
use std::sync::Arc;
use test_case::test_case;

const EMPTY: &str = r#"{"payload_codec":{"codec_id":"runtime_binary_v0","codec_version":0},"payload_bytes":[]}"#;
const BINARY: &str = r#"{"payload_codec":{"codec_id":"runtime_binary_v0","codec_version":0},"payload_bytes":[0,127,128,255]}"#;

fn payload(bytes: &[u8]) -> EngineCommandPayload {
    EngineCommandPayload { payload_codec: PayloadCodec::runtime_binary_v0(), payload_bytes: Arc::from(bytes) }
}

#[test_case(&[], EMPTY; "empty")]
#[test_case(&[0, 127, 128, 255], BINARY; "full_byte_range")]
fn payload_keeps_independent_json_bytes(bytes: &[u8], expected: &str) {
    let original = payload(bytes);

    assert_eq!(serde_json::to_string(&original).unwrap(), expected);
    let restored: EngineCommandPayload = serde_json::from_str(expected).unwrap();
    assert_eq!(restored, original);
}

#[test]
fn payload_clones_share_storage_before_and_after_deserialization() {
    let original = payload(&[0, 127, 128, 255]);
    let cloned = original.clone();
    assert!(Arc::ptr_eq(&original.payload_bytes, &cloned.payload_bytes));

    let restored: EngineCommandPayload = serde_json::from_str(BINARY).unwrap();
    let restored_clone = restored.clone();
    assert!(Arc::ptr_eq(&restored.payload_bytes, &restored_clone.payload_bytes));
    // Persistence carries values, not Arc identity across deserialization.
    assert!(!Arc::ptr_eq(&original.payload_bytes, &restored.payload_bytes));
    assert_eq!(restored, original);
}

#[test_case("[256]"; "overflow")]
#[test_case("[-1]"; "negative")]
#[test_case("[1.5]"; "fraction")]
#[test_case("[true]"; "boolean")]
#[test_case("null"; "null")]
#[test_case("\"AH+A/w==\""; "base64_is_not_a_byte_sequence")]
fn payload_still_rejects_non_byte_sequences(value: &str) {
    let json = BINARY.replace("[0,127,128,255]", value);

    let error = serde_json::from_str::<EngineCommandPayload>(&json).unwrap_err();

    assert_eq!(error.classify(), serde_json::error::Category::Data);
}

#[test]
fn restored_payload_keeps_client_intent_bytes_and_commitment() {
    // Client framing deliberately treats the domain command body as opaque.
    const BYTE_LIMIT: usize = 128;
    const COMMAND_LIMIT: usize = 1;
    let first = InputRef { client_id: 7, stream_id: 3, client_seq: 11, request_id: None };
    let original = ClientStreamBatch::try_new(
        first.clone(),
        vec![payload(&[0, 127, 128, 255])],
        BYTE_LIMIT,
        COMMAND_LIMIT,
    )
    .unwrap();
    let restored: EngineCommandPayload = serde_json::from_str(BINARY).unwrap();
    let shared_bytes = Arc::clone(&restored.payload_bytes);

    let batch = ClientStreamBatch::try_new(first, vec![restored], BYTE_LIMIT, COMMAND_LIMIT).unwrap();

    assert!(Arc::ptr_eq(&shared_bytes, &batch.commands()[0].payload_bytes));
    assert_eq!(
        batch.encode_v1(BYTE_LIMIT, COMMAND_LIMIT).unwrap(),
        original.encode_v1(BYTE_LIMIT, COMMAND_LIMIT).unwrap()
    );
    assert_eq!(batch.commitment_v1(), original.commitment_v1());
}
