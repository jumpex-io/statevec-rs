//! Immutable V1 client intent. Decoding grants no proposal or commit authority.

use sha2::{Digest, Sha256};
use std::sync::Arc;

use crate::{
    EngineCommandPayload, InputRef, PayloadCodec, RUNTIME_BINARY_V0_CODEC_ID, RUNTIME_BINARY_V0_CODEC_VERSION,
};

const MAGIC: &[u8; 8] = b"SVBATCH1";
const VERSION: u16 = 1;
const CODEC_TAG: u16 = 1;
/// Magic, version, codec tag, client, stream, first sequence and command count.
pub const CLIENT_STREAM_BATCH_HEADER_BYTES: usize = 36;
pub const CLIENT_STREAM_BATCH_COMMITMENT_BYTES: usize = 56;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientStreamBatchFailure {
    EmptyByteLimit,
    EncodedBytesExceeded { observed: usize, limit: usize },
    InvalidCount { observed: usize, limit: usize },
    MissingIdentity,
    TransportIdentityNotDurable,
    SequenceOverflow,
    LengthOverflow,
    InvalidLength,
    InvalidMagic,
    UnsupportedVersion { observed: u16 },
    UnsupportedCodecTag { observed: u16 },
    UnsupportedCodecIdentity { member: usize },
    UnsupportedCodecVersion { member: usize, observed: u16 },
}

impl std::fmt::Display for ClientStreamBatchFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "client stream batch: {self:?}")
    }
}

impl std::error::Error for ClientStreamBatchFailure {}

/// One nonempty contiguous stream range with original ordered command bytes.
///
/// The first identity and count derive every member's sequence. There are no
/// independently writable member identities, end cursor or batch identifier.
/// Server-assigned execution time and transport correlation are not client intent.
#[statevec_domain_roles::domain_record]
#[derive(Debug, PartialEq, Eq)]
pub struct ClientStreamBatch {
    first: InputRef,
    commands: Vec<EngineCommandPayload>,
}

/// Whole intent identity echoed by replies, not a claim of proposal or commit.
/// The digest covers the canonical intent encoding, including range and lengths.
#[statevec_domain_roles::domain_record]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientStreamBatchCommitment {
    client_id: u64,
    stream_id: u32,
    first_sequence: u64,
    count: u32,
    digest: crate::Digest32,
}

impl ClientStreamBatchCommitment {
    /// Hash the first input identity and ordered payloads without owning copies.
    /// Uses the same intent range, codec and byte limits as `ClientStreamBatch`.
    /// This projection does not validate execution declarations, member input
    /// coordinates or history, and grants no admission or applied authority.
    pub fn try_from_payloads(
        first: Option<&InputRef>,
        commands: &[EngineCommandPayload],
        maximum_encoded_bytes: usize,
        maximum_commands: usize,
    ) -> Result<Self, ClientStreamBatchFailure> {
        check_count(commands.len(), maximum_commands)?;
        let first = first.ok_or(ClientStreamBatchFailure::MissingIdentity)?;
        encoded_len(first, commands.iter(), maximum_encoded_bytes, maximum_commands)?;
        Ok(commitment_v1(first, commands.iter()))
    }

    pub const fn client_id(self) -> u64 {
        self.client_id
    }
    pub const fn stream_id(self) -> u32 {
        self.stream_id
    }
    pub const fn first_sequence(self) -> u64 {
        self.first_sequence
    }
    pub const fn count(self) -> u32 {
        self.count
    }
    pub const fn digest(self) -> crate::Digest32 {
        self.digest
    }
    pub fn next_sequence(self) -> u64 {
        self.first_sequence + u64::from(self.count)
    }

    pub fn encode_v1(self) -> [u8; CLIENT_STREAM_BATCH_COMMITMENT_BYTES] {
        let mut bytes = [0; CLIENT_STREAM_BATCH_COMMITMENT_BYTES];
        bytes[..8].copy_from_slice(&self.client_id.to_le_bytes());
        bytes[8..12].copy_from_slice(&self.stream_id.to_le_bytes());
        bytes[12..20].copy_from_slice(&self.first_sequence.to_le_bytes());
        bytes[20..24].copy_from_slice(&self.count.to_le_bytes());
        bytes[24..].copy_from_slice(&self.digest.bytes());
        bytes
    }

