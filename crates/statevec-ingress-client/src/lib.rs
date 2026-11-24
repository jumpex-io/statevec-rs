//! Generic ingress client. The owner decides admission, reconciliation and
//! retirement; adapters and physical ports do not retain parallel policy.

mod client {
    mod adapter {
        include!("adapter.rs");
    }
    mod port {
        include!("port.rs");
    }
    include!("owner.rs");
}

mod settings;
pub use settings::{BatchClientSettings, DeploymentIdentity, ReadClientSettings};

#[doc(hidden)]
pub use client::{
    BATCH_CLIENT_HANDOVER_RETRY_DEFAULT_MS, BATCH_CLIENT_LIFETIME_DEFAULT_MS, BATCH_CLIENT_LIFETIME_MAX_MS,
    BATCH_CLIENT_LIFETIME_MIN_MS, BATCH_CLIENT_MAX_ENDPOINTS, BATCH_CLIENT_MAX_UNAVAILABLE_REPLIES,
    BATCH_CLIENT_OPERATION_DEFAULT_MS, BATCH_CLIENT_OPERATION_MAX_MS, BATCH_CLIENT_OPERATION_MIN_MS,
    BATCH_CLIENT_RETRY_DEFAULT_MS, BATCH_CLIENT_RETRY_MAX_MS, BATCH_CLIENT_RETRY_MIN_MS, BatchConnectIo, BatchIoWorker,
    ClientEvent, ClientResult, DrivenClient,
};
pub use client::{
    BatchClient, BatchDriverStep, BatchDriverWorker, BatchEvent, BatchEventResult, BatchHandle, BatchProgress,
    BatchProgressResult, BatchRefused, BatchRefusedResult, BatchTcpWorker, BatchTerminal, BatchTerminalResult,
    ClientError, ClientStatus, batch_monotonic_now,
};

#[doc(inline)]
pub use ingress_api::stream_batch::RouteObservation;
#[doc(inline)]
pub use statevec_frame::applied_binding::{AppliedStreamBatchBinding, AppliedStreamBatchBindingError};
#[doc(inline)]
pub use statevec_frame::command::{CommandPayloadError, encode_runtime_binary_command_payload};
#[doc(inline)]
pub use statevec_frame::outcome::{CommitEvidence, CommitOutcome, EntryDigest, InlineResponseEvent};
#[doc(inline)]
pub use statevec_frame::read_protocol::{ReadLimits, ReadObservationError};
#[doc(inline)]
pub use statevec_frame::stream_batch_protocol::{
    BatchSubmitRefusal, EntryBinding, IngressUnavailableReason, MemberResult, RequestContext,
};
#[doc(inline)]
pub use statevec_frame::{
    ClientStreamBatch, ClientStreamBatchCommitment, ClientStreamBatchError, ClockReadError, Digest32,
    EngineCommandPayload, InputRef, MonotonicMillis, PayloadCodec, TxChainHash, TxId, TxSeq,
};
#[doc(inline)]
pub use statevec_model::Command;

/// Protocol and kind in the encoder's finished request.
fn encoded_wire_tags(bytes: &[u8]) -> Result<(u16, u16), ingress_api::stream_batch::FrameError> {
    let header = bytes
        .first_chunk::<{ ingress_api::stream_batch::HEADER_BYTES }>()
        .ok_or(statevec_frame::wire::WireError::IncompleteHeader { received: bytes.len() })?;
    Ok((u16::from_le_bytes([header[4], header[5]]), u16::from_le_bytes([header[6], header[7]])))
}

/// Called for the attempt on the responding socket. A peer that does not
/// know the protocol may refuse the first unanswered frame without its ID.
fn matches_refusal(
    request_id: std::num::NonZeroU64,
    wire_tags: (u16, u16),
    refusal: ingress_api::stream_batch::IngressRefusal,
    first_unanswered: Option<std::num::NonZeroU64>,
) -> bool {
    wire_tags == (refusal.observed_protocol, refusal.observed_kind)
        && match refusal.request_id {
            Some(id) => request_id == id,
            None => {
                first_unanswered == Some(request_id)
                    && !(refusal.minimum_protocol..=refusal.maximum_protocol).contains(&refusal.observed_protocol)
            }
        }
}

/// The ordinary discovery cadence and the bounded, explicitly announced leader
/// handover cadence. The handover window spans four deployment Raft Ticks.
/// Later releases may add settings. Start from [`BatchRetryConfig::for_raft_tick`]
/// or `From<u64>` and assign fields.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct BatchRetryConfig {
    pub retry_millis: u64,
    pub handover_retry_millis: u64,
    pub raft_tick_millis: u64,
    /// Interval for a bounded round of routing probes after an unanswered write.
    pub routing_poll_millis: u64,
}

/// Cumulative routing discovery activity of one batch client. Use it to see
/// how often slow replies start discovery and how often a successor is found.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct RouteDiscoveryCounts {
    /// Probe connections started after a silent exchange.
    pub visits: u64,
    /// Visits that found a writable successor for the pending batch.
    pub successors: u64,
}

#[doc(hidden)]
pub const ROUTING_POLL_MIN_MS: u64 = 50;
#[doc(hidden)]
pub const ROUTING_POLL_MAX_MS: u64 = 500;
#[doc(hidden)]
pub const ROUTING_POLL_DEFAULT_MS: u64 = 150;

const HANDOVER_WINDOW_TICKS: u64 = 4;

impl BatchRetryConfig {
    /// Use the deployment's configured Raft Tick for the four-Tick handover deadline.
    pub fn for_raft_tick(retry_millis: u64, tick: std::num::NonZeroU64) -> Self {
        Self {
            retry_millis,
            handover_retry_millis: BATCH_CLIENT_HANDOVER_RETRY_DEFAULT_MS
                .min(tick.get().saturating_mul(HANDOVER_WINDOW_TICKS)),
            raft_tick_millis: tick.get(),
            routing_poll_millis: ROUTING_POLL_DEFAULT_MS,
        }
    }
}

/// Compatibility for callers using the shipping default Raft Tick.
impl From<u64> for BatchRetryConfig {
    fn from(retry_millis: u64) -> Self {
        Self::for_raft_tick(retry_millis, statevec_frame::DEFAULT_RAFT_TICK_MILLIS)
    }
}

// The read owner: its own domain beside the batch owner, over the same events,
// effects and physical ports, with no write stream.
mod read {
    mod adapter {
        include!("read_adapter.rs");
    }
    include!("read_client.rs");
}
pub use read::{
    CLIENT_READ_LIMITS, IngressReadOwner, IngressRefusal, ReadAnswer, ReadEvent, ReadEventResult, ReadFloorResult,
    ReadHandle, ReadKey, ReadObservation, ReadQuery, ReadRecord, ReadRefusal, ReadRefused, ReadRefusedResult, ReadRole,
    ReadSession, ReadStatus,
};

mod typed_record;
pub use typed_record::{TypedRecordError, typed_record};

// Scheduling composition, not another semantic owner or physical port. There
// is exactly one BatchClient behind the presentation's exclusion lock.
mod background;
#[doc(hidden)]
pub use background::Presented;
pub use background::{Background, BatchBackground, ReadBackground};
