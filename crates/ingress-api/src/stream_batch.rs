//! Current v15 whole-batch ingress codec; retired versions have no fallback.
//! The context binds execution lineage, not the active Core version. Retained
//! Completed results on Reconcile keep the existing body layout.
//! Wire context is a declaration, not authenticated identity or admission proof.
//! The owner must join it to its checked cluster/lineage and current history.

use std::num::{NonZeroU64, NonZeroU128};

use statevec_frame::{CLIENT_STREAM_BATCH_HEADER_BYTES, ClientStreamBatch, ClientStreamBatchError, Digest32};

#[path = "stream_batch_reader.rs"]
mod reader;
#[path = "stream_batch_reply.rs"]
mod reply;
pub use reader::{FrameReader, ReadError, ReadProgress, StreamDirection, StreamFrame};
pub use reply::{
    BatchSubmitRefusal, EntryBinding, IngressUnavailableReason, MemberResult, Reply, ReplyBinding, ReplyDisposition,
    ReplyObservation, ReplyRequestError, decode_reply, encode_reply, reply_frame_len,
};

pub const VERSION: u16 = 15;
pub const HEADER_BYTES: usize = 16;
pub const CONTEXT_BYTES: usize = 120;
const PREFIX_BYTES: usize = statevec_frame::stream_batch_protocol::REQUEST_PREFIX_BYTES;
const MIN_FRAME_BYTES: usize = PREFIX_BYTES + CLIENT_STREAM_BATCH_HEADER_BYTES + 4;
const SUBMIT: u16 = 1;
const RECONCILE: u16 = 2;

pub use statevec_frame::stream_batch_protocol::{Request, RequestContext};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameError {
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
    Intent { source: ClientStreamBatchError },
    UnsupportedDisposition { observed: u8 },
    UnsupportedRefusal { observed: u8 },
    UnsupportedUnavailableReason { observed: u8 },
    UnsupportedMemberResult { observed: u8 },
    InvalidEntryBinding,
    MissingReplyObservation,
    InvalidReplyObservation,
    EntryObservationMismatch,
    AppliedIntentMismatch,
    AppliedBinding { source: statevec_frame::applied_binding::AppliedStreamBatchBindingError },
    InvalidAppliedFrontier,
    InvalidGapFrontier,
    ResultCount { observed: usize, expected: usize },
    InvalidOutcome,
    NoncontiguousResults,
    InvalidInlineEvent,
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "whole-batch ingress: {self:?}")
    }
}

impl std::error::Error for FrameError {
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
) -> Result<Vec<u8>, FrameError> {
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
) -> Result<Vec<u8>, FrameError> {
    encode_request_parts(context, batch, SUBMIT, maximum_frame_bytes, maximum_commands)
}

/// Query the same retained intent without transferring its custody to a
/// temporary Request. The caller supplies a distinct query correlation.
pub fn encode_reconcile(
    context: &RequestContext,
    batch: &ClientStreamBatch,
    maximum_frame_bytes: usize,
    maximum_commands: usize,
) -> Result<Vec<u8>, FrameError> {
    encode_request_parts(context, batch, RECONCILE, maximum_frame_bytes, maximum_commands)
}

fn encode_request_parts(
    context: &RequestContext,
    batch: &ClientStreamBatch,
    kind: u16,
    maximum_frame_bytes: usize,
    maximum_commands: usize,
) -> Result<Vec<u8>, FrameError> {
    let intent_len = batch
        .encoded_len_v1(usize::MAX, maximum_commands)
        .map_err(|source| FrameError::Intent { source })?;
    let frame_len = PREFIX_BYTES.checked_add(intent_len).ok_or(FrameError::LengthOverflow)?;
    check_bytes(frame_len, maximum_frame_bytes)?;
    let body_len = u32::try_from(frame_len - HEADER_BYTES).map_err(|_| FrameError::LengthOverflow)?;
    let mut bytes = Vec::with_capacity(frame_len);
    bytes.extend_from_slice(b"SVI0");
    bytes.extend_from_slice(&VERSION.to_le_bytes());
    bytes.extend_from_slice(&kind.to_le_bytes());
    bytes.extend_from_slice(&0_u32.to_le_bytes());
    bytes.extend_from_slice(&body_len.to_le_bytes());
    append_context(&mut bytes, context);
    batch
        .append_v1(&mut bytes, intent_len, maximum_commands)
        .map_err(|source| FrameError::Intent { source })?;
    Ok(bytes)
}