    /// Check structure and limits only. A peer can declare any digest; callers
    /// must compare with the locally retained original intent before using it.
    pub fn decode_v1(bytes: &[u8], maximum_commands: usize) -> Result<Self, ClientStreamBatchFailure> {
        if bytes.len() != CLIENT_STREAM_BATCH_COMMITMENT_BYTES {
            return Err(ClientStreamBatchFailure::InvalidLength);
        }
        let mut remaining = bytes;
        let first = InputRef {
            client_id: u64::from_le_bytes(take(&mut remaining)?),
            stream_id: u32::from_le_bytes(take(&mut remaining)?),
            client_seq: u64::from_le_bytes(take(&mut remaining)?),
            request_id: None,
        };
        let count = u32::from_le_bytes(take(&mut remaining)?);
        check_range(&first, count as usize, maximum_commands)?;
        Ok(Self {
            client_id: first.client_id,
            stream_id: first.stream_id,
            first_sequence: first.client_seq,
            count,
            digest: crate::Digest32::new(take(&mut remaining)?),
        })
    }
}

impl ClientStreamBatch {
    /// Refuses the complete input unchanged, including a later invalid member.
    /// Bounds cover this intent encoding, not the larger execution/Raft entry;
    /// its owner must separately check the complete entry and response bounds.
    /// Successful construction drops spare command/codec allocation capacity;
    /// intent bytes and shared payload allocations are unchanged.
    pub fn try_new(
        first: InputRef,
        commands: Vec<EngineCommandPayload>,
        maximum_encoded_bytes: usize,
        maximum_commands: usize,
    ) -> Result<Self, (ClientStreamBatchFailure, InputRef, Vec<EngineCommandPayload>)> {
        if let Err(cause) = encoded_len(&first, commands.iter(), maximum_encoded_bytes, maximum_commands) {
            return Err((cause, first, commands));
        }
        // Normalize only after the complete input has passed validation. Boxed
        // conversions remove spare capacity without copying shared payloads.
        let mut commands = commands.into_boxed_slice().into_vec();
        for command in &mut commands {
            let codec = &mut command.payload_codec.codec_id;
            *codec = if *codec == crate::RUNTIME_BINARY_V0_CODEC_ID {
                std::borrow::Cow::Borrowed(crate::RUNTIME_BINARY_V0_CODEC_ID)
            } else {
                std::mem::take(codec).into_owned().into_boxed_str().into_string().into()
            };
        }
        Ok(Self { first, commands })
    }

    pub fn first_input(&self) -> &InputRef {
        &self.first
    }

    pub fn commands(&self) -> &[EngineCommandPayload] {
        &self.commands
    }

    /// Actual retained variable input storage. Shared payloads are charged in
    /// full; successful construction has already removed spare Vec/String
    /// capacity. This is a local allocation projection, not an RSS estimate.
    pub fn retained_input_bytes(&self) -> Option<usize> {
        let members = self
            .commands
            .capacity()
            .checked_mul(std::mem::size_of::<EngineCommandPayload>())?;
        self.commands.iter().try_fold(members, |total, command| {
            total
                .checked_add(command.payload_codec.retained_id_bytes())?
                .checked_add(command.payload_bytes.len())
        })
    }

    /// Complete apply advances this stream to this value, never an interior member.
    pub fn next_sequence(&self) -> u64 {
        // Every constructor checks this sum, and there is no mutable accessor.
        self.first.client_seq + self.commands.len() as u64
    }

    pub fn into_parts(self) -> (InputRef, Vec<EngineCommandPayload>) {
        (self.first, self.commands)
    }

    pub fn commitment_v1(&self) -> ClientStreamBatchCommitment {
        commitment_v1(&self.first, self.commands.iter())
    }

    /// Encode the complete intent or refuse it; no fitting-prefix API exists.
    pub fn encode_v1(
        &self,
        maximum_encoded_bytes: usize,
        maximum_commands: usize,
    ) -> Result<Vec<u8>, ClientStreamBatchFailure> {
        let mut bytes = Vec::new();
        self.append_v1(&mut bytes, maximum_encoded_bytes, maximum_commands)?;
        Ok(bytes)
    }

