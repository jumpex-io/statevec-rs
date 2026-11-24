//! Ingress codec for whole-batch requests and replies, and direct
//! business reads. Retired versions have no fallback. The context binds
//! execution lineage, not the active Core version. Retained Completed results
//! on Reconcile keep the existing body layout.
//! Wire context is a declaration, not authenticated identity or admission proof.
//! The owner must join it to its checked cluster/lineage and current history.

use std::num::{NonZeroU64, NonZeroU128};

use statevec_frame::read_protocol::ReadRefusal;
use statevec_frame::wire::{WireError, WireFormat, WireHeader};
pub use statevec_frame::wire::HEADER_BYTES;
use statevec_frame::{CLIENT_STREAM_BATCH_HEADER_BYTES, ClientStreamBatch, ClientStreamBatchError, Digest32};

#[path = "stream_batch_read.rs"]
mod read;
#[path = "stream_batch_reader.rs"]
mod reader;
#[path = "stream_batch_refusal.rs"]
mod refusal;
#[path = "stream_batch_reply.rs"]
mod reply;
#[path = "stream_batch_routing.rs"]
mod routing;
#[cfg(test)]
#[path = "ut_reason_codes.rs"]
mod reason_tests;
pub use read::{
    READ_OBSERVATION_BYTES, ReadAnswer, ReadOutcome, ReadRecord, ReadRecordRef, ReadReply, ReadReplyBody, ReadRequest,
    decode_read_reply, decode_read_request, encode_read_reply, encode_read_request,
};
pub use reader::{FrameReader, IncompleteFrame, ReadError, ReadProgress, StreamDirection, StreamFrame, VerifiedFrame};
pub use refusal::IngressRefusal;
pub use routing::{RouteObservation, decode_route_reply, decode_route_request, encode_route_reply, encode_route_request};
pub use reply::{
    BatchSubmitRefusal, EntryBinding, HandoverAdvice, IngressUnavailableReason, MemberResult, Reply, ReplyBinding,
    ReplyDisposition, ReplyObservation, ReplyRequestError, decode_reply, encode_reply, reply_frame_len,
};

pub use statevec_frame::release::PROTOCOL_NUMBER as VERSION;
const INGRESS_MAGIC: [u8; 4] = *b"SVIN";
const INGRESS: WireFormat = WireFormat {
    magic: INGRESS_MAGIC,
    number: VERSION,
    kinds: &[SUBMIT, RECONCILE, reply::REPLY, read::READ, read::READ_REPLY, routing::ROUTE, routing::ROUTE_REPLY],
};
const PREFIX_BYTES: usize = statevec_frame::stream_batch_protocol::REQUEST_PREFIX_BYTES;
const MIN_FRAME_BYTES: usize = PREFIX_BYTES + CLIENT_STREAM_BATCH_HEADER_BYTES + 4;
const SUBMIT: u16 = 1;
const RECONCILE: u16 = 2;

pub use statevec_frame::stream_batch_protocol::{CONTEXT_BYTES, Request, RequestContext};

/// The `Read*` variants refuse a malformed read frame; the read codec's module
/// documentation states each field's rule.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum FrameError {
    EmptyByteLimit,
    EncodedBytesExceeded { observed: usize, limit: usize },
    LengthOverflow,
    InvalidLength,
    Wire { source: WireError },
    IngressProtocolUnsupported { refusal: IngressRefusal },
    IngressKindUnsupported { refusal: IngressRefusal },
    InvalidProtocolRange,
    ZeroSessionIncarnation,
    ZeroRequestId,
    Intent { source: ClientStreamBatchError },
    UnrecognizedDisposition { observed: u8, session_incarnation: NonZeroU128, request_id: NonZeroU64 },
    UnsupportedRefusal { observed: u8 },
    UnsupportedUnavailableReason { observed: u8 },
    UnencodableBatchRefusal { reason: BatchSubmitRefusal },
    UnencodableUnavailableReason { reason: IngressUnavailableReason },
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
    ReadKeyCount { observed: usize, limit: usize },
    ReadKeyLength { index: usize, observed: usize, limit: usize },
    UnsupportedReadKey { observed: u8 },
    InvalidReadKey,
    InvalidReadMinimum,
    MissingReadObservation,
    InvalidReadObservation,
    UnsupportedReadRole { observed: u8 },
    UnsupportedReadOutcome { observed: u8 },
    UnsupportedReadRefusal { observed: u8 },
    UnencodableReadRefusal { reason: ReadRefusal },
    NonClientReadRefusal,
    InvalidReadRefusal,
    InvalidReadRecord,
    ReadRecordBytes { observed: usize, limit: usize },
    InvalidRouteObservation,
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "whole-batch ingress: {self:?}")
    }
}

