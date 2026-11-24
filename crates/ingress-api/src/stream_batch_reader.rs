//! Mechanical custody of one partial whole-batch frame, not client/server admission.
use std::io::{self, Read};

#[cfg(test)]
#[path = "ut_stream_batch_reader.rs"]
mod tests;

use super::{
    FrameError, HEADER_BYTES, IngressRefusal, ReadReply, ReadRequest, Reply, Request, WireError,
    frame_len, read, refusal, reply, request_body, routing, verified_body,
};
use statevec_frame::read_protocol::{CLIENT_READ_LIMITS, ReadLimits};

const INITIAL_BODY_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamDirection {
    Requests,
    Replies,
}

impl StreamDirection {
    fn kinds(self) -> &'static [u16] {
        match self {
            Self::Requests => &[super::SUBMIT, super::RECONCILE, read::READ, routing::ROUTE],
            Self::Replies => &[reply::REPLY, read::READ_REPLY, routing::ROUTE_REPLY],
        }
    }
}

/// A complete ingress envelope with a verified checksum and direction.
/// The body is interpreted against its consumer's bounds when requested.
#[derive(Debug)]
pub struct VerifiedFrame {
    bytes: Vec<u8>,
    kind: Result<u16, IngressRefusal>,
}

impl VerifiedFrame {
    fn body(&self) -> &[u8] {
        &self.bytes[HEADER_BYTES..]
    }

    fn check_consumer(&self, maximum: usize, kind: u16) -> Result<(), FrameError> {
        let observed = match self.kind {
            Ok(observed) => observed,
            Err(refusal) => {
                super::check_bytes(self.bytes.len(), maximum)?;
                return Err(refusal.into_error());
            }
        };
        super::bounded_header(&self.bytes, maximum, super::INGRESS.kinds)?;
        if observed != kind {
            return Err(WireError::UnsupportedKind { observed }.into());
        }
        Ok(())
    }

    pub fn decode_reply(&self, maximum_frame_bytes: usize, maximum_commands: usize) -> Result<Reply, FrameError> {
        self.check_consumer(maximum_frame_bytes, reply::REPLY)?;
        reply::reply_body(self.body(), maximum_commands)
    }

    pub fn decode_read_reply(&self, maximum_frame_bytes: usize, limits: ReadLimits) -> Result<ReadReply, FrameError> {
        self.check_consumer(maximum_frame_bytes, read::READ_REPLY)?;
        read::read_reply_body(self.body(), limits)
    }

    pub fn is_route_reply(&self) -> bool {
        self.kind == Ok(routing::ROUTE_REPLY)
    }

    pub fn decode_route_reply(
        &self, maximum_frame_bytes: usize,
    ) -> Result<(super::RequestContext, routing::RouteObservation), FrameError> {
        self.check_consumer(maximum_frame_bytes, routing::ROUTE_REPLY)?;
        routing::route_reply_body(self.body())
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum StreamFrame {
    Request(Request),
    Reply(Reply),
    /// A direct client read, within the client read bounds.
    Read(ReadRequest),
    ReadReply(ReadReply),
    Route(super::RequestContext),
    RouteReply(super::RequestContext, routing::RouteObservation),
    /// The server checked the envelope and must write this refusal without
    /// admitting the request. An uncorrelated refusal closes after writing.
    IngressRefusal(IngressRefusal),
}

#[derive(Debug, PartialEq, Eq)]
pub enum ReadProgress<T = StreamFrame> {
    /// A short read or Interrupted permits another bounded poll without a new
    /// readiness edge. Yielding for fairness must preserve this runnable hint.
    Runnable,
    /// The device returned WouldBlock. Preserve partial bytes and wait for
    /// readability; repeatedly polling this result would busy-spin.
    WouldBlock,
    /// One whole frame. The device may already hold the next frame: continue
    /// bounded polling until WouldBlock instead of requiring another edge.
    Frame(T),
    /// Clean EOF at a frame boundary. The reader is now closed.
    Eof,
}

impl<T> ReadProgress<T> {
    fn try_map<U, E>(self, frame: impl FnOnce(T) -> Result<U, E>) -> Result<ReadProgress<U>, E> {
        Ok(match self {
            Self::Runnable => ReadProgress::Runnable,
            Self::WouldBlock => ReadProgress::WouldBlock,
            Self::Eof => ReadProgress::Eof,
            Self::Frame(value) => ReadProgress::Frame(frame(value)?),
        })
    }
}

#[derive(Debug)]
#[non_exhaustive]
pub enum ReadError {
    Frame { source: FrameError },
    /// An I/O failure or an expired reply deadline interrupted this frame.
    FrameRead { frame: FrameError, source: io::Error },
    Io { source: io::Error },
    Allocation { requested: usize, source: std::collections::TryReserveError },
    InvalidReadCount { observed: usize, offered: usize },
    EmptyCommandLimit,
    Closed,
}

impl std::fmt::Display for ReadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "whole-batch stream read: {self:?}")
    }
}