    /// Exact intent length under the consumer's limits, excluding its envelope.
    pub fn encoded_len_v1(
        &self,
        maximum_encoded_bytes: usize,
        maximum_commands: usize,
    ) -> Result<usize, ClientStreamBatchFailure> {
        encoded_len(&self.first, self.commands.iter(), maximum_encoded_bytes, maximum_commands)
    }

    /// Append one complete intent without a temporary payload copy. Typed refusal
    /// leaves the destination unchanged; its existing prefix is outside this limit.
    pub fn append_v1(
        &self,
        bytes: &mut Vec<u8>,
        maximum_encoded_bytes: usize,
        maximum_commands: usize,
    ) -> Result<(), ClientStreamBatchFailure> {
        let length = self.encoded_len_v1(maximum_encoded_bytes, maximum_commands)?;
        bytes
            .len()
            .checked_add(length)
            .ok_or(ClientStreamBatchFailure::LengthOverflow)?;
        bytes.reserve(length);
        self.emit_v1(|part| bytes.extend_from_slice(part));
        Ok(())
    }

    // One canonical byte traversal for encoding and digesting. Constructors have
    // checked every cast, and the immutable value has no unchecked entrypoint.
    fn emit_v1(&self, emit: impl FnMut(&[u8])) {
        emit_v1(&self.first, self.commands.iter(), emit);
    }

    /// Validate the complete envelope before allocating any member storage.
    /// Callers retain their input bytes on failure. Runtime schema/business
    /// validation and whole-batch admission remain independent requirements.
    pub fn decode_v1(
        bytes: &[u8],
        maximum_encoded_bytes: usize,
        maximum_commands: usize,
    ) -> Result<Self, ClientStreamBatchFailure> {
        check_bytes(bytes.len(), maximum_encoded_bytes)?;
        let mut remaining = bytes;
        if &take::<8>(&mut remaining)? != MAGIC {
            return Err(ClientStreamBatchFailure::InvalidMagic);
        }
        let version = u16::from_le_bytes(take(&mut remaining)?);
        if version != VERSION {
            return Err(ClientStreamBatchFailure::UnsupportedVersion { observed: version });
        }
        let codec = u16::from_le_bytes(take(&mut remaining)?);
        if codec != CODEC_TAG {
            return Err(ClientStreamBatchFailure::UnsupportedCodecTag { observed: codec });
        }
        let first = InputRef {
            client_id: u64::from_le_bytes(take(&mut remaining)?),
            stream_id: u32::from_le_bytes(take(&mut remaining)?),
            client_seq: u64::from_le_bytes(take(&mut remaining)?),
            request_id: None,
        };
        let count = usize::try_from(u32::from_le_bytes(take(&mut remaining)?))
            .map_err(|_| ClientStreamBatchFailure::LengthOverflow)?;
        check_range(&first, count, maximum_commands)?;
        // Each member needs its length word, even for a zero-byte payload. Never
        // reserve from an untrusted count that cannot fit in the actual input.
        if count > remaining.len() / 4 {
            return Err(ClientStreamBatchFailure::InvalidLength);
        }
        let mut scan = remaining;
        for _ in 0..count {
            take_payload(&mut scan)?;
        }
        if !scan.is_empty() {
            return Err(ClientStreamBatchFailure::InvalidLength);
        }
        let mut commands = Vec::with_capacity(count);
        for _ in 0..count {
            commands.push(EngineCommandPayload {
                payload_codec: PayloadCodec::runtime_binary_v0(),
                payload_bytes: Arc::from(take_payload(&mut remaining)?),
            });
        }
        Ok(Self { first, commands })
    }
}

fn check_count(count: usize, maximum_commands: usize) -> Result<(), ClientStreamBatchFailure> {
    if count == 0 || count > maximum_commands || u32::try_from(count).is_err() {
        return Err(ClientStreamBatchFailure::InvalidCount { observed: count, limit: maximum_commands });
    }
    Ok(())
}

fn check_range(first: &InputRef, count: usize, maximum_commands: usize) -> Result<(), ClientStreamBatchFailure> {
    check_count(count, maximum_commands)?;
    if first.client_id == 0 || first.stream_id == 0 || first.client_seq == 0 {
        return Err(ClientStreamBatchFailure::MissingIdentity);
    }
    if first.request_id.is_some() {
        return Err(ClientStreamBatchFailure::TransportIdentityNotDurable);
    }
    first
        .client_seq
        .checked_add(count as u64)
        .ok_or(ClientStreamBatchFailure::SequenceOverflow)?;
    Ok(())
}

