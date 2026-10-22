//! Core semantic identity, independent of platform releases and package bytes.

/// Shared digest value, not an installed-root certificate or authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Digest32([u8; 32]);

impl Digest32 {
    pub const ZERO: Self = Self([0; 32]);

    pub const fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub const fn bytes(self) -> [u8; 32] {
        self.0
    }
}

/// Configured Core schema, semantic revision and actual rule-dispatch setting.
/// This names semantics, not a package, runtime instance or installation right.
/// Ordering is byte order for canonical sets, not an upgrade/version ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CoreVersion(Digest32);

impl CoreVersion {
    /// Frozen semantic-V preimage shared by plugin authoring and the engine.
    /// `revision` includes normalized execution-relevant rules; dispatch is
    /// bound independently. Raw configuration and package bytes are not inputs.
    /// Derivation does not attest that a runtime actually reports these values.
    pub fn derive(schema: crate::SchemaIdentity, revision: &str, has_tx_invariants: bool) -> Self {
        use sha2::{Digest, Sha256};
        let mut hash = Sha256::new();
        hash.update(b"statevec-core-version-v1");
        hash.update(schema.schema_version.to_bytes());
        hash.update(schema.types_schema_fingerprint);
        hash.update(schema.record_schema_fingerprint);
        hash.update(schema.command_schema_fingerprint);
        hash.update(schema.event_schema_fingerprint);
        hash.update((revision.len() as u64).to_le_bytes());
        hash.update(revision.as_bytes());
        hash.update([u8::from(has_tx_invariants)]);
        Self::from_digest(Digest32::new(hash.finalize().into()))
    }

    pub const fn from_digest(digest: Digest32) -> Self {
        Self(digest)
    }

    pub const fn digest(self) -> Digest32 {
        self.0
    }
}
