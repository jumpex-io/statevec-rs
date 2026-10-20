use super::*;
use crate::command::{CommandDecoder, encode_runtime_binary_v0_command_payload};
use statevec_model::Command;
use std::sync::Arc;
use test_case::test_case;

// Literal vectors, independent of the encoder's constants and field writers.
const INIT: &[u8] = b"SVLC\x01\x00\x01\x00\x03\x00\x00\x00abc";
const UPDATE_BODY: &[u8] = b"SVLC\x01\x00\x02\x00\
    \x00\x01\x02\x03\x04\x05\x06\x07\x08\x09\x0a\x0b\x0c\x0d\x0e\x0f\
    \x10\x11\x12\x13\x14\x15\x16\x17\x18\x19\x1a\x1b\x1c\x1d\x1e\x1f\
    \x20\x21\x22\x23\x24\x25\x26\x27\x28\x29\x2a\x2b\x2c\x2d\x2e\x2f\
    \x30\x31\x32\x33\x34\x35\x36\x37\x38\x39\x3a\x3b\x3c\x3d\x3e\x3f\
    \x03\x00\x00\x00abc";

#[derive(Clone, Copy, Debug)]
enum Operation { Initialize, Update }

impl Operation {
    fn command(self) -> LifecycleCommand<'static> {
        match self {
            Self::Initialize => LifecycleCommand::InitializeCore { input: b"abc" },
            Self::Update => LifecycleCommand::UpdateCore {
                source: CoreVersion::from_digest(Digest32::new(std::array::from_fn(|i| i as u8))),
                target: CoreVersion::from_digest(Digest32::new(std::array::from_fn(|i| i as u8 + 32))),
                input: b"abc",
            },
        }
    }
    fn vector(self) -> &'static [u8] {
        match self { Self::Initialize => INIT, Self::Update => UPDATE_BODY }
    }
}

#[test_case(Operation::Initialize; "initialize")]
#[test_case(Operation::Update; "update")]
fn literal_body_and_outer_envelope_are_exact(operation: Operation) {
    let expected = operation.vector();
    let encoded = operation.command().encode_command(17, 9).unwrap();

    assert_eq!(&encoded.payload_bytes[..26], b"SVR0\x00\x00\x03\x00\x02\xf0\
        \x11\x00\x00\x00\x00\x00\x00\x00\x09\x00\x00\x00\x00\x00\x00\x00");
    assert_eq!(u32::from_le_bytes(encoded.payload_bytes[26..30].try_into().unwrap()) as usize, expected.len());
    let envelope = CommandDecoder.decode_envelope(&encoded).unwrap();
    assert_eq!(envelope.command_kind, 0xF002);
    assert_eq!(envelope.payload, expected);
    assert_eq!(LifecycleCommand::decode(expected), Ok(operation.command()));
    let decoded = LifecycleCommand::decode(envelope.payload).unwrap();
    let input = match decoded {
        LifecycleCommand::InitializeCore { input } | LifecycleCommand::UpdateCore { input, .. } => input,
    };
    assert_eq!(input.as_ptr(), encoded.payload_bytes[encoded.payload_bytes.len() - 3..].as_ptr());
    assert_eq!(CommandDecoder.decode_checked(&encoded), Err(CommandPayloadError::InvalidKind { kind: 0xF002 }));
    let raw = Command::new(0xF002, 17, 9, expected.to_vec());
    assert_eq!(encode_runtime_binary_v0_command_payload(&raw), Err(CommandPayloadError::InvalidKind { kind: 0xF002 }));
}

#[test_case(Operation::Initialize; "initialize")]
#[test_case(Operation::Update; "update")]
fn every_truncation_and_trailing_byte_refuse(operation: Operation) {
    let bytes = operation.vector();
    for len in 0..bytes.len() {
        assert_eq!(LifecycleCommand::decode(&bytes[..len]), Err(LifecycleCodecError::Truncated), "cut at {len}");
    }
    let mut trailing = bytes.to_vec();
    trailing.push(0);
    assert_eq!(LifecycleCommand::decode(&trailing), Err(LifecycleCodecError::TrailingBytes));
    let mut oversized = bytes.to_vec();
    let length_offset = bytes.len() - 7;
    oversized[length_offset..length_offset + 4].copy_from_slice(&u32::MAX.to_le_bytes());
    assert_eq!(LifecycleCommand::decode(&oversized), Err(LifecycleCodecError::Truncated));
}

#[test_case(0, 0, LifecycleCodecError::WrongMagic; "magic")]
#[test_case(4, 2, LifecycleCodecError::UnsupportedVersion { version: 2 }; "version")]
#[test_case(6, 0, LifecycleCodecError::UnknownOperation { tag: 0 }; "zero_tag")]
#[test_case(6, 3, LifecycleCodecError::UnknownOperation { tag: 3 }; "unknown_tag")]
#[test_case(7, 1, LifecycleCodecError::ReservedFlags { flags: 1 }; "reserved_flags")]
fn malformed_header_has_typed_error(offset: usize, byte: u8, error: LifecycleCodecError) {
    let mut bytes = INIT.to_vec();
    bytes[offset] = byte;
    assert_eq!(LifecycleCommand::decode(&bytes), Err(error));
}

#[test_case(0xF000; "old_schema_reservation")]
#[test_case(0xF001; "old_manifest_reservation")]
#[test_case(0xF003; "unallocated_system_kind")]
fn reserved_kinds_cannot_use_the_execution_envelope(kind: u16) {
    let mut payload = Operation::Initialize.command().encode_command(1, 0).unwrap();
    Arc::make_mut(&mut payload.payload_bytes)[8..10].copy_from_slice(&kind.to_le_bytes());
    assert_eq!(CommandDecoder.decode_envelope(&payload), Err(CommandPayloadError::InvalidKind { kind }));
    assert_eq!(CommandDecoder.decode_checked(&payload), Err(CommandPayloadError::InvalidKind { kind }));
}

#[test]
fn empty_input_is_valid_and_impossible_sizes_refuse_before_allocation() {
    let command = LifecycleCommand::InitializeCore { input: b"" };
    let encoded = command.encode_command(0, 0).unwrap();
    let envelope = CommandDecoder.decode_envelope(&encoded).unwrap();
    assert_eq!(LifecycleCommand::decode(envelope.payload), Ok(command));
    assert_eq!(encoded_length(12, usize::MAX), Err(LifecycleCodecError::LengthOverflow));
    assert_eq!(encoded_length(76, u32::MAX as usize), Err(LifecycleCodecError::LengthOverflow));
    assert_eq!(encoded_length(12, 0), Ok(12));
}