impl std::error::Error for ReadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Frame { source } => Some(source),
            Self::Io { source } | Self::FrameRead { source, .. } => Some(source),
            Self::Allocation { source, .. } => Some(source),
            _ => None,
        }
    }
}

enum PartialFrame {
    Header { bytes: [u8; HEADER_BYTES], filled: usize },
    Body { bytes: Vec<u8>, filled: usize, length: usize },
    Closed,
}

/// Byte counts for an unfinished frame, as observed by a `FrameReader`.
///
/// ```compile_fail
/// use ingress_api::stream_batch::{FrameError, IncompleteFrame};
/// let progress: IncompleteFrame = FrameError::InvalidLength.into();
/// ```
///
/// ```compile_fail
/// use ingress_api::stream_batch::IncompleteFrame;
/// let progress = IncompleteFrame { received: 0, declared_body: None };
/// ```
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct IncompleteFrame {
    received: usize,
    declared_body: Option<usize>,
}

impl From<IncompleteFrame> for FrameError {
    fn from(frame: IncompleteFrame) -> Self {
        match frame.declared_body {
            Some(declared) => WireError::IncompleteBody { received: frame.received, declared },
            None => WireError::IncompleteHeader { received: frame.received },
        }.into()
    }
}

/// A single direction, fixed-limit decoder for a persistent byte stream.
/// Each poll performs at most one `Read::read`, with no internal retry loop.
/// Use a nonblocking device for nonblocking polling. Interrupted/WouldBlock
/// preserve partial bytes. Any other error is terminal: the caller retires the
/// connection, never scans for a new magic or retries on a fresh decoder.
/// No body is allocated until the complete header passes its total-byte bound.
/// Initially reserve at most 64 KiB; grow fallibly as received bytes fill the
/// buffer, never from the header's declared length alone. The configured limit
/// still bounds the complete frame; callers must choose an affordable limit.
/// Runnable and Frame retain read readiness across scheduling yields. Only
/// WouldBlock requests an external readable notification; this is not a Future
/// and does not register a waker on behalf of its caller.
pub struct FrameReader {
    kinds: &'static [u16],
    maximum_frame_bytes: usize,
    maximum_commands: usize,
    partial: PartialFrame,
}

impl FrameReader {
    pub fn new(
        direction: StreamDirection,
        maximum_frame_bytes: usize,
        maximum_commands: usize,
    ) -> Result<Self, ReadError> {
        Self::with_kinds(direction.kinds(), maximum_frame_bytes, maximum_commands)
    }

    /// A batch connection also carries the replies to its routing probes.
    pub fn batch_replies(maximum_frame_bytes: usize, maximum_commands: usize) -> Result<Self, ReadError> {
        Self::with_kinds(&[reply::REPLY, routing::ROUTE_REPLY], maximum_frame_bytes, maximum_commands)
    }

    /// A connection that carries only direct-read replies.
    pub fn read_replies(maximum_frame_bytes: usize) -> Result<Self, ReadError> {
        Self::with_kinds(&[read::READ_REPLY], maximum_frame_bytes, 1)
    }

