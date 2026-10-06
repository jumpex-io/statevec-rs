use std::sync::Arc;

use sha2::{Digest, Sha256};
use test_case::test_case;

use crate::{
    ClientStreamBatch, ClientStreamBatchCommitment, ClientStreamBatchFailure as Failure, EngineCommandPayload,
    InputRef, PayloadCodec,
};

const BYTE_LIMIT: usize = 1024;
const COMMAND_LIMIT: usize = 4;

fn first(sequence: u64) -> InputRef {
    InputRef { client_id: 7, stream_id: 9, client_seq: sequence, request_id: None }
}

fn payload(bytes: &[u8]) -> EngineCommandPayload {
    EngineCommandPayload {
        payload_codec: PayloadCodec { codec_id: "runtime_binary_v0".into(), codec_version: 0 },
        payload_bytes: Arc::from(bytes),
    }
}

fn batch() -> ClientStreamBatch {
    ClientStreamBatch::try_new(first(11), vec![payload(&[0xaa, 0xbb]), payload(&[])], BYTE_LIMIT, COMMAND_LIMIT)
        .expect("two original commands in one stream")
}

#[test]
fn current_stream_batch_bytes_preserve_whole_original_intent() {
    // An independent fixed wire sample, including a zero-byte second command.
    // The codec validates framing, not whether a runtime accepts that command.
    let expected = [
        b'S', b'V', b'B', b'A', b'T', b'C', b'H', b'1', 1, 0, 1, 0, 7, 0, 0, 0, 0, 0, 0, 0, 9, 0, 0, 0, 11, 0, 0, 0, 0,
        0, 0, 0, 2, 0, 0, 0, 2, 0, 0, 0, 0xaa, 0xbb, 0, 0, 0, 0,
    ];
    let original = batch();
    assert_eq!(original.encode_v1(expected.len(), 2).unwrap(), expected);
    let borrowed = ClientStreamBatchCommitment::try_from_payloads(
        Some(original.first_input()),
        original.commands(),
        expected.len(),
        2,
    )
    .unwrap();
    assert_eq!(borrowed, original.commitment_v1());
    assert_eq!(borrowed.digest().bytes(), <[u8; 32]>::from(Sha256::digest(expected)));
    assert_eq!(
        (borrowed.client_id(), borrowed.stream_id(), borrowed.first_sequence(), borrowed.count()),
        (7, 9, 11, 2)
    );

    let decoded = ClientStreamBatch::decode_v1(&expected, expected.len(), 2).unwrap();
    assert_eq!(decoded.first_input(), &first(11));
    assert_eq!(decoded.next_sequence(), 13);
    assert_eq!(decoded.commands(), &[payload(&[0xaa, 0xbb]), payload(&[])]);
    assert_eq!(decoded, original);
}

#[test]
fn stream_batch_appends_after_an_existing_envelope_without_changing_intent() {
    let original = batch();
    let expected = original.encode_v1(BYTE_LIMIT, COMMAND_LIMIT).unwrap();
    let mut bytes = b"outer-envelope".to_vec();

    original.append_v1(&mut bytes, expected.len(), COMMAND_LIMIT).unwrap();

    assert_eq!(&bytes[..14], b"outer-envelope");
    assert_eq!(&bytes[14..], expected);
    assert_eq!(original.encoded_len_v1(BYTE_LIMIT, COMMAND_LIMIT).unwrap(), expected.len());
}

#[test_case(45, COMMAND_LIMIT, Failure::EncodedBytesExceeded { observed: 46, limit: 45 }; "byte_refusal")]
#[test_case(BYTE_LIMIT, 1, Failure::InvalidCount { observed: 2, limit: 1 }; "count_refusal")]
fn stream_batch_append_refusal_preserves_the_destination(byte_limit: usize, command_limit: usize, failure: Failure) {
    let original = batch();
    let mut destination = vec![0x42; 7];
    let original_pointer = destination.as_ptr();
    let original_capacity = destination.capacity();

    assert_eq!(original.append_v1(&mut destination, byte_limit, command_limit), Err(failure));

    assert_eq!(destination, [0x42; 7]);
    assert_eq!(destination.as_ptr(), original_pointer, "refusal must precede reserve");
    assert_eq!(destination.capacity(), original_capacity);
}

