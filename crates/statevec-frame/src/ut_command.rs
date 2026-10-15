use super::*;
use test_case::test_case;

#[test]
fn lengths_are_checked_without_allocating_the_declared_body() {
    assert_eq!(encoded_length(0), Ok((0, 30)));
    assert_eq!(encoded_length(usize::MAX), Err(CommandPayloadFailure::LengthOverflow));
    #[cfg(target_pointer_width = "64")]
    {
        assert_eq!(encoded_length(u32::MAX as usize), Ok((u32::MAX, u32::MAX as usize + 30)));
        assert_eq!(encoded_length(u32::MAX as usize + 1), Err(CommandPayloadFailure::LengthOverflow));
    }
    let mut payload = runtime_binary_v0_payload(1, 1, 0, 0, &[]);
    Arc::make_mut(&mut payload.payload_bytes)[26..30].copy_from_slice(&u32::MAX.to_le_bytes());
    let expected = if usize::BITS > 32 {
        CommandPayloadFailure::Truncated
    } else {
        CommandPayloadFailure::LengthOverflow
    };
    assert_eq!(decode_runtime_binary_v0(&payload), Err(expected));
}

#[test]
fn every_partial_header_is_truncated() {
    let full = runtime_binary_v0_payload(1, 1, 0, 0, &[]);
    for length in 0..30 {
        let payload = EngineCommandPayload {
            payload_codec: full.payload_codec.clone(), payload_bytes: Arc::from(&full.payload_bytes[..length]),
        };
        assert_eq!(decode_runtime_binary_v0(&payload), Err(CommandPayloadFailure::Truncated), "length={length}");
    }
}

#[test]
fn codec_version_and_full_width_fields_remain_exact() {
    let command = Command::new(USER_KIND_MAX, u64::MAX, u64::MAX, Vec::new());
    let mut payload = encode_runtime_binary_v0_command_payload(&command).unwrap();
    let fields = RuntimeBinaryV0CommandDecoder.decode_checked_fields(&payload).unwrap();
    assert_eq!((fields.command_kind, fields.ext_seq, fields.ref_ext_time_us), (USER_KIND_MAX, u64::MAX, u64::MAX));
    assert!(fields.payload.is_empty());
    payload.payload_codec.codec_version = u16::MAX;
    assert_eq!(decode_runtime_binary_v0(&payload), Err(CommandPayloadFailure::UnsupportedCodecVersion { version: u16::MAX }));
}

#[test_case(INVALID_KIND; "invalid_zero")]
#[test_case(USER_KIND_MAX + 1; "first_reserved_kind")]
#[test_case(u16::MAX; "last_reserved_kind")]
fn encoder_rejects_invalid_command_kind(kind: u16) {
    let command = Command::new(kind, 17, 9, vec![1, 2, 3]);

    let result = encode_runtime_binary_v0_command_payload(&command);

    assert!(
        matches!(result, Err(CommandPayloadFailure::InvalidKind { kind: actual }) if actual == kind),
        "encoder must reject a kind its decoder cannot accept: {result:?}"
    );
}

#[test_case(USER_KIND_MIN; "lowest_user_kind")]
#[test_case(USER_KIND_MAX; "highest_user_kind")]
fn legal_command_kind_round_trips(kind: u16) {
    let command = Command::new(kind, 17, 9, vec![1, 2, 3]);

    let encoded = encode_runtime_binary_v0_command_payload(&command).unwrap();
    let decoded = RuntimeBinaryV0CommandDecoder.decode_checked(&encoded).unwrap();

    assert_eq!(decoded, command);
}

#[test]
fn valid_command_keeps_current_wire_bytes() {
    let command = Command::new(1, 17, 9, vec![1, 2, 3]);

    let encoded = encode_runtime_binary_v0_command_payload(&command).unwrap();

    assert_eq!(encoded.payload_codec.codec_id, "runtime_binary_v0");
    assert_eq!(encoded.payload_codec.codec_version, 0);
    assert_eq!(
        encoded.payload_bytes.as_ref(),
        &[
            b'S', b'V', b'R', b'0', 0, 0, 3, 0, 1, 0, 17, 0, 0, 0, 0, 0, 0, 0, 9, 0, 0, 0, 0, 0, 0, 0, 3, 0, 0, 0, 1,
            2, 3,
        ]
    );
}

fn runtime_binary_v0_payload(
    flags: u16,
    command_kind: u16,
    ext_seq: u64,
    ref_ext_time_us: u64,
    inner_payload: &[u8],
) -> EngineCommandPayload {
    let mut payload_bytes = Vec::new();
    payload_bytes.extend_from_slice(b"SVR0");
    payload_bytes.extend_from_slice(&0u16.to_le_bytes());
    payload_bytes.extend_from_slice(&flags.to_le_bytes());
    payload_bytes.extend_from_slice(&command_kind.to_le_bytes());
    payload_bytes.extend_from_slice(&ext_seq.to_le_bytes());
    payload_bytes.extend_from_slice(&ref_ext_time_us.to_le_bytes());
    payload_bytes.extend_from_slice(&(inner_payload.len() as u32).to_le_bytes());
    payload_bytes.extend_from_slice(inner_payload);
    EngineCommandPayload {
        payload_codec: PayloadCodec {
            codec_id: std::borrow::Cow::Borrowed(RUNTIME_BINARY_V0_CODEC_ID),
            codec_version: RUNTIME_BINARY_V0_CODEC_VERSION,
        },
        payload_bytes: payload_bytes.into(),
    }
}

fn decode_runtime_binary_v0(payload: &EngineCommandPayload) -> Result<Command, CommandPayloadFailure> {
    RuntimeBinaryV0CommandDecoder.decode_checked(payload)
}