    fn with_kinds(kinds: &'static [u16], maximum_frame_bytes: usize, maximum_commands: usize) -> Result<Self, ReadError> {
        if maximum_frame_bytes == 0 {
            return Err(ReadError::Frame { source: FrameError::EmptyByteLimit });
        }
        if maximum_commands == 0 {
            return Err(ReadError::EmptyCommandLimit);
        }
        Ok(Self { kinds, maximum_frame_bytes, maximum_commands, partial: Self::header() })
    }

    fn header() -> PartialFrame {
        PartialFrame::Header { bytes: [0; HEADER_BYTES], filled: 0 }
    }

    /// The unfinished frame at the current cursor, for the connection's read deadline.
    pub fn incomplete_frame(&self) -> Option<FrameError> {
        self.incomplete_cursor().map(FrameError::from)
    }

    /// The byte position a driver may attach to its next scheduling turn.
    pub fn incomplete_cursor(&self) -> Option<IncompleteFrame> {
        match &self.partial {
            PartialFrame::Header { filled: 0, .. } | PartialFrame::Closed => None,
            PartialFrame::Header { filled, .. } => Some(IncompleteFrame { received: *filled, declared_body: None }),
            PartialFrame::Body { filled, length, .. } => Some(IncompleteFrame {
                received: filled - HEADER_BYTES,
                declared_body: Some(length - HEADER_BYTES),
            }),
        }
    }

    pub fn poll_read(&mut self, reader: &mut impl Read) -> Result<ReadProgress, ReadError> {
        self.poll_decoded(reader, false)
    }

    pub fn poll_request(&mut self, reader: &mut impl Read) -> Result<ReadProgress, ReadError> {
        self.poll_decoded(reader, true)
    }

    fn poll_decoded(&mut self, reader: &mut impl Read, refuse_requests: bool) -> Result<ReadProgress, ReadError> {
        let result = self.poll_envelope(reader, refuse_requests)?.try_map(|frame| {
            match frame.kind {
                Ok(kind @ (super::SUBMIT | super::RECONCILE)) => {
                    request_body(kind, frame.body(), self.maximum_frame_bytes, self.maximum_commands)
                        .map(StreamFrame::Request)
                }
                Ok(reply::REPLY) => reply::reply_body(frame.body(), self.maximum_commands).map(StreamFrame::Reply),
                Ok(read::READ) => read::read_request_body(frame.body(), CLIENT_READ_LIMITS).map(StreamFrame::Read),
                Ok(read::READ_REPLY) => read::read_reply_body(frame.body(), CLIENT_READ_LIMITS).map(StreamFrame::ReadReply),
                Ok(routing::ROUTE) => routing::route_body(frame.body()).map(StreamFrame::Route),
                Ok(routing::ROUTE_REPLY) => routing::route_reply_body(frame.body())
                    .map(|(context, observation)| StreamFrame::RouteReply(context, observation)),
                Ok(observed) => Err(WireError::UnsupportedKind { observed }.into()),
                Err(refusal) if refuse_requests => Ok(StreamFrame::IngressRefusal(refusal)),
                Err(refusal) => Err(refusal.into_error()),
            }.map_err(|source| ReadError::Frame { source })
        });
        if result.is_err() {
            self.partial = PartialFrame::Closed;
        }
        result
    }

    /// Extract the original bytes of one verified envelope without decoding the body.
    pub fn poll_bytes(&mut self, reader: &mut impl Read) -> Result<ReadProgress<Vec<u8>>, ReadError> {
        self.poll_frame(reader)?.try_map(|frame| Ok(frame.bytes))
    }

    /// Retain the verified envelope across a handoff to the body decoder.
    pub fn poll_frame(&mut self, reader: &mut impl Read) -> Result<ReadProgress<VerifiedFrame>, ReadError> {
        self.poll_envelope(reader, false)
    }