/// Validate a header before allocating/reading its body. The returned total
/// frame length is only a framing bound, not validation of the body or context.
pub fn request_frame_len(header: &[u8; HEADER_BYTES], maximum_frame_bytes: usize) -> Result<usize, FrameError> {
    decode_header(header, maximum_frame_bytes, &[SUBMIT, RECONCILE], MIN_FRAME_BYTES).map(|(_, length)| length)
}

/// Validate the complete frame, context and intent before allocating members.
/// The fixed header rejects retired versions before decoding their body.
pub fn decode_request(
    bytes: &[u8],
    maximum_frame_bytes: usize,
    maximum_commands: usize,
) -> Result<Request, FrameError> {
    check_bytes(bytes.len(), maximum_frame_bytes)?;
    let mut remaining = bytes;
    let (kind, frame_len) =
        decode_header(&take(&mut remaining)?, maximum_frame_bytes, &[SUBMIT, RECONCILE], MIN_FRAME_BYTES)?;
    if frame_len != bytes.len() {
        return Err(FrameError::InvalidLength);
    }
    let context = take_context(&mut remaining)?;
    let batch = ClientStreamBatch::decode_v1(remaining, maximum_frame_bytes - PREFIX_BYTES, maximum_commands)
        .map_err(|source| FrameError::Intent { source })?;
    Ok(match kind {
        SUBMIT => Request::Submit { context, batch },
        RECONCILE => Request::Reconcile { context, batch },
        // decode_header checked the kind before the body was decoded.
        _ => return Err(FrameError::UnsupportedKind { observed: kind }),
    })
}

fn decode_header(
    header: &[u8; HEADER_BYTES],
    maximum_frame_bytes: usize,
    kinds: &[u16],
    minimum: usize,
) -> Result<(u16, usize), FrameError> {
    let mut remaining = header.as_slice();
    if &take::<4>(&mut remaining)? != b"SVI0" {
        return Err(FrameError::InvalidMagic);
    }
    let version = u16::from_le_bytes(take(&mut remaining)?);
    if version != VERSION {
        return Err(FrameError::UnsupportedVersion { observed: version });
    }
    let kind = u16::from_le_bytes(take(&mut remaining)?);
    if !kinds.contains(&kind) {
        return Err(FrameError::UnsupportedKind { observed: kind });
    }
    if take::<4>(&mut remaining)? != [0; 4] {
        return Err(FrameError::Reserved);
    }
    let body_len =
        usize::try_from(u32::from_le_bytes(take(&mut remaining)?)).map_err(|_| FrameError::LengthOverflow)?;
    let frame_len = HEADER_BYTES.checked_add(body_len).ok_or(FrameError::LengthOverflow)?;
    check_bytes(frame_len, maximum_frame_bytes)?;
    if frame_len < minimum {
        return Err(FrameError::InvalidLength);
    }
    Ok((kind, frame_len))
}

fn append_context(bytes: &mut Vec<u8>, context: &RequestContext) {
    bytes.extend_from_slice(&context.cluster_identity.bytes());
    bytes.extend_from_slice(&context.genesis_identity.bytes());
    bytes.extend_from_slice(&context.execution_lineage.bytes());
    bytes.extend_from_slice(&context.session_incarnation.get().to_le_bytes());
    bytes.extend_from_slice(&context.request_id.get().to_le_bytes());
}

fn take_context(remaining: &mut &[u8]) -> Result<RequestContext, FrameError> {
    Ok(RequestContext {
        cluster_identity: Digest32::new(take(remaining)?),
        genesis_identity: Digest32::new(take(remaining)?),
        execution_lineage: Digest32::new(take(remaining)?),
        session_incarnation: NonZeroU128::new(u128::from_le_bytes(take(remaining)?))
            .ok_or(FrameError::ZeroSessionIncarnation)?,
        request_id: NonZeroU64::new(u64::from_le_bytes(take(remaining)?)).ok_or(FrameError::ZeroRequestId)?,
    })
}

fn check_bytes(observed: usize, limit: usize) -> Result<(), FrameError> {
    if limit == 0 {
        return Err(FrameError::EmptyByteLimit);
    }
    if observed > limit {
        return Err(FrameError::EncodedBytesExceeded { observed, limit });
    }
    Ok(())
}

fn take<const N: usize>(remaining: &mut &[u8]) -> Result<[u8; N], FrameError> {
    let (value, rest) = remaining.split_at_checked(N).ok_or(FrameError::InvalidLength)?;
    let value = value.try_into().map_err(|_| FrameError::InvalidLength)?;
    *remaining = rest;
    Ok(value)
}