#[test_case(0, Failure::EmptyByteLimit; "zero_byte_limit")]
#[test_case(45, Failure::EncodedBytesExceeded { observed: 46, limit: 45 }; "one_byte_short")]
fn stream_batch_bounds_refuse_the_complete_encoding(limit: usize, expected: Failure) {
    let original = batch();
    let bytes = original.encode_v1(BYTE_LIMIT, COMMAND_LIMIT).unwrap();
    assert_eq!(
        ClientStreamBatchCommitment::try_from_payloads(
            Some(original.first_input()),
            original.commands(),
            limit,
            COMMAND_LIMIT
        ),
        Err(expected.clone())
    );
    assert_eq!(original.encode_v1(limit, COMMAND_LIMIT), Err(expected.clone()));
    assert_eq!(ClientStreamBatch::decode_v1(&bytes, limit, COMMAND_LIMIT), Err(expected));
    assert_eq!(original.commands().len(), 2, "refusal cannot trim the input");
}

#[test_case(0; "zero_commands_allowed")]
#[test_case(1; "one_command_short")]
fn stream_batch_count_limit_refuses_without_prefix_encoding(limit: usize) {
    let original = batch();
    let bytes = original.encode_v1(BYTE_LIMIT, COMMAND_LIMIT).unwrap();
    let expected = Failure::InvalidCount { observed: 2, limit };
    assert_eq!(
        ClientStreamBatchCommitment::try_from_payloads(
            Some(original.first_input()),
            original.commands(),
            BYTE_LIMIT,
            limit
        ),
        Err(expected.clone())
    );
    assert_eq!(original.encode_v1(BYTE_LIMIT, limit), Err(expected.clone()));
    assert_eq!(ClientStreamBatch::decode_v1(&bytes, BYTE_LIMIT, limit), Err(expected));
}

#[derive(Clone, Copy, Debug)]
enum BadInput {
    Empty,
    ZeroClient,
    ZeroStream,
    ZeroSequence,
    SequenceOverflow,
    RequestId,
    LaterCodec,
    LaterVersion,
}

#[test_case(BadInput::Empty; "empty_batch")]
#[test_case(BadInput::ZeroClient; "zero_client")]
#[test_case(BadInput::ZeroStream; "zero_stream")]
#[test_case(BadInput::ZeroSequence; "zero_sequence")]
#[test_case(BadInput::SequenceOverflow; "unrepresentable_next_sequence")]
#[test_case(BadInput::RequestId; "transport_identity")]
#[test_case(BadInput::LaterCodec; "later_codec_identity")]
#[test_case(BadInput::LaterVersion; "later_codec_version")]
fn invalid_stream_batch_construction_returns_all_original_input(case: BadInput) {
    let mut identity = first(11);
    let mut commands = vec![payload(&[1]), payload(&[2])];
    let expected = match case {
        BadInput::Empty => {
            commands.clear();
            Failure::InvalidCount { observed: 0, limit: COMMAND_LIMIT }
        }
        BadInput::ZeroClient => {
            identity.client_id = 0;
            Failure::MissingIdentity
        }
        BadInput::ZeroStream => {
            identity.stream_id = 0;
            Failure::MissingIdentity
        }
        BadInput::ZeroSequence => {
            identity.client_seq = 0;
            Failure::MissingIdentity
        }
        BadInput::SequenceOverflow => {
            identity.client_seq = u64::MAX - 1;
            Failure::SequenceOverflow
        }
        BadInput::RequestId => {
            identity.request_id = Some("correlation".to_owned());
            Failure::TransportIdentityNotDurable
        }
        BadInput::LaterCodec => {
            commands[1].payload_codec.codec_id = "unrecognized".into();
            Failure::UnsupportedCodecIdentity { member: 1 }
        }
        BadInput::LaterVersion => {
            commands[1].payload_codec.codec_version = 9;
            Failure::UnsupportedCodecVersion { member: 1, observed: 9 }
        }
    };
    commands.reserve_exact(128);
    for command in &mut commands {
        command.payload_codec.codec_id.to_mut().reserve_exact(128);
    }
    let original_identity = identity.clone();
    let original_commands = commands.clone();
    assert_eq!(
        ClientStreamBatchCommitment::try_from_payloads(Some(&identity), &commands, BYTE_LIMIT, COMMAND_LIMIT),
        Err(expected.clone()),
        "borrowed intent must preserve the owned constructor's typed failure: {case:?}"
    );
    let original_allocation = commands.as_ptr();
    let original_capacity = commands.capacity();
    let codec_allocations: Vec<_> = commands
        .iter()
        .map(|command| (command.payload_codec.codec_id.as_ptr(), command.payload_codec.retained_id_bytes()))
        .collect();

    let (failure, returned_identity, returned_commands) =
        ClientStreamBatch::try_new(identity, commands, BYTE_LIMIT, COMMAND_LIMIT).unwrap_err();

    assert_eq!(failure, expected, "{case:?}");
    assert_eq!(returned_identity, original_identity);
    assert_eq!(returned_commands, original_commands);
    assert_eq!(returned_commands.as_ptr(), original_allocation, "return the original vector, not a replacement");
    assert_eq!(returned_commands.capacity(), original_capacity, "refusal must not normalize allocations");
    for (command, (pointer, capacity)) in returned_commands.iter().zip(codec_allocations) {
        assert_eq!(command.payload_codec.codec_id.as_ptr(), pointer);
        assert_eq!(command.payload_codec.retained_id_bytes(), capacity);
    }
}

