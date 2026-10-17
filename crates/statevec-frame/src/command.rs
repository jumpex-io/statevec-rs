//! The current command payload codec, shared by public clients and native engines.
//!
//! This module only encodes command payloads. It does not assign stream positions,
//! sign intents, impose deployment limits or grant execution/admission authority.
use crate::{EngineCommandPayload, PayloadCodec};
use statevec_model::command::{Command, INVALID_KIND, USER_KIND_MAX, USER_KIND_MIN};
use std::sync::Arc;

/// Exhaustive typed codec failures; independent of engine and Raft state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandPayloadError {
    UnsupportedCodec { codec_id: String },
    UnsupportedCodecVersion { version: u16 },
    Truncated,
    WrongMagic,
    UnsupportedVersion { version: u16 },
    ReservedFlags { flags: u16 },
    InvalidKind { kind: u16 },
    LengthOverflow,
    TrailingBytes,
    SequenceWithoutFlag,
    TimeWithoutFlag,
}

impl std::fmt::Display for CommandPayloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "current command payload: {self:?}")
    }
}
impl std::error::Error for CommandPayloadError {}

/// Validated owned fields of a command payload.
pub struct DecodedCommandPayload {
    pub command_kind: u16,
    pub ext_seq: u64,
    pub ref_ext_time_us: u64,
    pub payload: Vec<u8>,
}
pub use crate::{RUNTIME_BINARY_V0_CODEC_ID, RUNTIME_BINARY_V0_CODEC_VERSION};

const RUNTIME_BINARY_V0_MAGIC: &[u8; 4] = b"SVR0";
const RUNTIME_BINARY_V0_PAYLOAD_VERSION: u16 = 0;
const RUNTIME_BINARY_V0_FLAG_EXT_SEQ: u16 = 0x0001;

const RUNTIME_BINARY_V0_FLAG_REF_EXT_TIME_US: u16 = 0x0002;
const RUNTIME_BINARY_V0_KNOWN_FLAGS: u16 = RUNTIME_BINARY_V0_FLAG_EXT_SEQ | RUNTIME_BINARY_V0_FLAG_REF_EXT_TIME_US;
const RUNTIME_BINARY_V0_HEADER_LEN: usize = 4 + 2 + 2 + 2 + 8 + 8 + 4;

/// Decoder for the existing `runtime_binary_v0` command payload.
#[derive(Debug, Default, Clone, Copy)]
pub struct CommandDecoder;

/// Encodes one runtime command into the sole current command payload format.
///
/// The command is the complete typed input. Callers cannot supply codec fields,
/// presence flags, or declared lengths independently. The body length must fit
/// in u32 and the 30-byte header plus body must fit in usize. No new deployment
/// limit is imposed here; ingress still applies its configured request limits.
pub fn encode_runtime_binary_v0_command_payload(
    command: &Command,
) -> Result<EngineCommandPayload, CommandPayloadError> {
    check_command_kind(command.command_kind())?;
    let (payload_len, encoded_len) = encoded_length(command.payload().len())?;
    let mut flags = RUNTIME_BINARY_V0_FLAG_EXT_SEQ;
    if command.ref_ext_time_us() != 0 {
        flags |= RUNTIME_BINARY_V0_FLAG_REF_EXT_TIME_US;
    }
    let mut bytes = Vec::with_capacity(encoded_len);
    bytes.extend_from_slice(RUNTIME_BINARY_V0_MAGIC);
    bytes.extend_from_slice(&RUNTIME_BINARY_V0_PAYLOAD_VERSION.to_le_bytes());
    bytes.extend_from_slice(&flags.to_le_bytes());
    bytes.extend_from_slice(&command.command_kind().to_le_bytes());
    bytes.extend_from_slice(&command.ext_seq().to_le_bytes());
    bytes.extend_from_slice(&command.ref_ext_time_us().to_le_bytes());
    bytes.extend_from_slice(&payload_len.to_le_bytes());
    bytes.extend_from_slice(command.payload());
    Ok(EngineCommandPayload { payload_codec: PayloadCodec::runtime_binary_v0(), payload_bytes: Arc::from(bytes) })
}

impl CommandDecoder {
    pub fn decode_checked(&self, payload: &EngineCommandPayload) -> Result<Command, CommandPayloadError> {
        self.check_codec(payload)?;
        decode_runtime_binary_v0_payload(&payload.payload_bytes)
    }
    pub fn decode_checked_fields(
        &self,
        payload: &EngineCommandPayload,
    ) -> Result<DecodedCommandPayload, CommandPayloadError> {
        self.check_codec(payload)?;
        decode_runtime_binary_v0_payload_fields(&payload.payload_bytes)
    }
    fn check_codec(&self, payload: &EngineCommandPayload) -> Result<(), CommandPayloadError> {
        if payload.payload_codec.codec_id != RUNTIME_BINARY_V0_CODEC_ID {
            return Err(CommandPayloadError::UnsupportedCodec {
                codec_id: payload.payload_codec.codec_id.to_string(),
            });
        }
        if payload.payload_codec.codec_version != RUNTIME_BINARY_V0_CODEC_VERSION {
            return Err(CommandPayloadError::UnsupportedCodecVersion {
                version: payload.payload_codec.codec_version,
            });
        }
        Ok(())
    }
}
fn decode_runtime_binary_v0_payload(bytes: &[u8]) -> Result<Command, CommandPayloadError> {
    let decoded = decode_runtime_binary_v0_payload_fields(bytes)?;
    Ok(Command::new(decoded.command_kind, decoded.ext_seq, decoded.ref_ext_time_us, decoded.payload))
}

