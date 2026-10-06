// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

//! Client intent, request/reply values and application evidence.
//! No Raft log, checkpoint, peer protocol or execution implementation lives here.
//!
//! ```compile_fail,E0432
//! use statevec_frame::{EngineLogEntry, CheckpointArtifact};
//! ```

use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub mod applied_binding;
mod clock;
mod digest;
pub mod outcome;
mod stream_batch;
pub mod stream_batch_protocol;

pub use clock::{ClockReadFailure, MonotonicMillis};
pub use digest::Digest32;
pub use stream_batch::{
    CLIENT_STREAM_BATCH_COMMITMENT_BYTES, CLIENT_STREAM_BATCH_HEADER_BYTES, ClientStreamBatch,
    ClientStreamBatchCommitment, ClientStreamBatchFailure,
};

#[cfg(test)]
mod ut_stream_batch;

/// 32-byte hash value.
pub type Hash = [u8; 32];
/// 32-byte digest value.
pub type Digest256 = Hash;
/// Transaction identifier.
pub type TxId = Digest256;
/// Canonical transaction sequence.
pub type TxSeq = u64;
/// Commit-prefix chain hash.
pub type TxChainHash = Hash;

/// Optional ingress stream evidence for an engine input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputRef {
    /// Auth/control-plane assigned client id.
    pub client_id: u64,
    /// Client-owned logical stream id.
    pub stream_id: u32,
    /// Strict per-stream sequence number.
    pub client_seq: u64,
    /// Optional request identifier.
    pub request_id: Option<String>,
}

/// Engine-local input shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineInput<C> {
    /// Optional ingress stream evidence.
    pub input_ref: Option<InputRef>,
    /// Domain or encoded command.
    pub command: C,
}

/// Logical codec identity for the sole current cluster command payload.
pub const RUNTIME_BINARY_V0_CODEC_ID: &str = "runtime_binary_v0";
/// Logical codec version; command-container tags preserve this complete tuple.
pub const RUNTIME_BINARY_V0_CODEC_VERSION: u16 = 0;

/// Payload codec identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PayloadCodec {
    /// Codec identifier. Known identities borrow their constant; the value and
    /// its serialized form are the same string either way.
    pub codec_id: std::borrow::Cow<'static, str>,
    /// Codec version.
    pub codec_version: u16,
}

impl PayloadCodec {
    /// The sole current cluster codec, without allocating its identifier.
    pub const fn runtime_binary_v0() -> Self {
        Self {
            codec_id: std::borrow::Cow::Borrowed(RUNTIME_BINARY_V0_CODEC_ID),
            codec_version: RUNTIME_BINARY_V0_CODEC_VERSION,
        }
    }

    /// Heap bytes retained by the identifier (zero when borrowed).
    pub fn retained_id_bytes(&self) -> usize {
        match &self.codec_id {
            std::borrow::Cow::Borrowed(_) => 0,
            std::borrow::Cow::Owned(id) => id.capacity(),
        }
    }
}

/// Encoded engine command payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineCommandPayload {
    /// Payload codec.
    pub payload_codec: PayloadCodec,
    /// Encoded payload bytes.
    pub payload_bytes: Arc<[u8]>,
}