fn encoded_len<'a>(
    first: &InputRef,
    commands: impl ExactSizeIterator<Item = &'a EngineCommandPayload>,
    maximum_encoded_bytes: usize,
    maximum_commands: usize,
) -> Result<usize, ClientStreamBatchFailure> {
    check_range(first, commands.len(), maximum_commands)?;
    let mut length = CLIENT_STREAM_BATCH_HEADER_BYTES;
    check_bytes(length, maximum_encoded_bytes)?;
    for (member, command) in commands.enumerate() {
        if command.payload_codec.codec_id != RUNTIME_BINARY_V0_CODEC_ID {
            return Err(ClientStreamBatchFailure::UnsupportedCodecIdentity { member });
        }
        if command.payload_codec.codec_version != RUNTIME_BINARY_V0_CODEC_VERSION {
            return Err(ClientStreamBatchFailure::UnsupportedCodecVersion {
                member,
                observed: command.payload_codec.codec_version,
            });
        }
        u32::try_from(command.payload_bytes.len()).map_err(|_| ClientStreamBatchFailure::LengthOverflow)?;
        length = length
            .checked_add(4)
            .and_then(|n| n.checked_add(command.payload_bytes.len()))
            .ok_or(ClientStreamBatchFailure::LengthOverflow)?;
        check_bytes(length, maximum_encoded_bytes)?;
    }
    Ok(length)
}

fn commitment_v1<'a>(
    first: &InputRef,
    commands: impl ExactSizeIterator<Item = &'a EngineCommandPayload>,
) -> ClientStreamBatchCommitment {
    let count = commands.len() as u32;
    let mut digest = Sha256::new();
    emit_v1(first, commands, |bytes| digest.update(bytes));
    ClientStreamBatchCommitment {
        client_id: first.client_id,
        stream_id: first.stream_id,
        first_sequence: first.client_seq,
        count,
        digest: crate::Digest32::new(digest.finalize().into()),
    }
}

// Both immutable intent forms use this traversal after complete range/length
// validation. Only private slice-backed iterators reach these unchecked casts.
fn emit_v1<'a>(
    first: &InputRef,
    commands: impl ExactSizeIterator<Item = &'a EngineCommandPayload>,
    mut emit: impl FnMut(&[u8]),
) {
    emit(MAGIC);
    emit(&VERSION.to_le_bytes());
    emit(&CODEC_TAG.to_le_bytes());
    emit(&first.client_id.to_le_bytes());
    emit(&first.stream_id.to_le_bytes());
    emit(&first.client_seq.to_le_bytes());
    emit(&(commands.len() as u32).to_le_bytes());
    for command in commands {
        emit(&(command.payload_bytes.len() as u32).to_le_bytes());
        emit(&command.payload_bytes);
    }
}

fn check_bytes(observed: usize, limit: usize) -> Result<(), ClientStreamBatchFailure> {
    if limit == 0 {
        return Err(ClientStreamBatchFailure::EmptyByteLimit);
    }
    if observed > limit {
        return Err(ClientStreamBatchFailure::EncodedBytesExceeded { observed, limit });
    }
    Ok(())
}

fn take<const N: usize>(remaining: &mut &[u8]) -> Result<[u8; N], ClientStreamBatchFailure> {
    let (value, rest) = remaining.split_at_checked(N).ok_or(ClientStreamBatchFailure::InvalidLength)?;
    let value = value.try_into().map_err(|_| ClientStreamBatchFailure::InvalidLength)?;
    *remaining = rest;
    Ok(value)
}

fn take_payload<'a>(remaining: &mut &'a [u8]) -> Result<&'a [u8], ClientStreamBatchFailure> {
    let length =
        usize::try_from(u32::from_le_bytes(take(remaining)?)).map_err(|_| ClientStreamBatchFailure::LengthOverflow)?;
    let (payload, rest) = remaining
        .split_at_checked(length)
        .ok_or(ClientStreamBatchFailure::InvalidLength)?;
    *remaining = rest;
    Ok(payload)
}
