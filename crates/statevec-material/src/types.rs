//! Shared material identities and typed physical observations; no loader or store policy.
use statevec_model::{CoreVersion, Digest32};
use std::io;

/// Targets supported by this physical boundary. Each still needs load qualification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginTarget {
    X86_64LinuxGnu,
    Aarch64LinuxGnu,
}

impl PluginTarget {
    pub fn triple(self) -> &'static str {
        match self {
            Self::X86_64LinuxGnu => "x86_64-unknown-linux-gnu",
            Self::Aarch64LinuxGnu => "aarch64-unknown-linux-gnu",
        }
    }

    pub fn host() -> Result<Self, PluginMaterialFailure> {
        match (std::env::consts::ARCH, cfg!(all(target_env = "gnu", target_pointer_width = "64"))) {
            ("x86_64", true) => Ok(Self::X86_64LinuxGnu),
            ("aarch64", true) => Ok(Self::Aarch64LinuxGnu),
            _ => Err(PluginMaterialFailure::UnsupportedHost),
        }
    }

    pub fn machine(self) -> u16 {
        match self {
            Self::X86_64LinuxGnu => 62,
            Self::Aarch64LinuxGnu => 183,
        }
    }

    pub fn permits_soname(self, name: &[u8]) -> bool {
        matches!(
            name,
            b"libc.so.6" | b"libm.so.6" | b"libdl.so.2" | b"libpthread.so.0" | b"librt.so.1" | b"libgcc_s.so.1"
        ) || name
            == match self {
                Self::X86_64LinuxGnu => b"ld-linux-x86-64.so.2".as_slice(),
                Self::Aarch64LinuxGnu => b"ld-linux-aarch64.so.1".as_slice(),
            }
    }
}

/// SHA-256 of exact deployable bytes, separate from the business execution profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PluginArtifactDigest(pub [u8; 32]);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginMaterialIoStep {
    OpenSource,
    SourceMetadata,
    OpenValidatedSource,
    CreateMemfd,
    ReadSource,
    WriteMemfd,
    Seal,
    InspectSeals,
    HashSealed,
    ReadElf,
}

#[derive(Debug)]
pub enum PluginMaterialFailure {
    UnsupportedHost,
    TargetMismatch { requested: PluginTarget, host: PluginTarget },
    Io { step: PluginMaterialIoStep, source: io::Error },
    NotRegularFile,
    EmptyArtifact,
    TooLarge { observed: u64, limit: u64 },
    ReadCountBeyondBuffer { observed: usize, available: usize },
    IncompleteSeals,
    DigestMismatch { expected: PluginArtifactDigest, observed: PluginArtifactDigest },
    Elf(PluginElfFailure),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginElfFailure {
    InvalidHeader,
    UnsupportedEncoding,
    NotSharedObject,
    MachineMismatch { expected: u16, observed: u16 },
    InvalidProgramTable,
    FileRange { offset: u64, length: u64 },
    InvalidLoadRange,
    InterpreterPresent,
    MissingDynamic,
    DuplicateDynamic,
    InvalidDynamicLength,
    MissingDynamicTerminator,
    ForbiddenDynamicTag { tag: u64 },
    PositionIndependentExecutable,
    DuplicateStringTableField { tag: u64 },
    MissingStringTable,
    UnmappedMetadata,
    AmbiguousMetadata,
    InvalidDependencyName { offset: u64 },
    DependencyNotAllowed { offset: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CoreMaterialId {
    pub core_version: CoreVersion,
    pub package_digest: Digest32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CoreArtifact {
    pub target: PluginTarget,
    pub digest: PluginArtifactDigest,
    pub length: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaterialIoStep {
    Open,
    /// The namespace inode was pinned; reopening its data fd is unavailable.
    ReopenPinned,
    Read,
    Write,
    Metadata,
    SyncFile,
    SyncDirectory,
    /// Namespace preparation; this call has not attempted publication.
    SyncBeforePublication,
    /// Published name is visible; its durability is not yet confirmed.
    SyncAfterPublication,
    /// Reconfirm an already visible package before binding or retry success.
    SyncExistingPublication,
    TemporaryName,
    CreateDirectory,
    Lock,
    Publish,
    Bind,
    List,
    RemoveTemporary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManifestField {
    Format,
    Abi,
    CoreVersion,
    Schema,
    Revision,
    Configuration,
    Artifacts,
    Hex,
    Decimal,
}

#[derive(Debug)]
pub enum CoreMaterialError {
    Io { step: MaterialIoStep, source: io::Error },
    NotRegular,
    TooLarge { observed: u64, limit: u64 },
    Truncated,
    Header { offset: u64 },
    Padding { offset: u64 },
    TrailingBytes,
    ManifestJson { source: serde_json::Error },
    Manifest { field: ManifestField },
    Digest { expected: Digest32, actual: Digest32 },
    Artifact(PluginMaterialFailure),
    MissingTarget { target: PluginTarget },
    MissingPackage { material: CoreMaterialId },
    MissingSelection { version: CoreVersion, target: PluginTarget },
    InvalidSelection,
    SelectionChanged,
    NamespaceLimit,
    PageLimit,
}