#[test]
fn borrowed_stream_commitment_refuses_missing_first_identity_without_changing_input() {
    let original = batch();
    let before = original.commands().to_vec();

    assert_eq!(
        ClientStreamBatchCommitment::try_from_payloads(None, original.commands(), BYTE_LIMIT, COMMAND_LIMIT),
        Err(Failure::MissingIdentity)
    );
    assert_eq!(original.commands(), before);
}

#[test]
fn stream_batch_last_representable_next_sequence_roundtrips() {
    let batch =
        ClientStreamBatch::try_new(first(u64::MAX - 2), vec![payload(&[1]), payload(&[2])], BYTE_LIMIT, 2).unwrap();
    let bytes = batch.encode_v1(BYTE_LIMIT, 2).unwrap();
    let decoded = ClientStreamBatch::decode_v1(&bytes, BYTE_LIMIT, 2).unwrap();
    let borrowed =
        ClientStreamBatchCommitment::try_from_payloads(Some(batch.first_input()), batch.commands(), BYTE_LIMIT, 2)
            .unwrap();
    assert_eq!(borrowed, batch.commitment_v1());
    assert_eq!(borrowed.next_sequence(), u64::MAX);
    assert_eq!(decoded.next_sequence(), u64::MAX);
    assert_eq!(decoded, batch);
}

#[test_case(u64::MAX - 1; "second_member_has_no_successor")]
#[test_case(u64::MAX; "first_member_has_no_successor")]
fn encoded_stream_batch_must_have_a_representable_next_sequence(first_sequence: u64) {
    let mut bytes = batch().encode_v1(BYTE_LIMIT, COMMAND_LIMIT).unwrap();
    bytes[24..32].copy_from_slice(&first_sequence.to_le_bytes());
    assert_eq!(ClientStreamBatch::decode_v1(&bytes, BYTE_LIMIT, COMMAND_LIMIT), Err(Failure::SequenceOverflow));
}

#[test]
fn stream_batch_truncation_and_trailing_bytes_never_decode_a_prefix() {
    let bytes = batch().encode_v1(BYTE_LIMIT, COMMAND_LIMIT).unwrap();
    for cut in 0..bytes.len() {
        assert_eq!(
            ClientStreamBatch::decode_v1(&bytes[..cut], BYTE_LIMIT, COMMAND_LIMIT),
            Err(Failure::InvalidLength),
            "cut={cut}"
        );
    }
    let mut trailing = bytes;
    trailing.push(0);
    assert_eq!(ClientStreamBatch::decode_v1(&trailing, BYTE_LIMIT, COMMAND_LIMIT), Err(Failure::InvalidLength));
}

#[test_case(0, 0, Failure::InvalidMagic; "magic")]
#[test_case(8, 2, Failure::UnsupportedVersion { observed: 2 }; "version")]
#[test_case(10, 0, Failure::UnsupportedCodecTag { observed: 0 }; "codec")]
#[test_case(12, 0, Failure::MissingIdentity; "client")]
#[test_case(20, 0, Failure::MissingIdentity; "stream")]
#[test_case(24, 0, Failure::MissingIdentity; "sequence")]
#[test_case(32, 0, Failure::InvalidCount { observed: 0, limit: COMMAND_LIMIT }; "count")]
#[test_case(42, 1, Failure::InvalidLength; "later_payload_length")]
fn stream_batch_rejects_invalid_complete_frames(offset: usize, replacement: u8, expected: Failure) {
    let mut bytes = batch().encode_v1(BYTE_LIMIT, COMMAND_LIMIT).unwrap();
    bytes[offset] = replacement;
    assert_eq!(ClientStreamBatch::decode_v1(&bytes, BYTE_LIMIT, COMMAND_LIMIT), Err(expected));
}