impl std::error::Error for FrameError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Wire { source } => Some(source),
            Self::Intent { source } => Some(source),
            Self::AppliedBinding { source } => Some(source),
            _ => None,
        }
    }
}

impl From<WireError> for FrameError {
    fn from(source: WireError) -> Self {
        Self::Wire { source }
    }
}

impl FrameError {
    pub fn ingress_refusal(&self) -> Option<IngressRefusal> {
        match self {
            Self::IngressProtocolUnsupported { refusal } | Self::IngressKindUnsupported { refusal } => Some(*refusal),
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
    let mut bytes = start_frame(frame_len, maximum_frame_bytes)?;
    append_context(&mut bytes, context);
    batch
        .append_v1(&mut bytes, intent_len, maximum_commands)
        .map_err(|source| FrameError::Intent { source })?;
    finish_frame(bytes, kind)
}

/// The bounded total length. Number, kind and contents are checked with the complete body.
pub fn request_frame_len(header: &[u8; HEADER_BYTES], maximum_frame_bytes: usize) -> Result<usize, FrameError> {
    frame_len(header, maximum_frame_bytes)
}

/// Validate the complete frame, context and intent before allocating members.
/// The checksum and supported tags precede every body interpretation.
pub fn decode_request(
    bytes: &[u8],
    maximum_frame_bytes: usize,
    maximum_commands: usize,
) -> Result<Request, FrameError> {
    let (kind, body) = verified_body(bytes, maximum_frame_bytes, &[SUBMIT, RECONCILE])?;
    request_body(kind, body, maximum_frame_bytes, maximum_commands)
}

fn request_body(kind: u16, mut remaining: &[u8], maximum_frame_bytes: usize, maximum_commands: usize) -> Result<Request, FrameError> {
    let context = take_context(&mut remaining)?;
    let batch = ClientStreamBatch::decode_v1(remaining, maximum_frame_bytes - PREFIX_BYTES, maximum_commands)
        .map_err(|source| FrameError::Intent { source })?;
    Ok(match kind {
        SUBMIT => Request::Submit { context, batch },
        RECONCILE => Request::Reconcile { context, batch },
        _ => return Err(WireError::UnsupportedKind { observed: kind }.into()),
    })
}

fn bounded_header(bytes: &[u8], maximum: usize, kinds: &'static [u16]) -> Result<WireHeader, FrameError> {
    if maximum == 0 {
        return Err(FrameError::EmptyByteLimit);
    }
    let header = WireFormat { kinds, ..INGRESS }.header(bytes, maximum.saturating_sub(HEADER_BYTES))?;
    check_bytes(HEADER_BYTES, maximum)?;
    Ok(header)
}

fn frame_len(header: &[u8; HEADER_BYTES], maximum: usize) -> Result<usize, FrameError> {
    let header = bounded_header(header, maximum, INGRESS.kinds)?;
    HEADER_BYTES.checked_add(header.body_len()).ok_or(FrameError::LengthOverflow)
}

fn verified_body<'a>(bytes: &'a [u8], maximum: usize, kinds: &'static [u16]) -> Result<(u16, &'a [u8]), FrameError> {
    if refusal::is_reply_direction(kinds) && bytes.starts_with(&refusal::REFUSAL_MAGIC) {
        return Err(IngressRefusal::decode(bytes, maximum)?.into_error());
    }
    let header = bounded_header(bytes, maximum, kinds)?;
    let body = &bytes[HEADER_BYTES..];
    let kind = header.verify(body)?;
    let minimum = match kind {
        SUBMIT | RECONCILE => MIN_FRAME_BYTES,
        reply::REPLY => statevec_frame::stream_batch_protocol::REPLY_BASE_BYTES,
        read::READ => read::MIN_REQUEST_BYTES,
        read::READ_REPLY => read::MIN_REPLY_BYTES,
        routing::ROUTE => PREFIX_BYTES,
        routing::ROUTE_REPLY => routing::MIN_REPLY_BYTES,
        _ => return Err(WireError::UnsupportedKind { observed: kind }.into()),
    };
    if bytes.len() < minimum {
        return Err(FrameError::InvalidLength);
    }
    Ok((kind, body))
}

fn start_frame(length: usize, maximum: usize) -> Result<Vec<u8>, FrameError> {
    check_bytes(length, maximum)?;
    u32::try_from(length - HEADER_BYTES).map_err(|_| FrameError::LengthOverflow)?;
    let mut bytes = Vec::with_capacity(length);
    bytes.resize(HEADER_BYTES, 0);
    Ok(bytes)
}

fn finish_frame(mut bytes: Vec<u8>, kind: u16) -> Result<Vec<u8>, FrameError> {
    let header = INGRESS.encode(kind, &bytes[HEADER_BYTES..], u32::MAX as usize)?;
    bytes[..HEADER_BYTES].copy_from_slice(&header);
    Ok(bytes)
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
