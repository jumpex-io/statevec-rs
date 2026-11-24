use super::ProjectorAbiError as A;
use super::ffi::{SvConsumeOffset, SvProjectorId};
use crate::offset::validate_projector_id;
use crate::{ConsumeOffset, ConsumeOffsetError};

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
        let cluster = crate::offset::cluster_bytes(&value.cluster_id)?;
        Ok(Self { cluster, projector_id: SvProjectorId::encode(&value.projector_id)?, raft_term: value.raft_term, raft_index: value.raft_index })
    }

    pub fn decode(&self) -> Result<ConsumeOffset, A> {
        let projector_id = self.projector_id.decode()?;
        if self.raft_term == 0 { return Err(A::ZeroTerm); }
        if self.raft_index == 0 { return Err(A::ZeroIndex); }
        let cluster_id = super::inputs::cluster_id(self.cluster);
        Ok(ConsumeOffset { cluster_id, projector_id: projector_id.to_owned(), raft_term: self.raft_term, raft_index: self.raft_index })
    }
}
