//! Portable consumer metadata. Decoding this value never constructs a live cut.
use serde::{Deserialize, Serialize};

pub const MAX_PROJECTOR_ID_BYTES: usize = 128;
pub const MAX_OFFSET_JSON_BYTES: usize = 1024;

/// An application's durable whole-entry prefix, not a host acknowledgement.
/// Save alongside business effects before offering the corresponding live cut.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsumeOffset {
    pub cluster_id: String,
    pub projector_id: String,
    pub raft_term: u64,
    pub raft_index: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum OffsetJsonCategory { Io, Syntax, Data, Eof }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ConsumeOffsetError {
    TooLarge { actual: usize, maximum: usize },
    Json { category: OffsetJsonCategory, line: usize, column: usize },
    ClusterIdentity,
    ProjectorIdLength { actual: usize },
    EmbeddedNul,
    ControlCharacter { scalar: u32 },
    ZeroTerm,
    ZeroIndex,
    ClusterMismatch,
    ProjectorMismatch,
}

impl std::fmt::Display for ConsumeOffsetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "consume offset: {self:?}") }
}
impl std::error::Error for ConsumeOffsetError {}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OffsetFields {
    cluster_id: String,
    projector_id: String,
    raft_term: u64,
    raft_index: u64,
}

pub(crate) fn validate_projector_id(value: &str) -> Result<(), ConsumeOffsetError> {
    if value.is_empty() || value.len() > MAX_PROJECTOR_ID_BYTES {
        return Err(ConsumeOffsetError::ProjectorIdLength { actual: value.len() });
    }
    for scalar in value.chars() {
        if scalar == '\0' { return Err(ConsumeOffsetError::EmbeddedNul); }
        if scalar.is_control() {
            return Err(ConsumeOffsetError::ControlCharacter { scalar: scalar.into() });
        }
    }
    Ok(())
}

impl ConsumeOffset {
    /// Check canonical spelling and nonzero complete-entry coordinates.
    pub fn validate(&self) -> Result<(), ConsumeOffsetError> {
        if self.cluster_id.len() != 64 || !self.cluster_id.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
            return Err(ConsumeOffsetError::ClusterIdentity);
        }
        validate_projector_id(&self.projector_id)?;
        if self.raft_term == 0 { return Err(ConsumeOffsetError::ZeroTerm); }
        if self.raft_index == 0 { return Err(ConsumeOffsetError::ZeroIndex); }
        Ok(())
    }

    /// Compare exact logical source names; never trim, case-fold or normalize.
    pub fn check_source(&self, cluster_id: &str, projector_id: &str) -> Result<(), ConsumeOffsetError> {
        self.validate()?;
        if self.cluster_id != cluster_id { return Err(ConsumeOffsetError::ClusterMismatch); }
        if self.projector_id != projector_id { return Err(ConsumeOffsetError::ProjectorMismatch); }
        Ok(())
    }

    pub fn encode_json(&self) -> Result<Vec<u8>, ConsumeOffsetError> {
        self.validate()?;
        let fields = OffsetFields {
            cluster_id: self.cluster_id.clone(), projector_id: self.projector_id.clone(),
            raft_term: self.raft_term, raft_index: self.raft_index,
        };
        let bytes = serde_json::to_vec(&fields).map_err(json_failure)?;
        check_length(bytes.len())?;
        Ok(bytes)
    }

    /// Checks the complete input bound before JSON allocation. Duplicate/unknown
    /// fields, non-integer coordinates and trailing data are refused.
    pub fn decode_json(bytes: &[u8]) -> Result<Self, ConsumeOffsetError> {
        check_length(bytes.len())?;
        let fields: OffsetFields = serde_json::from_slice(bytes).map_err(json_failure)?;
        let offset = Self { cluster_id: fields.cluster_id, projector_id: fields.projector_id, raft_term: fields.raft_term, raft_index: fields.raft_index };
        offset.validate()?;
        Ok(offset)
    }
}

fn check_length(actual: usize) -> Result<(), ConsumeOffsetError> {
    if actual > MAX_OFFSET_JSON_BYTES {
        Err(ConsumeOffsetError::TooLarge { actual, maximum: MAX_OFFSET_JSON_BYTES })
    } else { Ok(()) }
}

fn json_failure(error: serde_json::Error) -> ConsumeOffsetError {
    use serde_json::error::Category;
    let category = match error.classify() { Category::Io => OffsetJsonCategory::Io, Category::Syntax => OffsetJsonCategory::Syntax, Category::Data => OffsetJsonCategory::Data, Category::Eof => OffsetJsonCategory::Eof };
    // Never retain rendered JSON errors: they can contain application values.
    ConsumeOffsetError::Json { category, line: error.line(), column: error.column() }
}

#[cfg(test)]
#[path = "ut_offset.rs"]
mod tests;
