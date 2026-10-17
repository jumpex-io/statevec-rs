//! Portable consumer metadata. Decoding this value never constructs a live cut.
pub const MAX_PROJECTOR_ID_BYTES: usize = 128;
pub const MAX_OFFSET_PROPERTIES_BYTES: usize = 1024;

/// An application's durable whole-entry prefix, not a host acknowledgement.
/// Save alongside business effects before offering the corresponding live cut.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsumeOffset {
    pub cluster_id: String,
    pub projector_id: String,
    pub raft_term: u64,
    pub raft_index: u64,
}

/// Checked logical namespace, copied into native source certificates. This is
/// an identity value, not frame custody or proof of an acknowledged offset.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ProjectorId {
    length: u8,
    bytes: [u8; MAX_PROJECTOR_ID_BYTES],
}

impl ProjectorId {
    pub fn new(value: &str) -> Result<Self, ConsumeOffsetError> {
        validate_projector_id(value)?;
        let mut bytes = [0; MAX_PROJECTOR_ID_BYTES];
        bytes[..value.len()].copy_from_slice(value.as_bytes());
        Ok(Self { length: value.len() as u8, bytes })
    }

    pub fn as_str(&self) -> &str {
        std::str::from_utf8(&self.bytes[..usize::from(self.length)])
            .expect("private checked UTF-8 identity")
    }
}

impl std::fmt::Debug for ProjectorId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("ProjectorId").field(&self.as_str()).finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// A known offset field, used in diagnostics without retaining input values.
pub enum OffsetField { ClusterId, ProjectorId, RaftTerm, RaftIndex }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ConsumeOffsetError {
    TooLarge { actual: usize, maximum: usize },
    Utf8 { valid_up_to: usize },
    PropertySyntax { line: usize },
    UnknownProperty { line: usize },
    DuplicateProperty { field: OffsetField },
    MissingProperty { field: OffsetField },
    InvalidInteger { field: OffsetField },
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

    /// Encode exactly four LF-terminated UTF-8 `key=value` lines in field order.
    /// Values are literal: no trimming, escapes, comments or continuation lines.
    pub fn encode_properties(&self) -> Result<Vec<u8>, ConsumeOffsetError> {
        self.validate()?;
        let bytes = format!(
            "cluster_id={}\nprojector_id={}\nraft_term={}\nraft_index={}\n",
            self.cluster_id, self.projector_id, self.raft_term, self.raft_index,
        ).into_bytes();
        check_length(bytes.len())?;
        Ok(bytes)
    }

    /// Decode four unique fields in any order, bounded before allocation.
    /// Every line ends with LF and splits at its first `=`; names and values
    /// are not trimmed or unescaped. Blank/comment lines, missing/unknown keys,
    /// CRLF, BOM, signs and leading-zero integers are not part of this format.
    /// This is a small fixed schema, not the Java Properties grammar.
    pub fn decode_properties(bytes: &[u8]) -> Result<Self, ConsumeOffsetError> {
        check_length(bytes.len())?;
        let text = std::str::from_utf8(bytes)
            .map_err(|error| ConsumeOffsetError::Utf8 { valid_up_to: error.valid_up_to() })?;
        let body = text.strip_suffix('\n').ok_or(ConsumeOffsetError::PropertySyntax {
            line: bytes.iter().filter(|&&b| b == b'\n').count() + 1,
        })?;
        let (mut cluster_id, mut projector_id, mut raft_term, mut raft_index) = (None, None, None, None);
        for (index, line) in body.split('\n').enumerate() {
            let line_number = index + 1;
            let (key, value) = line.split_once('=')
                .ok_or(ConsumeOffsetError::PropertySyntax { line: line_number })?;
            let (field, slot) = match key {
                "cluster_id" => (OffsetField::ClusterId, &mut cluster_id),
                "projector_id" => (OffsetField::ProjectorId, &mut projector_id),
                "raft_term" => (OffsetField::RaftTerm, &mut raft_term),
                "raft_index" => (OffsetField::RaftIndex, &mut raft_index),
                _ => return Err(ConsumeOffsetError::UnknownProperty { line: line_number }),
            };
            if slot.replace(value).is_some() {
                return Err(ConsumeOffsetError::DuplicateProperty { field });
            }
        }
        let offset = Self {
            cluster_id: required(cluster_id, OffsetField::ClusterId)?.into(),
            projector_id: required(projector_id, OffsetField::ProjectorId)?.into(),
            raft_term: integer(required(raft_term, OffsetField::RaftTerm)?, OffsetField::RaftTerm)?,
            raft_index: integer(required(raft_index, OffsetField::RaftIndex)?, OffsetField::RaftIndex)?,
        };
        offset.validate()?;
        Ok(offset)
    }
}

fn check_length(actual: usize) -> Result<(), ConsumeOffsetError> {
    if actual > MAX_OFFSET_PROPERTIES_BYTES {
        Err(ConsumeOffsetError::TooLarge { actual, maximum: MAX_OFFSET_PROPERTIES_BYTES })
    } else { Ok(()) }
}

fn required(value: Option<&str>, field: OffsetField) -> Result<&str, ConsumeOffsetError> {
    value.ok_or(ConsumeOffsetError::MissingProperty { field })
}

fn integer(value: &str, field: OffsetField) -> Result<u64, ConsumeOffsetError> {
    let invalid = ConsumeOffsetError::InvalidInteger { field };
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit())
        || (value.len() > 1 && value.starts_with('0')) {
        return Err(invalid);
    }
    // Preserve the field, never the rejected input or its rendered parse error.
    value.parse().map_err(|_| invalid)
}

#[cfg(test)]
#[path = "ut_offset.rs"]
mod tests;
