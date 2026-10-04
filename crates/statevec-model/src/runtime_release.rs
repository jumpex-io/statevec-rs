// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

//! Immutable core-runtime release identity.
//!
//! This value does not authorize loading, preparation, Raft proposal,
//! activation, or execution.

use serde::{Deserialize, Serialize};

/// Platform-independent digest of one complete canonical runtime release entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct CoreRuntimeReleaseId([u8; 32]);

impl CoreRuntimeReleaseId {
    /// Constructs an identity previously authenticated by a durable codec.
    pub const fn from_digest(digest: [u8; 32]) -> Self {
        Self(digest)
    }

    /// Returns the canonical release-entry digest.
    pub const fn digest(self) -> [u8; 32] {
        self.0
    }
}
