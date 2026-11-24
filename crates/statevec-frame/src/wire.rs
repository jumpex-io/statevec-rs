// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

//! The bounded prefix shared by the StateVec streaming wire formats.

pub const HEADER_BYTES: usize = 16;

/// A family declares its accepted format number and kinds. Body limits can
/// differ between its readers, for example for admin requests and replies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WireFormat {
    pub magic: [u8; 4],
    pub number: u16,
    pub kinds: &'static [u16],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum WireError {
    IncompleteHeader { received: usize },
    WrongMagic { observed: [u8; 4] },
    BodyTooLarge { observed: usize, limit: usize },
    IncompleteBody { received: usize, declared: usize },
    Checksum { stored: u32, computed: u32 },
    UnsupportedNumber { observed: u16 },
    UnsupportedKind { observed: u16 },
    TrailingBytes { count: usize },
}

impl std::fmt::Display for WireError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "wire frame {self:?}")
    }
}

impl std::error::Error for WireError {}

/// The family magic and declared body length have been checked. Only the
/// length is available until the body has passed the checksum and tag checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WireHeader {
    format: WireFormat,
    bytes: [u8; HEADER_BYTES],
}

impl WireFormat {
    /// Encode the prefix for a complete body, including an empty one.
    pub fn encode(self, kind: u16, body: &[u8], body_limit: usize) -> Result<[u8; HEADER_BYTES], WireError> {
        let limit = body_limit.min(u32::MAX as usize);
        if body.len() > limit {
            return Err(WireError::BodyTooLarge { observed: body.len(), limit });
        }
        if !self.kinds.contains(&kind) {
            return Err(WireError::UnsupportedKind { observed: kind });
        }
        let mut bytes = [0; HEADER_BYTES];
        bytes[..4].copy_from_slice(&self.magic);
        bytes[4..6].copy_from_slice(&self.number.to_le_bytes());
        bytes[6..8].copy_from_slice(&kind.to_le_bytes());
        bytes[8..12].copy_from_slice(&(body.len() as u32).to_le_bytes());
        let check = checksum(&bytes[..12], body);
        bytes[12..].copy_from_slice(&check.to_le_bytes());
        Ok(bytes)
    }

    /// Inspect only the first header, before reserving any space for the body.
    pub fn header(self, bytes: &[u8], body_limit: usize) -> Result<WireHeader, WireError> {
        let Some(prefix) = bytes.first_chunk::<HEADER_BYTES>() else {
            return Err(WireError::IncompleteHeader { received: bytes.len() });
        };
        let magic = [prefix[0], prefix[1], prefix[2], prefix[3]];
        if magic != self.magic {
            return Err(WireError::WrongMagic { observed: magic });
        }
        let header = WireHeader { format: self, bytes: *prefix };
        let observed = header.body_len();
        if observed > body_limit {
            return Err(WireError::BodyTooLarge { observed, limit: body_limit });
        }
        Ok(header)
    }

    /// Decode a buffer whose caller requires exactly one frame.
    pub fn decode_exact(self, bytes: &[u8], body_limit: usize) -> Result<(u16, &[u8]), WireError> {
        let header = self.header(bytes, body_limit)?;
        let body = &bytes[HEADER_BYTES..];
        let kind = header.verify(body)?;
        Ok((kind, body))
    }
}

impl WireHeader {
    pub fn body_len(&self) -> usize {
        u32::from_le_bytes([self.bytes[8], self.bytes[9], self.bytes[10], self.bytes[11]]) as usize
    }

    /// Check the checksum before interpreting the number or kind. Stream
    /// readers supply exactly the declared bytes and keep the next prefix.
    pub fn verify(&self, body: &[u8]) -> Result<u16, WireError> {
        let declared = self.body_len();
        let Some(frame) = body.get(..declared) else {
            return Err(WireError::IncompleteBody { received: body.len(), declared });
        };
        let stored = u32::from_le_bytes([self.bytes[12], self.bytes[13], self.bytes[14], self.bytes[15]]);
        let computed = checksum(&self.bytes[..12], frame);
        if computed != stored {
            return Err(WireError::Checksum { stored, computed });
        }
        let number = u16::from_le_bytes([self.bytes[4], self.bytes[5]]);
        if number != self.format.number {
            return Err(WireError::UnsupportedNumber { observed: number });
        }
        let kind = u16::from_le_bytes([self.bytes[6], self.bytes[7]]);
        if !self.format.kinds.contains(&kind) {
            return Err(WireError::UnsupportedKind { observed: kind });
        }
        if body.len() > declared {
            return Err(WireError::TrailingBytes { count: body.len() - declared });
        }
        Ok(kind)
    }
}

fn checksum(prefix: &[u8], body: &[u8]) -> u32 {
    let mut hasher = crc32fast::Hasher::new();
    hasher.update(prefix);
    hasher.update(body);
    hasher.finalize()
}