fn assert_current_payload_failure(error: CommandPayloadFailure, expected: CommandPayloadFailure) {
    assert_eq!(error, expected);
}

#[test]
fn runtime_binary_v0_decoder_decodes_flagged_payload() -> Result<(), CommandPayloadFailure> {
    let inner_payload = b"inner-command-payload";
    let encoded = runtime_binary_v0_payload(0x0001 | 0x0002, 42, 9, 123_456, inner_payload);

    let decoded = decode_runtime_binary_v0(&encoded)?;

    assert_eq!(decoded.command_kind(), 42);
    assert_eq!(decoded.ext_seq(), 9);
    assert_eq!(decoded.ref_ext_time_us(), 123_456);
    assert_eq!(decoded.payload(), inner_payload);
    assert_ne!(decoded.payload(), encoded.payload_bytes.as_ref());
    Ok(())
}

#[test]
fn runtime_binary_v0_decoder_uses_neutral_values_for_absent_optional_fields() -> Result<(), CommandPayloadFailure> {
    let encoded = runtime_binary_v0_payload(0, 7, 0, 0, b"body");

    let decoded = decode_runtime_binary_v0(&encoded)?;

    assert_eq!(decoded.command_kind(), 7);
    assert_eq!(decoded.ext_seq(), 0);
    assert_eq!(decoded.ref_ext_time_us(), 0);
    assert_eq!(decoded.payload(), b"body");
    Ok(())
}

#[test]
fn runtime_binary_v0_decoder_rejects_reserved_flags_and_absent_nonzero_fields() {
    let reserved = runtime_binary_v0_payload(0x0004, 7, 0, 0, b"body");
    assert_current_payload_failure(
        decode_runtime_binary_v0(&reserved).expect_err("reserved flags should reject"),
        CommandPayloadFailure::ReservedFlags { flags: 4 },
    );

    let ext_without_flag = runtime_binary_v0_payload(0, 7, 1, 0, b"body");
    assert_current_payload_failure(
        decode_runtime_binary_v0(&ext_without_flag).expect_err("non-zero ext_seq without flag should reject"),
        CommandPayloadFailure::SequenceWithoutFlag,
    );

    let ref_time_without_flag = runtime_binary_v0_payload(0, 7, 0, 1, b"body");
    assert_current_payload_failure(
        decode_runtime_binary_v0(&ref_time_without_flag)
            .expect_err("non-zero ref_ext_time_us without flag should reject"),
        CommandPayloadFailure::TimeWithoutFlag,
    );
}

#[test]
fn runtime_binary_v0_decoder_rejects_unknown_codec_and_bad_command_kind() {
    let mut unknown_codec = runtime_binary_v0_payload(0, 7, 0, 0, b"body");
    unknown_codec.payload_codec.codec_id = "other".into();
    assert!(matches!(
        decode_runtime_binary_v0(&unknown_codec).expect_err("unknown codec should reject"),
        CommandPayloadFailure::UnsupportedCodec { codec_id } if codec_id == "other"
    ));

    let invalid_command_kind = runtime_binary_v0_payload(0, 0, 0, 0, b"body");
    assert_current_payload_failure(
        decode_runtime_binary_v0(&invalid_command_kind).expect_err("invalid command kind should reject"),
        CommandPayloadFailure::InvalidKind { kind: 0 },
    );
}

#[test]
fn runtime_binary_v0_decoder_rejects_structural_payload_errors() {
    let mut bad_magic = runtime_binary_v0_payload(0, 7, 0, 0, b"body");
    Arc::make_mut(&mut bad_magic.payload_bytes)[0] = b'X';
    assert_current_payload_failure(
        decode_runtime_binary_v0(&bad_magic).expect_err("bad magic should reject"),
        CommandPayloadFailure::WrongMagic,
    );

    let mut bad_version = runtime_binary_v0_payload(0, 7, 0, 0, b"body");
    Arc::make_mut(&mut bad_version.payload_bytes)[4..6].copy_from_slice(&1u16.to_le_bytes());
    assert_current_payload_failure(
        decode_runtime_binary_v0(&bad_version).expect_err("bad version should reject"),
        CommandPayloadFailure::UnsupportedVersion { version: 1 },
    );

    let mut trailing = runtime_binary_v0_payload(0, 7, 0, 0, b"body");
    let mut trailing_bytes = trailing.payload_bytes.to_vec();
    trailing_bytes.push(1);
    trailing.payload_bytes = trailing_bytes.into();
    assert_current_payload_failure(
        decode_runtime_binary_v0(&trailing).expect_err("trailing bytes should reject"),
        CommandPayloadFailure::TrailingBytes,
    );

    let mut truncated_inner = runtime_binary_v0_payload(0, 7, 0, 0, b"body");
    let mut truncated_inner_bytes = truncated_inner.payload_bytes.to_vec();
    truncated_inner_bytes.truncate(truncated_inner_bytes.len() - 1);
    truncated_inner.payload_bytes = truncated_inner_bytes.into();
    assert_current_payload_failure(
        decode_runtime_binary_v0(&truncated_inner).expect_err("truncated inner payload should reject"),
        CommandPayloadFailure::Truncated,
    );

    let mut truncated_header = runtime_binary_v0_payload(0, 7, 0, 0, b"body");
    let mut truncated_header_bytes = truncated_header.payload_bytes.to_vec();
    truncated_header_bytes.truncate(8);
    truncated_header.payload_bytes = truncated_header_bytes.into();
    assert_current_payload_failure(
        decode_runtime_binary_v0(&truncated_header).expect_err("truncated header should reject"),
        CommandPayloadFailure::Truncated,
    );
}
