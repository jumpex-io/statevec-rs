use super::{ProjectorAbiFailure as A, ffi::{SvConsumeOffset, SvProjectorId}};
use crate::{ConsumeOffset, ConsumeOffsetError, offset::validate_projector_id};

impl SvProjectorId {
    pub fn encode(value: &str) -> Result<Self, ConsumeOffsetError> {
        validate_projector_id(value)?;
        let mut result = Self { len: value.len() as u64, ..Self::default() };
        result.bytes[..value.len()].copy_from_slice(value.as_bytes());
        Ok(result)
    }

    pub fn decode(&self) -> Result<&str, A> {
        if self.len == 0 || self.len > 128 { return Err(A::Length { actual: self.len, maximum: 128 }); }
        if let Some(byte) = self.bytes[self.len as usize..].iter().find(|b| **b != 0) {
            return Err(A::Reserved { actual: u64::from(*byte) });
        }
        let value = std::str::from_utf8(&self.bytes[..self.len as usize]).map_err(|_| A::Utf8)?;
        validate_projector_id(value).map_err(|cause| match cause {
            ConsumeOffsetError::EmbeddedNul => A::EmbeddedNul,
            ConsumeOffsetError::ControlCharacter { scalar } => A::ControlCharacter { scalar },
            ConsumeOffsetError::ProjectorIdLength { actual } => A::Length { actual: actual as u64, maximum: 128 },
            _ => unreachable!("ID validation only returns ID causes"),
        })?;
        Ok(value)
    }
}

impl SvConsumeOffset {
    pub fn encode(value: &ConsumeOffset) -> Result<Self, ConsumeOffsetError> {
        value.validate()?;
        let mut cluster = [0; 32];
        for (out, pair) in cluster.iter_mut().zip(value.cluster_id.as_bytes().chunks_exact(2)) {
            let nibble = |b: u8| if b <= b'9' { b - b'0' } else { b - b'a' + 10 };
            *out = nibble(pair[0]) * 16 + nibble(pair[1]);
        }
        Ok(Self { cluster, projector_id: SvProjectorId::encode(&value.projector_id)?, raft_term: value.raft_term, raft_index: value.raft_index })
    }

    pub fn decode(&self) -> Result<ConsumeOffset, A> {
        let projector_id = self.projector_id.decode()?;
        if self.raft_term == 0 || self.raft_index == 0 { return Err(A::Flag { actual: 0 }); }
        let mut cluster_id = String::with_capacity(64);
        const HEX: &[u8; 16] = b"0123456789abcdef";
        for byte in self.cluster {
            cluster_id.push(HEX[usize::from(byte >> 4)] as char);
            cluster_id.push(HEX[usize::from(byte & 15)] as char);
        }
        Ok(ConsumeOffset { cluster_id, projector_id: projector_id.to_owned(), raft_term: self.raft_term, raft_index: self.raft_index })
    }
}