    fn poll_envelope(
        &mut self,
        reader: &mut impl Read,
        refuse_requests: bool,
    ) -> Result<ReadProgress<VerifiedFrame>, ReadError> {
        let result = self.read_once(reader).and_then(|progress| progress.try_map(|bytes| {
            let is_reply = refusal::is_reply_direction(self.kinds);
            let kind = if is_reply && bytes.starts_with(&refusal::REFUSAL_MAGIC) {
                Err(IngressRefusal::decode(&bytes, self.maximum_frame_bytes)
                    .map_err(|source| ReadError::Frame { source })?)
            } else {
                match verified_body(&bytes, self.maximum_frame_bytes, self.kinds) {
                    Ok((kind, _)) => Ok(kind),
                    Err(source) if refuse_requests && !is_reply => {
                        Err(refusal::from_checked_request(&bytes, &source).ok_or(ReadError::Frame { source })?)
                    }
                    Err(source) => return Err(ReadError::Frame { source }),
                }
            };
            Ok(VerifiedFrame { bytes, kind })
        }));
        if result.is_err() || matches!(result, Ok(ReadProgress::Eof)) {
            self.partial = PartialFrame::Closed;
        }
        result
    }

    fn read_once(&mut self, reader: &mut impl Read) -> Result<ReadProgress<Vec<u8>>, ReadError> {
        let (bytes, filled) = match &mut self.partial {
            PartialFrame::Header { bytes, filled } => (bytes.as_mut_slice(), filled),
            PartialFrame::Body { bytes, filled, length } => {
                if *filled == bytes.len() {
                    grow_body(bytes, bytes.len().saturating_mul(2).min(*length))?;
                }
                (bytes.as_mut_slice(), filled)
            }
            PartialFrame::Closed => return Err(ReadError::Closed),
        };
        let offered = bytes.len() - *filled;
        let read = match reader.read(&mut bytes[*filled..]) {
            Ok(read) if read > offered => return Err(ReadError::InvalidReadCount { observed: read, offered }),
            Ok(read) => read,
            Err(source) if source.kind() == io::ErrorKind::Interrupted => return Ok(ReadProgress::Runnable),
            Err(source) if source.kind() == io::ErrorKind::WouldBlock => return Ok(ReadProgress::WouldBlock),
            Err(source) if matches!(source.kind(), io::ErrorKind::UnexpectedEof | io::ErrorKind::TimedOut) => {
                return Err(match self.incomplete_frame() {
                    Some(frame) => ReadError::FrameRead { frame, source },
                    None => ReadError::Io { source },
                });
            }
            Err(source) => return Err(ReadError::Io { source }),
        };
        if read == 0 {
            return match self.incomplete_frame() {
                Some(source) => Err(ReadError::Frame { source }),
                None => Ok(ReadProgress::Eof),
            };
        }
        *filled += read;
        if *filled != bytes.len() {
            return Ok(ReadProgress::Runnable);
        }
        match &self.partial {
            PartialFrame::Header { bytes, .. } => {
                let length = if bytes.starts_with(&refusal::REFUSAL_MAGIC) && refusal::is_reply_direction(self.kinds) {
                    refusal::frame_length(bytes, self.maximum_frame_bytes)
                } else {
                    frame_len(bytes, self.maximum_frame_bytes)
                }.map_err(|source| ReadError::Frame { source })?;
                if length == HEADER_BYTES {
                    let complete = bytes.to_vec();
                    self.partial = Self::header();
                    return Ok(ReadProgress::Frame(complete));
                }
                let mut body = Vec::new();
                grow_body(&mut body, length.min(INITIAL_BODY_BYTES))?;
                body[..HEADER_BYTES].copy_from_slice(bytes);
                self.partial = PartialFrame::Body { bytes: body, filled: HEADER_BYTES, length };
                Ok(ReadProgress::Runnable)
            }
            PartialFrame::Body { filled, length, .. } if filled < length => Ok(ReadProgress::Runnable),
            PartialFrame::Body { .. } => {
                let PartialFrame::Body { bytes, .. } = std::mem::replace(&mut self.partial, Self::header()) else {
                    unreachable!("complete body")
                };
                Ok(ReadProgress::Frame(bytes))
            }
            PartialFrame::Closed => Err(ReadError::Closed),
        }
    }
}

fn grow_body(bytes: &mut Vec<u8>, length: usize) -> Result<(), ReadError> {
    bytes
        .try_reserve_exact(length - bytes.len())
        .map_err(|source| ReadError::Allocation { requested: length, source })?;
    bytes.resize(length, 0);
    Ok(())
}
