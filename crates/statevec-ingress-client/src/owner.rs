// INV-CN-COMMAND-IDENTITY-OUTCOME: this owner controls only local intent.
// Same-intent recovery never invents a new identity or erases known commitment.
pub use port::{
    BatchConnectIo, BatchDriverStep, BatchDriverWorker, BatchIoWorker, BatchTcpWorker, batch_monotonic_now,
};
use statevec_domain_roles::{
    domain_actor, domain_core_state, domain_event, domain_failure, domain_initial_owner_creation, domain_invariant,
    domain_projection, domain_result, domain_transition, exclusive_domain_owner,
};
use statevec_frame::applied_binding::AppliedStreamBatchBindingV1;
use statevec_frame::stream_batch_protocol::{
    BatchSubmitRefusal, EntryBinding, IngressUnavailableReason, MemberResult, ReplyDisposition, ReplyRequestFailure,
    RequestContext,
};
use statevec_frame::{ClientStreamBatch, ClientStreamBatchFailure, Digest32, InputRef};
pub use statevec_frame::{ClockReadFailure, MonotonicMillis};
use std::net::SocketAddr;
use std::num::{NonZeroU64, NonZeroU128};
use std::sync::Arc;

/// The single memory-only interpretation retains all canonical/custody facts.
#[domain_core_state]
enum ClientState {
    Batch {
        context: RequestContext,
        stream: InputRef,
        endpoints: Vec<(u64, SocketAddr)>,
        maximum_commands: u32,
        maximum_frame_bytes: u32,
        retry_millis: u64,
        operation_millis: u64,
        lifetime_millis: u64,
        unavailable_reply_limit: u32,
        // No-progress accounting for the selected endpoint visit, never
        // evidence about history or permission to retransmit.
        unavailable_replies: u32,
        // A physical clock source, not another lifecycle or retry owner.
        clock: Box<dyn FnMut() -> Result<MonotonicMillis, ClockReadFailure> + Send>,
        last_now: MonotonicMillis,
        next_connection_id: u64,
        connection: BatchConnection,
        slot: Option<BatchSlot>,
    },
}

impl std::fmt::Debug for ClientState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Batch { context, connection, slot, unavailable_replies, .. } => f
                .debug_struct("Batch")
                .field("context", context)
                .field("connection", connection)
                .field("slot", slot)
                .field("unavailable_replies", unavailable_replies)
                .finish_non_exhaustive(),
        }
    }
}

/// One stream's memory-only request owner, never cloneable or externally constructible.
/// Recovery requires caller-retained original intent through `try_import`; there
/// is no SDK resume file or frontier-based reconstruction.
/// ```compile_fail,E0599
/// fn duplicate(client: statevec_ingress_client::IngressClientOwner) { let _second = client.clone(); }
/// ```
/// ```compile_fail,E0603
/// use statevec_ingress_client::client::ClientState;
/// ```
/// ```compile_fail,E0599
/// // A lost process cannot reconstruct custody from a legacy SDK directory.
/// let _resume = statevec_ingress_client::IngressClientOwner::resume_with_io;
/// ```
#[derive(Debug)]
#[domain_actor]
#[exclusive_domain_owner]
#[must_use = "retain the client and its pending request; dropping an in-memory client loses that request"]
pub struct IngressClientOwner {
    state: ClientState,
}

