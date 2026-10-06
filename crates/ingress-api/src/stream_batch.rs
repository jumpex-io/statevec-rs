//! Current v14 whole-batch ingress codec; retired versions have no fallback.
//! v14 permits retained Completed results on Reconcile; the body layout is unchanged.
//! Wire context is a declaration, not authenticated identity or admission proof.
//! The owner must join it to its checked cluster/profile and current history.

use std::num::{NonZeroU64, NonZeroU128};

use statevec_frame::{CLIENT_STREAM_BATCH_HEADER_BYTES, ClientStreamBatch, ClientStreamBatchFailure, Digest32};

#[path = "stream_batch_reader.rs"]
mod reader;
#[path = "stream_batch_reply.rs"]
mod reply;
pub use reader::{FrameReader, ReadFailure, ReadProgress, StreamDirection, StreamFrame};
pub use reply::{
    BatchSubmitRefusal, EntryBinding, IngressUnavailableReason, MemberResult, Reply, ReplyBinding, ReplyDisposition,
    ReplyObservation, ReplyRequestFailure, decode_reply, encode_reply, reply_frame_len,
};

pub const VERSION: u16 = 14;
pub const HEADER_BYTES: usize = 16;
pub const CONTEXT_BYTES: usize = 120;
const PREFIX_BYTES: usize = statevec_frame::stream_batch_protocol::REQUEST_PREFIX_BYTES;
const MIN_FRAME_BYTES: usize = PREFIX_BYTES + CLIENT_STREAM_BATCH_HEADER_BYTES + 4;
const SUBMIT: u16 = 1;
const RECONCILE: u16 = 2;

pub use statevec_frame::stream_batch_protocol::{Request, RequestContext};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameFailure {
    EmptyByteLimit,
    EncodedBytesExceeded { observed: usize, limit: usize },
    LengthOverflow,
    InvalidLength,
    InvalidMagic,
    UnsupportedVersion { observed: u16 },
    UnsupportedKind { observed: u16 },
    Reserved,
    ZeroSessionIncarnation,
    ZeroRequestId,
    Intent { source: ClientStreamBatchFailure },
    UnsupportedDisposition { observed: u8 },
    UnsupportedRefusal { observed: u8 },
    UnsupportedUnavailableReason { observed: u8 },
    UnsupportedMemberResult { observed: u8 },
    InvalidEntryBinding,
    MissingReplyObservation,
    InvalidReplyObservation,
    EntryObservationMismatch,
    AppliedIntentMismatch,
    AppliedBinding { source: statevec_frame::applied_binding::AppliedStreamBatchBindingFailure },
    InvalidAppliedFrontier,
    InvalidGapFrontier,
    ResultCount { observed: usize, expected: usize },
    InvalidOutcome,
    NoncontiguousResults,
    InvalidInlineEvent,
}

impl std::fmt::Display for FrameFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "whole-batch ingress: {self:?}")
    }
}

impl std::error::Error for FrameFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Intent { source } => Some(source),
            Self::AppliedBinding { source } => Some(source),
            _ => None,
        }
    }
}

/// Encode a complete request under a total-frame byte limit (header included).
/// Failure leaves the borrowed request and every original command unchanged.
pub fn encode_request(
    request: &Request,
    maximum_frame_bytes: usize,
    maximum_commands: usize,
) -> Result<Vec<u8>, FrameFailure> {
    let kind = match request {
        Request::Submit { .. } => SUBMIT,
        Request::Reconcile { .. } => RECONCILE,
    };
    encode_request_parts(request.context(), request.batch(), kind, maximum_frame_bytes, maximum_commands)
}

/// Encode the owner's retained original intent without transferring or cloning
/// its custody merely to construct a temporary Request value.
pub fn encode_submit(
    context: &RequestContext,
    batch: &ClientStreamBatch,
    maximum_frame_bytes: usize,
    maximum_commands: usize,
) -> Result<Vec<u8>, FrameFailure> {
    encode_request_parts(context, batch, SUBMIT, maximum_frame_bytes, maximum_commands)
}

/// Query the same retained intent without transferring its custody to a
/// temporary Request. The caller supplies a distinct query correlation.
pub fn encode_reconcile(
    context: &RequestContext,
    batch: &ClientStreamBatch,
    maximum_frame_bytes: usize,
    maximum_commands: usize,
) -> Result<Vec<u8>, FrameFailure> {
    encode_request_parts(context, batch, RECONCILE, maximum_frame_bytes, maximum_commands)
}

fn encode_request_parts(
    context: &RequestContext,
    batch: &ClientStreamBatch,
    kind: u16,
    maximum_frame_bytes: usize,
    maximum_commands: usize,
) -> Result<Vec<u8>, FrameFailure> {
    let intent_len = batch
        .encoded_len_v1(usize::MAX, maximum_commands)
        .map_err(|source| FrameFailure::Intent { source })?;
    let frame_len = PREFIX_BYTES.checked_add(intent_len).ok_or(FrameFailure::LengthOverflow)?;
    check_bytes(frame_len, maximum_frame_bytes)?;
    let body_len = u32::try_from(frame_len - HEADER_BYTES).map_err(|_| FrameFailure::LengthOverflow)?;
    let mut bytes = Vec::with_capacity(frame_len);
    bytes.extend_from_slice(b"SVI0");
    bytes.extend_from_slice(&VERSION.to_le_bytes());
    bytes.extend_from_slice(&kind.to_le_bytes());
    bytes.extend_from_slice(&0_u32.to_le_bytes());
    bytes.extend_from_slice(&body_len.to_le_bytes());
    append_context(&mut bytes, context);
    batch
        .append_v1(&mut bytes, intent_len, maximum_commands)
        .map_err(|source| FrameFailure::Intent { source })?;
    Ok(bytes)
}