fn decode_runtime_binary_v0_payload_fields(bytes: &[u8]) -> Result<DecodedCommandPayload, CommandPayloadError> {
    if bytes.len() < RUNTIME_BINARY_V0_HEADER_LEN {
        return Err(CommandPayloadError::Truncated);
    }

    let mut cursor = 0usize;
    let magic = take::<4>(bytes, &mut cursor)?;
    if magic != *RUNTIME_BINARY_V0_MAGIC {
        return Err(CommandPayloadError::WrongMagic);
    }

    let version = read_u16_le(bytes, &mut cursor)?;
    if version != RUNTIME_BINARY_V0_PAYLOAD_VERSION {
        return Err(CommandPayloadError::UnsupportedVersion { version });
    }

    let flags = read_u16_le(bytes, &mut cursor)?;
    let reserved_flags = flags & !RUNTIME_BINARY_V0_KNOWN_FLAGS;
    if reserved_flags != 0 {
        return Err(CommandPayloadError::ReservedFlags { flags: reserved_flags });
    }

    let command_kind = read_u16_le(bytes, &mut cursor)?;
    check_command_kind(command_kind)?;

    let ext_seq = read_u64_le(bytes, &mut cursor)?;
    let ref_ext_time_us = read_u64_le(bytes, &mut cursor)?;
    let payload_len = read_u32_le(bytes, &mut cursor)? as usize;
    let end = cursor
        .checked_add(payload_len)
        .ok_or_else(|| CommandPayloadError::LengthOverflow)?;
    if end > bytes.len() {
        return Err(CommandPayloadError::Truncated);
    }
    if end != bytes.len() {
        return Err(CommandPayloadError::TrailingBytes);
    }

    let has_ext_seq = flags & RUNTIME_BINARY_V0_FLAG_EXT_SEQ != 0;
    let has_ref_ext_time_us = flags & RUNTIME_BINARY_V0_FLAG_REF_EXT_TIME_US != 0;
    if !has_ext_seq && ext_seq != 0 {
        return Err(CommandPayloadError::SequenceWithoutFlag);
    }
    if !has_ref_ext_time_us && ref_ext_time_us != 0 {
        return Err(CommandPayloadError::TimeWithoutFlag);
    }

    let ext_seq = if has_ext_seq { ext_seq } else { 0 };
    let ref_ext_time_us = if has_ref_ext_time_us { ref_ext_time_us } else { 0 };
    Ok(DecodedCommandPayload { command_kind, ext_seq, ref_ext_time_us, payload: bytes[cursor..end].to_vec() })
}

fn encoded_length(length: usize) -> Result<(u32, usize), CommandPayloadError> {
    let payload_len = u32::try_from(length).map_err(|_| CommandPayloadError::LengthOverflow)?;
    let encoded_len = RUNTIME_BINARY_V0_HEADER_LEN.checked_add(length).ok_or(CommandPayloadError::LengthOverflow)?;
    Ok((payload_len, encoded_len))
}

fn check_command_kind(kind: u16) -> Result<(), CommandPayloadError> {
    if kind == INVALID_KIND || !(USER_KIND_MIN..=USER_KIND_MAX).contains(&kind) {
        return Err(CommandPayloadError::InvalidKind { kind });
    }
    Ok(())
}

fn read_u16_le(bytes: &[u8], cursor: &mut usize) -> Result<u16, CommandPayloadError> {
    Ok(u16::from_le_bytes(take::<2>(bytes, cursor)?))
}

fn read_u32_le(bytes: &[u8], cursor: &mut usize) -> Result<u32, CommandPayloadError> {
    Ok(u32::from_le_bytes(take::<4>(bytes, cursor)?))
}

fn read_u64_le(bytes: &[u8], cursor: &mut usize) -> Result<u64, CommandPayloadError> {
    Ok(u64::from_le_bytes(take::<8>(bytes, cursor)?))
}

fn take<const N: usize>(bytes: &[u8], cursor: &mut usize) -> Result<[u8; N], CommandPayloadError> {
    let end = cursor.checked_add(N).ok_or_else(|| CommandPayloadError::LengthOverflow)?;
    let slice = bytes.get(*cursor..end).ok_or_else(|| CommandPayloadError::Truncated)?;
    *cursor = end;
    let mut out = [0u8; N];
    out.copy_from_slice(slice);
    Ok(out)
}

#[cfg(test)]
#[path = "ut_command.rs"]
mod tests;
