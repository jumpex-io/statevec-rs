use std::fmt;

use crate::model::{SYSTEM_RUNTIME_MANIFEST_RECORD_KIND, SYSTEM_SCHEMA_RECORD_KIND, Version};
use crate::registry::{SchemaIdentity, SchemaRegistry};

pub const SYSTEM_SCHEMA_METADATA_MAGIC: &[u8; 4] = b"SVSM";
pub const SYSTEM_SCHEMA_METADATA_VERSION: u16 = 2;
pub const SYSTEM_SCHEMA_ENCODING_CANONICAL_JSON: u8 = 0;
pub const SYSTEM_SCHEMA_ENCODING_CANONICAL_JSON_NAME: &str = "canonical-json";
pub const NONE_EFFECTIVE_TO_TX_SEQ: u64 = u64::MAX;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemSchemaMetadata {
    pub identity: SchemaIdentity,
    pub schema_encoding: u8,
    pub canonical_idl_sha256: [u8; 32],
    pub canonical_idl_bytes: u64,
    pub canonical_idl: Vec<u8>,
    pub effective_from_tx_seq: u64,
    pub effective_to_tx_seq: u64,
    pub runtime_manifest_bytes: u64,
    pub runtime_manifest: Vec<u8>,
}

impl SystemSchemaMetadata {
    pub fn schema_encoding_name(&self) -> String {
        match self.schema_encoding {
            SYSTEM_SCHEMA_ENCODING_CANONICAL_JSON => SYSTEM_SCHEMA_ENCODING_CANONICAL_JSON_NAME.to_string(),
            value => format!("unknown({value})"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemSchemaMetadataError(String);

impl SystemSchemaMetadataError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for SystemSchemaMetadataError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SystemSchemaMetadataError {}

pub fn encode_system_schema_metadata(registry: &SchemaRegistry) -> Vec<u8> {
    encode_system_schema_metadata_with_runtime_manifest(registry, &[])
}

pub fn encode_system_schema_metadata_with_runtime_manifest(
    registry: &SchemaRegistry,
    runtime_manifest: &[u8],
) -> Vec<u8> {
    let canonical_idl = registry.to_canonical_idl_bytes();
    let canonical_idl_sha256 = registry.canonical_idl_sha256();
    let runtime_manifest_len = runtime_manifest.len() as u64;
    let mut out = Vec::with_capacity(4 + 2 + 1 + 2 + 2 + 8 + 8 + 2 + 64 + 32 + 8 + canonical_idl.len() + 8);
    out.extend_from_slice(SYSTEM_SCHEMA_METADATA_MAGIC);
    out.extend_from_slice(&SYSTEM_SCHEMA_METADATA_VERSION.to_le_bytes());
    out.push(SYSTEM_SCHEMA_ENCODING_CANONICAL_JSON);
    out.extend_from_slice(&SYSTEM_SCHEMA_RECORD_KIND.to_le_bytes());
    out.extend_from_slice(&SYSTEM_RUNTIME_MANIFEST_RECORD_KIND.to_le_bytes());
    out.extend_from_slice(&0u64.to_le_bytes());
    out.extend_from_slice(&NONE_EFFECTIVE_TO_TX_SEQ.to_le_bytes());
    out.extend_from_slice(&registry.schema_version().to_bytes());
    out.extend_from_slice(&registry.record_schema_fingerprint());
    out.extend_from_slice(&registry.command_schema_fingerprint());
    out.extend_from_slice(&registry.event_schema_fingerprint());
    out.extend_from_slice(&registry.types_schema_fingerprint());
    out.extend_from_slice(&canonical_idl_sha256);
    out.extend_from_slice(&(canonical_idl.len() as u64).to_le_bytes());
    out.extend_from_slice(&canonical_idl);
    out.extend_from_slice(&runtime_manifest_len.to_le_bytes());
    out.extend_from_slice(runtime_manifest);
    out
}

pub fn parse_system_schema_metadata(bytes: &[u8]) -> Result<Option<SystemSchemaMetadata>, SystemSchemaMetadataError> {
    if bytes.is_empty() {
        return Ok(None);
    }
    let mut cursor = ByteCursor::new(bytes);
    cursor.expect_bytes(SYSTEM_SCHEMA_METADATA_MAGIC, "magic")?;
    let format_version = cursor.take_u16("system metadata version")?;
    if format_version != SYSTEM_SCHEMA_METADATA_VERSION {
        return Err(SystemSchemaMetadataError::new(format!("unsupported system metadata version {format_version}")));
    }
    let schema_encoding = cursor.take_u8("schema encoding")?;
    let schema_record_kind = cursor.take_u16("schema record kind")?;
    if schema_record_kind != SYSTEM_SCHEMA_RECORD_KIND {
        return Err(SystemSchemaMetadataError::new(format!("unexpected schema record kind {schema_record_kind}")));
    }
    let runtime_manifest_record_kind = cursor.take_u16("runtime manifest record kind")?;
    if runtime_manifest_record_kind != SYSTEM_RUNTIME_MANIFEST_RECORD_KIND {
        return Err(SystemSchemaMetadataError::new(format!(
            "unexpected runtime manifest record kind {runtime_manifest_record_kind}"
        )));
    }
    let effective_from_tx_seq = cursor.take_u64("effective_from_tx_seq")?;
    let effective_to_tx_seq = cursor.take_u64("effective_to_tx_seq")?;
    let schema_version = Version::from_bytes(cursor.take_array("schema_version")?);
    let record_schema_fingerprint = cursor.take_array("record schema fingerprint")?;
    let command_schema_fingerprint = cursor.take_array("command schema fingerprint")?;
    let event_schema_fingerprint = cursor.take_array("event schema fingerprint")?;
    let types_schema_fingerprint = cursor.take_array("types schema fingerprint")?;
    let canonical_idl_sha256 = cursor.take_array("canonical IDL SHA-256")?;
    let canonical_idl_bytes = cursor.take_u64("canonical IDL length")?;
    let canonical_idl_len = usize::try_from(canonical_idl_bytes)
        .map_err(|_| SystemSchemaMetadataError::new("canonical IDL length does not fit usize"))?;
    let canonical_idl = cursor.take(canonical_idl_len, "canonical IDL bytes")?.to_vec();
    let runtime_manifest_bytes = cursor.take_u64("runtime manifest length")?;
    let runtime_manifest_len = usize::try_from(runtime_manifest_bytes)
        .map_err(|_| SystemSchemaMetadataError::new("runtime manifest length does not fit usize"))?;
    let runtime_manifest = cursor.take(runtime_manifest_len, "runtime manifest bytes")?.to_vec();
    if cursor.remaining() != 0 {
        return Err(SystemSchemaMetadataError::new(format!(
            "trailing bytes after system metadata: {}",
            cursor.remaining()
        )));
    }

    Ok(Some(SystemSchemaMetadata {
        identity: SchemaIdentity {
            schema_version,
            types_schema_fingerprint,
            record_schema_fingerprint,
            command_schema_fingerprint,
            event_schema_fingerprint,
        },
        schema_encoding,
        canonical_idl_sha256,
        canonical_idl_bytes,
        canonical_idl,
        effective_from_tx_seq,
        effective_to_tx_seq,
        runtime_manifest_bytes,
        runtime_manifest,
    }))
}

struct ByteCursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> ByteCursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.offset)
    }

    fn expect_bytes(&mut self, expected: &[u8], label: &str) -> Result<(), SystemSchemaMetadataError> {
        let actual = self.take(expected.len(), label)?;
        if actual != expected {
            return Err(SystemSchemaMetadataError::new(format!("{label} mismatch")));
        }
        Ok(())
    }

    fn take_u8(&mut self, label: &str) -> Result<u8, SystemSchemaMetadataError> {
        Ok(self.take(1, label)?[0])
    }

    fn take_u16(&mut self, label: &str) -> Result<u16, SystemSchemaMetadataError> {
        Ok(u16::from_le_bytes(self.take_array(label)?))
    }

    fn take_u64(&mut self, label: &str) -> Result<u64, SystemSchemaMetadataError> {
        Ok(u64::from_le_bytes(self.take_array(label)?))
    }

    fn take_array<const N: usize>(&mut self, label: &str) -> Result<[u8; N], SystemSchemaMetadataError> {
        self.take(N, label)?
            .try_into()
            .map_err(|_| SystemSchemaMetadataError::new(format!("{label} length mismatch")))
    }

    fn take(&mut self, len: usize, label: &str) -> Result<&'a [u8], SystemSchemaMetadataError> {
        let end = self
            .offset
            .checked_add(len)
            .ok_or_else(|| SystemSchemaMetadataError::new(format!("{label} length overflow")))?;
        let Some(slice) = self.bytes.get(self.offset..end) else {
            return Err(SystemSchemaMetadataError::new(format!(
                "{label} truncated: need {} bytes at offset {}, remaining {}",
                len,
                self.offset,
                self.remaining()
            )));
        };
        self.offset = end;
        Ok(slice)
    }
}