/// Physical observations and driving. Acceptance, import and consumption use
/// only `try_submit`, `try_import` and `poll_event`.
/// ```compile_fail,E0599
/// fn batch_adoption_is_not_a_drive_event(batch: statevec_frame::ClientStreamBatch) {
///     let _event = statevec_ingress_client::ClientEvent::AdoptBatch { batch };
/// }
/// ```
#[derive(Debug)]
#[domain_event]
pub enum ClientEvent {
    /// The entry adapter samples the session's bound clock at processing time.
    Drive,
    /// Err means the failed connect has returned all physical socket custody.
    Connected {
        operation_id: u64,
        result: std::io::Result<()>,
    },
    /// All socket/partial-I/O custody for this operation has actually retired.
    Closed {
        operation_id: u64,
    },
    /// One complete bounded frame from this connection, or its read failure.
    /// Framing/short reads remain mechanical; the owner decodes and classifies.
    Read {
        operation_id: u64,
        result: Result<Vec<u8>, ingress_api::stream_batch::ReadFailure>,
    },
    /// Return of the exact full-frame Write effect. Partial writes stay with
    /// the driver until completion or connection disposal; they are not events.
    Written {
        operation_id: u64,
        request_id: NonZeroU64,
        result: std::io::Result<()>,
    },
    Stop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[domain_projection]
pub enum ClientStatus {
    Batch {
        occupied: usize,
        connected: bool,
        /// No more admission; physical Close may still be outstanding.
        stopped: bool,
        /// Earliest timer or immediately runnable owner effect. None permits
        /// waiting for physical observations, not discarding occupied slots.
        next_deadline: Option<MonotonicMillis>,
    },
}

#[derive(Debug)]
#[domain_failure]
pub enum ClientFailure {
    InvalidIdentity,
    InvalidLimit,
    InvalidEndpoints,
    InvalidStream,
    InvalidTime {
        source: ClockReadFailure,
    },
    TimeRegressed {
        previous: MonotonicMillis,
        observed: MonotonicMillis,
    },
    Full,
    NotConnected,
    Stopped,
    RequestLifetimeExpired,
    TooLarge {
        source: ClientStreamBatchFailure,
    },
    BatchSequence {
        expected: u64,
        observed: u64,
    },
    InvalidEvent,
    InvalidInput,
    /// A stream cursor cannot advance; it is unrelated to local correlation IDs.
    SequenceExhausted,
    RequestIdExhausted,
    ConnectionIdExhausted,
    BatchReservationInvariant,
    BatchConnectionInvariant,
    BatchFrame {
        source: ingress_api::stream_batch::FrameFailure,
    },
    BatchReply {
        source: ReplyRequestFailure,
    },
    BatchRead {
        source: ingress_api::stream_batch::ReadFailure,
    },
    BatchWrite {
        source: std::io::Error,
    },
    BatchConnect {
        source: std::io::Error,
    },
    BatchWait {
        source: std::io::Error,
    },
    DriverSpawn {
        source: std::io::Error,
    },
    DriverPanicked,
    WaiterPanicked,
    ConflictingCommittedEntry {
        known: EntryBinding,
        observed: EntryBinding,
    },
    InsufficientAppliedEvidence {
        next_sequence: u64,
    },
    BatchConflict,
    CommittedHistoryMissing {
        known: EntryBinding,
    },
    ServerRefused {
        reason: BatchSubmitRefusal,
    },
    StreamGap {
        expected_sequence: u64,
    },
}

#[derive(Debug)]
#[domain_result]
#[must_use = "execute the effect or retain the unconsumed observation"]
pub enum ClientResult {
    /// Mechanical effects from drive(); the caller still owns the borrowed client.
    Connect {
        operation_id: u64,
        peer_id: u64,
        address: SocketAddr,
    },
    Close {
        operation_id: u64,
    },
    /// One exact encoded operation, never a second copy of batch authority.
    /// Drive will not issue another write until Written or actual Closed.
    Write {
        operation_id: u64,
        request_id: NonZeroU64,
        bytes: Vec<u8>,
    },
    /// The I/O/frame observation was consumed. Drive owes a Close effect;
    /// the real connection and partial buffers have NOT yet been retired.
    ConnectionFault {
        operation_id: u64,
        cause: ClientFailure,
    },
    Waiting {
        wake_at: Option<MonotonicMillis>,
    },
    /// Consumed, including actual socket retirement. Never replay the event.
    /// Invalid time stops the session: its unresolved terminal keeps the time
    /// cause; this diagnostic preserves the physical cause with no next wake.
    ConnectionFailed {
        operation_id: u64,
        source: std::io::Error,
        next_wake: Option<MonotonicMillis>,
    },
    /// Invalid time stopped the session. The observation was consumed, never
    /// replayed; physical Close/Closed must still drain before teardown.
    ClockFailed {
        cause: ClientFailure,
    },
    /// The event was not consumed. Time observation/expiry may still advance;
    /// rejection never claims the entire owner was rolled back.
    DriveRejected {
        event: ClientEvent,
        cause: ClientFailure,
    },
}

impl IngressClientOwner {
    pub fn status(&self) -> ClientStatus {
        self.batch_status()
    }
}

// Same domain root: no additional owner, adapter-held slots or completion queue.
include!("batch_client.rs");
