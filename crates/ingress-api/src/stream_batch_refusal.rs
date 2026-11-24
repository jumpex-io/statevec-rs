//! Frozen ingress refusal. This frame is independent of ingress protocol numbers.

use std::num::NonZeroU64;

use statevec_frame::release::FROZEN_WIRE_NUMBER;
use statevec_frame::wire::{WireError, WireFormat};

use super::{FrameError, HEADER_BYTES, VERSION, check_bytes};

pub(super) const REFUSAL_MAGIC: [u8; 4] = *b"SVIU";
const FORMAT: WireFormat = WireFormat { magic: REFUSAL_MAGIC, number: FROZEN_WIRE_NUMBER, kinds: &[1] };
const BODY_BYTES: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct IngressRefusal {
    pub observed_protocol: u16,
    pub observed_kind: u16,
    pub request_id: Option<NonZeroU64>,
    /// Inclusive minimum and maximum client wire protocols.
    pub minimum_protocol: u16,
    pub maximum_protocol: u16,
}

impl IngressRefusal {
    pub const fn new(
        observed_protocol: u16,
        observed_kind: u16,
        request_id: Option<NonZeroU64>,
        minimum_protocol: u16,
        maximum_protocol: u16,
    ) -> Self {
        Self { observed_protocol, observed_kind, request_id, minimum_protocol, maximum_protocol }
    }

    pub(super) fn for_request(prefix: &[u8; HEADER_BYTES], request_id: Option<NonZeroU64>) -> Self {
        Self::new(
            u16::from_le_bytes([prefix[4], prefix[5]]),
            u16::from_le_bytes([prefix[6], prefix[7]]),
            request_id,
            VERSION,
            VERSION,
        )
    }

    pub fn encode(self) -> [u8; HEADER_BYTES + BODY_BYTES] {
        let mut bytes = [0; HEADER_BYTES + BODY_BYTES];
        bytes[16..18].copy_from_slice(&self.observed_protocol.to_le_bytes());
        bytes[18..20].copy_from_slice(&self.observed_kind.to_le_bytes());
        bytes[20..28].copy_from_slice(&self.request_id.map_or(0, NonZeroU64::get).to_le_bytes());
        bytes[28..30].copy_from_slice(&self.minimum_protocol.to_le_bytes());
        bytes[30..32].copy_from_slice(&self.maximum_protocol.to_le_bytes());
        let header = FORMAT.encode(1, &bytes[HEADER_BYTES..], BODY_BYTES).expect("fixed refusal size and kind");
        bytes[..HEADER_BYTES].copy_from_slice(&header);
        bytes
    }

    pub(super) fn decode(bytes: &[u8], maximum: usize) -> Result<Self, FrameError> {
        check_bytes(bytes.len(), maximum)?;
        let (_, mut body) = FORMAT.decode_exact(bytes, BODY_BYTES)?;
        let observed_protocol = u16::from_le_bytes(super::take(&mut body)?);
        let observed_kind = u16::from_le_bytes(super::take(&mut body)?);
        let request_id = NonZeroU64::new(u64::from_le_bytes(super::take(&mut body)?));
        let minimum_protocol = u16::from_le_bytes(super::take(&mut body)?);
        let maximum_protocol = u16::from_le_bytes(super::take(&mut body)?);
        if minimum_protocol > maximum_protocol {
            return Err(FrameError::InvalidProtocolRange);
        }
        Ok(Self::new(observed_protocol, observed_kind, request_id, minimum_protocol, maximum_protocol))
    }

    pub(super) fn into_error(self) -> FrameError {
        if (self.minimum_protocol..=self.maximum_protocol).contains(&self.observed_protocol) {
            FrameError::IngressKindUnsupported { refusal: self }
        } else {
            FrameError::IngressProtocolUnsupported { refusal: self }
        }
    }
}

pub(super) fn frame_length(prefix: &[u8; HEADER_BYTES], maximum: usize) -> Result<usize, FrameError> {
    let header = FORMAT.header(prefix, BODY_BYTES)?;
    let total = HEADER_BYTES + header.body_len();
    check_bytes(total, maximum)?;
    Ok(total)
}

pub(super) fn is_reply_direction(kinds: &[u16]) -> bool {
    kinds.contains(&super::reply::REPLY) || kinds.contains(&super::read::READ_REPLY)
        || kinds.contains(&super::routing::ROUTE_REPLY)
}

/// Only the complete verified request reader can create a refusal from these
/// errors. The caller has read exactly the declared body and checked its CRC.
pub(super) fn from_checked_request(bytes: &[u8], error: &FrameError) -> Option<IngressRefusal> {
    let prefix = bytes.first_chunk::<HEADER_BYTES>()?;
    let request_id = match error {
        FrameError::Wire { source: WireError::UnsupportedNumber { .. } } => None,
        FrameError::Wire { source: WireError::UnsupportedKind { .. } } => {
            let end = super::PREFIX_BYTES;
            bytes.get(end - 8..end)
                .and_then(|raw| raw.try_into().ok())
                .and_then(|raw| NonZeroU64::new(u64::from_le_bytes(raw)))
        }
        _ => return None,
    };
    Some(IngressRefusal::for_request(prefix, request_id))
}
