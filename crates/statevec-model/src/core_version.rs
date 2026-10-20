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
    pub const fn from_digest(digest: Digest32) -> Self {
        Self(digest)
    }

    pub const fn digest(self) -> Digest32 {
        self.0
    }
}
