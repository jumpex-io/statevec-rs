// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

//! Host protocol and encoding support. Client applications use `statevec-ingress-client`.
//! No Raft log, checkpoint, peer protocol or execution implementation lives here.
//!
//! ```compile_fail,E0432
//! use statevec_frame::WholeStreamBatch;
//! ```
//!
//! ```compile_fail,E0432
//! use statevec_frame::CheckpointArtifact;
//! ```

#[doc(hidden)]
pub mod applied_binding;
mod clock;
#[doc(hidden)]
pub mod command;
#[doc(hidden)]
pub mod lifecycle;
#[doc(hidden)]
pub mod outcome;
#[doc(hidden)]
pub mod read_protocol;
mod stream_batch;
#[doc(hidden)]
pub mod stream_batch_protocol;
#[doc(hidden)]
pub mod release;
#[doc(hidden)]
pub mod wire;

/// Default node-local Raft Tick period shared by deployment and ingress clients.
#[doc(hidden)]
pub const DEFAULT_RAFT_TICK_MILLIS: std::num::NonZeroU64 = std::num::NonZeroU64::new(15).unwrap();

#[doc(hidden)]
pub use clock::{ClockReadError, MonotonicMillis};
#[doc(hidden)]
pub use statevec_model::{CoreVersion, Digest32};
#[doc(hidden)]
pub use stream_batch::{
    CLIENT_STREAM_BATCH_COMMITMENT_BYTES, CLIENT_STREAM_BATCH_HEADER_BYTES, ClientStreamBatch,
    ClientStreamBatchCommitment, ClientStreamBatchError,
};

#[cfg(test)]
mod ut_stream_batch;
#[cfg(test)]
mod ut_wire;

#[doc(hidden)]
pub use statevec_model::TxSeq;
#[doc(hidden)]
pub use payload::{
    Digest256, EngineCommandPayload, EngineInput, Hash, InputRef, PayloadCodec, RUNTIME_BINARY_V0_CODEC_ID,
    RUNTIME_BINARY_V0_CODEC_VERSION, TxChainHash, TxId,
};

// Apply doc visibility to the frame path so client re-exports can document these same types.
mod payload {
    use serde::{Deserialize, Serialize};
    use std::sync::Arc;

    /// 32-byte hash value.
    pub type Hash = [u8; 32];
    /// 32-byte digest value.
    pub type Digest256 = Hash;
    /// Transaction identifier.
    pub type TxId = [u8; 32];
    /// Commit-prefix chain hash.
    pub type TxChainHash = [u8; 32];

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
    pub const RUNTIME_BINARY_V0_CODEC_ID: &str = "runtime_binary";
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
        #[serde(with = "shared_payload_bytes")]
        pub payload_bytes: Arc<[u8]>,
    }

    // Only the byte sequence is serialized, not its shared allocation. Keep the
    // existing sequence representation (not serialize_bytes) without serde's rc
    // feature; ordinary payload clones still share the Arc allocation.
    mod shared_payload_bytes {
        use serde::{Deserialize, Deserializer, Serialize, Serializer};
        use std::sync::Arc;

        pub(super) fn serialize<S: Serializer>(bytes: &Arc<[u8]>, serializer: S) -> Result<S::Ok, S::Error> {
            bytes.as_ref().serialize(serializer)
        }

        pub(super) fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Arc<[u8]>, D::Error> {
            Box::<[u8]>::deserialize(deserializer).map(Arc::from)
        }
    }
}
