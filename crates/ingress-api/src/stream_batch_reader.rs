//! Mechanical custody of one partial whole-batch frame, not client/server admission.
use std::io::{self, Read};

use super::{
    FrameFailure, HEADER_BYTES, Reply, Request, decode_reply, decode_request, reply_frame_len, request_frame_len,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamDirection {
    Requests,
    Replies,
}

#[derive(Debug, PartialEq, Eq)]
pub enum StreamFrame {
    Request(Request),
    Reply(Reply),
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

#[derive(Debug)]
pub enum ReadFailure {
    Frame { source: FrameFailure },
    Io { source: io::Error },
    InvalidReadCount { observed: usize, offered: usize },
    EmptyCommandLimit,
    Closed,
}

impl std::fmt::Display for ReadFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "whole-batch stream read: {self:?}")
    }
}

impl std::error::Error for ReadFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Frame { source } => Some(source),
            Self::Io { source } => Some(source),
            _ => None,
        }
    }
}

enum PartialFrame {
    Header { bytes: [u8; HEADER_BYTES], filled: usize },
    Body { bytes: Vec<u8>, filled: usize },
    Closed,
}

/// A single direction, fixed-limit decoder for a persistent byte stream.
/// Each poll performs at most one `Read::read`, with no internal retry loop.
/// Use a nonblocking device for nonblocking polling. Interrupted/WouldBlock
/// preserve partial bytes. Any other error is terminal: the caller retires the
/// connection, never scans for a new magic or retries on a fresh decoder.
/// No body is allocated until the complete header passes its total-byte bound.
/// Runnable and Frame retain read readiness across scheduling yields. Only
/// WouldBlock requests an external readable notification; this is not a Future
/// and does not register a waker on behalf of its caller.
pub struct FrameReader {
    direction: StreamDirection,
    maximum_frame_bytes: usize,
    maximum_commands: usize,
    partial: PartialFrame,
}

impl FrameReader {
    pub fn new(
        direction: StreamDirection,
        maximum_frame_bytes: usize,
        maximum_commands: usize,
    ) -> Result<Self, ReadFailure> {
        if maximum_frame_bytes == 0 {
            return Err(ReadFailure::Frame { source: FrameFailure::EmptyByteLimit });
        }
        if maximum_commands == 0 {
            return Err(ReadFailure::EmptyCommandLimit);
        }
        Ok(Self { direction, maximum_frame_bytes, maximum_commands, partial: Self::header() })
    }

    fn header() -> PartialFrame {
        PartialFrame::Header { bytes: [0; HEADER_BYTES], filled: 0 }
    }

    pub fn poll_read(&mut self, reader: &mut impl Read) -> Result<ReadProgress, ReadFailure> {
        let result = match self.poll_bytes(reader) {
            Ok(ReadProgress::Frame(bytes)) => match self.direction {
                StreamDirection::Requests => {
                    decode_request(&bytes, self.maximum_frame_bytes, self.maximum_commands).map(StreamFrame::Request)
                }
                StreamDirection::Replies => {
                    decode_reply(&bytes, self.maximum_frame_bytes, self.maximum_commands).map(StreamFrame::Reply)
                }
            }
            .map(ReadProgress::Frame)
            .map_err(|source| ReadFailure::Frame { source }),
            Ok(ReadProgress::Runnable) => Ok(ReadProgress::Runnable),
            Ok(ReadProgress::WouldBlock) => Ok(ReadProgress::WouldBlock),
            Ok(ReadProgress::Eof) => Ok(ReadProgress::Eof),
            Err(source) => Err(source),
        };
        if result.is_err() {
            self.partial = PartialFrame::Closed;
        }
        result
    }

    /// Extract one bounded frame without interpreting its body. Header/version/
    /// length validation still precedes allocation. The returned bytes are NOT a
    /// validated request or reply: the receiving semantic owner must run the
    /// production decoder before using them. This avoids decode/re-encode in a
    /// physical driver whose owner retains unconsumed read observations.
    pub fn poll_bytes(&mut self, reader: &mut impl Read) -> Result<ReadProgress<Vec<u8>>, ReadFailure> {
        let result = self.read_once(reader);
        if result.is_err() || matches!(result, Ok(ReadProgress::Eof)) {
            self.partial = PartialFrame::Closed;
        }
        result
    }

    fn read_once(&mut self, reader: &mut impl Read) -> Result<ReadProgress<Vec<u8>>, ReadFailure> {
        let (bytes, filled) = match &mut self.partial {
            PartialFrame::Header { bytes, filled } => (bytes.as_mut_slice(), filled),
            PartialFrame::Body { bytes, filled } => (bytes.as_mut_slice(), filled),
            PartialFrame::Closed => return Err(ReadFailure::Closed),
        };
        let offered = bytes.len() - *filled;
        let read = match reader.read(&mut bytes[*filled..]) {
            Ok(read) if read > offered => return Err(ReadFailure::InvalidReadCount { observed: read, offered }),
            Ok(read) => read,
            Err(source) if source.kind() == io::ErrorKind::Interrupted => return Ok(ReadProgress::Runnable),
            Err(source) if source.kind() == io::ErrorKind::WouldBlock => return Ok(ReadProgress::WouldBlock),
            Err(source) => return Err(ReadFailure::Io { source }),
        };
        if read == 0 {
            return if matches!(self.partial, PartialFrame::Header { filled: 0, .. }) {
                Ok(ReadProgress::Eof)
            } else {
                Err(ReadFailure::Io { source: io::Error::from(io::ErrorKind::UnexpectedEof) })
            };
        }
        *filled += read;
        if *filled != bytes.len() {
            return Ok(ReadProgress::Runnable);
        }
        match &self.partial {
            PartialFrame::Header { bytes, .. } => {
                let length = match self.direction {
                    StreamDirection::Requests => request_frame_len(bytes, self.maximum_frame_bytes),
                    StreamDirection::Replies => reply_frame_len(bytes, self.maximum_frame_bytes),
                }
                .map_err(|source| ReadFailure::Frame { source })?;
                let mut body = vec![0; length];
                body[..HEADER_BYTES].copy_from_slice(bytes);
                self.partial = PartialFrame::Body { bytes: body, filled: HEADER_BYTES };
                Ok(ReadProgress::Runnable)
            }
            PartialFrame::Body { .. } => {
                let PartialFrame::Body { bytes, .. } = std::mem::replace(&mut self.partial, Self::header()) else {
                    unreachable!("complete body")
                };
                Ok(ReadProgress::Frame(bytes))
            }
            PartialFrame::Closed => Err(ReadFailure::Closed),
        }
    }
}
