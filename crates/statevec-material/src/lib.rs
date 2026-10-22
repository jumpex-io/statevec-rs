//! Canonical Core material authoring. Static checks never execute a plugin or certify its actual V.
//! The server reuses this codec, then independently seals, hashes, loads and checks actual metadata.
#![forbid(unsafe_code)]
use statevec_model::{CoreVersion, Digest32, SchemaIdentity};

mod manifest;
mod types;
pub mod tar;
pub use types::*;
pub use manifest::{decode_declaration, decode_manifest, encode_manifest};

#[cfg(target_os = "linux")]
mod pack;
#[cfg(target_os = "linux")]
pub use pack::pack;
#[cfg(target_os = "linux")]
pub mod authoring;
#[cfg(target_os = "linux")]
pub mod artifact_io;
#[cfg(target_os = "linux")]
pub mod elf;
#[cfg(target_os = "linux")]
pub mod package_io;

pub const MAX_MANIFEST_BYTES: usize = 16 * 1024;
pub const MAX_PACKAGE_BYTES: u64 = 514 * 1024 * 1024;
pub const TRANSFER_BYTES: usize = 64 * 1024;
pub const MAX_CORE_CONFIG_BYTES: usize = 1024 * 1024;
pub const MAX_CORE_REVISION_BYTES: usize = 4096;
pub const MAX_PLUGIN_ARTIFACT_BYTES: u64 = 256 * 1024 * 1024;

/// Author declarations. `pack` computes configuration/artifact hashes itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CorePackageDeclaration {
    pub schema_identity: SchemaIdentity,
    pub execution_revision: String,
    pub has_tx_invariants: bool,
}

impl CorePackageDeclaration {
    pub fn core_version(&self) -> CoreVersion {
        CoreVersion::derive(
            self.schema_identity,
            &self.execution_revision,
            self.has_tx_invariants,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CorePackageManifest {
    pub declaration: CorePackageDeclaration,
    pub configuration_digest: Digest32,
    pub configuration_length: u64,
    pub artifacts: Vec<CoreArtifact>,
}

pub fn target_name(target: PluginTarget) -> &'static str {
    match target {
        PluginTarget::X86_64LinuxGnu => "artifacts/x86_64-unknown-linux-gnu/core.so",
        PluginTarget::Aarch64LinuxGnu => "artifacts/aarch64-unknown-linux-gnu/core.so",
    }
}

pub fn parse_target(value: &str) -> Result<PluginTarget, CoreMaterialError> {
    match value {
        "x86_64-unknown-linux-gnu" => Ok(PluginTarget::X86_64LinuxGnu),
        "aarch64-unknown-linux-gnu" => Ok(PluginTarget::Aarch64LinuxGnu),
        _ => Err(CoreMaterialError::Manifest { field: ManifestField::Artifacts }),
    }
}

pub fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    bytes
        .iter()
        .flat_map(|b| [char::from(DIGITS[usize::from(b >> 4)]), char::from(DIGITS[usize::from(b & 15)])])
        .collect()
}

pub fn unhex<const N: usize>(text: &str) -> Result<[u8; N], CoreMaterialError> {
    let invalid = || CoreMaterialError::Manifest { field: ManifestField::Hex };
    if text.len() != N * 2 {
        return Err(invalid());
    }
    let mut result = [0; N];
    for (dst, pair) in result.iter_mut().zip(text.as_bytes().chunks_exact(2)) {
        let digit = |b| match b {
            b'0'..=b'9' => Ok(b - b'0'),
            b'a'..=b'f' => Ok(b - b'a' + 10),
            _ => Err(invalid()),
        };
        *dst = digit(pair[0])? * 16 + digit(pair[1])?;
    }
    Ok(result)
}