/// Validate a header before allocating/reading its body. The returned total
/// frame length is only a framing bound, not validation of the body or context.
pub fn request_frame_len(header: &[u8; HEADER_BYTES], maximum_frame_bytes: usize) -> Result<usize, FrameFailure> {
    decode_header(header, maximum_frame_bytes, &[SUBMIT, RECONCILE], MIN_FRAME_BYTES).map(|(_, length)| length)
}

/// Validate the complete frame, context and intent before allocating members.
/// The fixed header rejects retired versions before decoding their body.
pub fn decode_request(
    bytes: &[u8],
    maximum_frame_bytes: usize,
    maximum_commands: usize,
) -> Result<Request, FrameFailure> {
    check_bytes(bytes.len(), maximum_frame_bytes)?;
    let mut remaining = bytes;
    let (kind, frame_len) =
        decode_header(&take(&mut remaining)?, maximum_frame_bytes, &[SUBMIT, RECONCILE], MIN_FRAME_BYTES)?;
    if frame_len != bytes.len() {
        return Err(FrameFailure::InvalidLength);
    }
    let context = take_context(&mut remaining)?;
    let batch = ClientStreamBatch::decode_v1(remaining, maximum_frame_bytes - PREFIX_BYTES, maximum_commands)
        .map_err(|source| FrameFailure::Intent { source })?;
    Ok(match kind {
        SUBMIT => Request::Submit { context, batch },
        RECONCILE => Request::Reconcile { context, batch },
        // decode_header checked the kind before the body was decoded.
        _ => return Err(FrameFailure::UnsupportedKind { observed: kind }),
    })
}

fn decode_header(
    header: &[u8; HEADER_BYTES],
    maximum_frame_bytes: usize,
    kinds: &[u16],
    minimum: usize,
) -> Result<(u16, usize), FrameFailure> {
    let mut remaining = header.as_slice();
    if &take::<4>(&mut remaining)? != b"SVI0" {
        return Err(FrameFailure::InvalidMagic);
    }
    let version = u16::from_le_bytes(take(&mut remaining)?);
    if version != VERSION {
        return Err(FrameFailure::UnsupportedVersion { observed: version });
    }
    let kind = u16::from_le_bytes(take(&mut remaining)?);
    if !kinds.contains(&kind) {
        return Err(FrameFailure::UnsupportedKind { observed: kind });
    }
    if take::<4>(&mut remaining)? != [0; 4] {
        return Err(FrameFailure::Reserved);
    }
    let body_len =
        usize::try_from(u32::from_le_bytes(take(&mut remaining)?)).map_err(|_| FrameFailure::LengthOverflow)?;
    let frame_len = HEADER_BYTES.checked_add(body_len).ok_or(FrameFailure::LengthOverflow)?;
    check_bytes(frame_len, maximum_frame_bytes)?;
    if frame_len < minimum {
        return Err(FrameFailure::InvalidLength);
    }
    Ok((kind, frame_len))
}

fn append_context(bytes: &mut Vec<u8>, context: &RequestContext) {
    bytes.extend_from_slice(&context.cluster_identity.bytes());
    bytes.extend_from_slice(&context.genesis_identity.bytes());
    bytes.extend_from_slice(&context.execution_profile.bytes());
    bytes.extend_from_slice(&context.session_incarnation.get().to_le_bytes());
    bytes.extend_from_slice(&context.request_id.get().to_le_bytes());
}

fn take_context(remaining: &mut &[u8]) -> Result<RequestContext, FrameFailure> {
    Ok(RequestContext {
        cluster_identity: Digest32::new(take(remaining)?),
        genesis_identity: Digest32::new(take(remaining)?),
        execution_profile: Digest32::new(take(remaining)?),
        session_incarnation: NonZeroU128::new(u128::from_le_bytes(take(remaining)?))
            .ok_or(FrameFailure::ZeroSessionIncarnation)?,
        request_id: NonZeroU64::new(u64::from_le_bytes(take(remaining)?)).ok_or(FrameFailure::ZeroRequestId)?,
    })
}

fn check_bytes(observed: usize, limit: usize) -> Result<(), FrameFailure> {
    if limit == 0 {
        return Err(FrameFailure::EmptyByteLimit);
    }
    if observed > limit {
        return Err(FrameFailure::EncodedBytesExceeded { observed, limit });
    }
    Ok(())
}

fn take<const N: usize>(remaining: &mut &[u8]) -> Result<[u8; N], FrameFailure> {
    let (value, rest) = remaining.split_at_checked(N).ok_or(FrameFailure::InvalidLength)?;
    let value = value.try_into().map_err(|_| FrameFailure::InvalidLength)?;
    *remaining = rest;
    Ok(value)
}
