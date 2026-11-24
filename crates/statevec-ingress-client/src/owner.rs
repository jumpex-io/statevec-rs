// INV-CN-COMMAND-IDENTITY-OUTCOME: this owner controls only local intent.
// Same-intent recovery never invents a new identity or erases known commitment.
pub use port::{
    BatchConnectIo, BatchDriverStep, BatchDriverWorker, BatchIoWorker, BatchTcpWorker, batch_monotonic_now,
};
use ingress_api::stream_batch::IncompleteFrame;
use statevec_frame::applied_binding::AppliedStreamBatchBinding;
use statevec_frame::stream_batch_protocol::{
    BatchSubmitRefusal, EntryBinding, IngressUnavailableReason, MemberResult, ReplyDisposition, ReplyRequestError,
    RequestContext,
};
use statevec_frame::{ClientStreamBatch, ClientStreamBatchError, InputRef};
pub use statevec_frame::{ClockReadError, MonotonicMillis};
use std::net::SocketAddr;
use std::num::NonZeroU64;
use std::sync::Arc;

/// The single memory-only interpretation retains all canonical/custody facts.
enum ClientState {
    Batch {
        context: RequestContext,
        stream: InputRef,
        // Raft index of the exact result that advanced this stream cursor.
        // A caller-supplied starting sequence has no such evidence.
        previous_applied_index: u64,
        endpoints: Vec<(u64, SocketAddr)>,
        maximum_commands: u32,
        maximum_frame_bytes: u32,
        maximum_outcome_bytes: u32,
        retry_millis: u64,
        handover_retry_millis: u64,
        handover_window_millis: u64,
        observed_leader: (u64, Option<NonZeroU64>),
        routing_poll_millis: u64,
        route_observation: Option<crate::RouteObservation>,
        // Cumulative discovery visits and writable successors found.
        route_visits: u64,
        route_successors: u64,
        route_probe: RouteProbe,
        // Idle connection failures back off; accepted batch retry timing is unchanged.
        idle_reconnect_millis: u64,
        operation_millis: u64,
        lifetime_millis: u64,
        unavailable_reply_limit: u32,
        // No-progress accounting for the selected endpoint visit, never
        // evidence about history or permission to retransmit.
        unavailable_replies: u32,
        // A physical clock source, not another lifecycle or retry owner.
        clock: Box<dyn FnMut() -> Result<MonotonicMillis, ClockReadError> + Send>,
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
/// fn duplicate(client: statevec_ingress_client::BatchClient) { let _second = client.clone(); }
/// ```
/// ```compile_fail,E0603
/// use statevec_ingress_client::client::ClientState;
/// ```
/// ```compile_fail,E0599
/// // A lost process cannot reconstruct custody from a legacy SDK directory.
/// let _resume = statevec_ingress_client::BatchClient::resume_with_io;
/// ```
#[derive(Debug)]
#[must_use = "retain the client and its pending request; dropping an in-memory client loses that request"]
pub struct BatchClient {
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
    /// Actual socket disposal also certifies zero bytes for this exact Write.
    ClosedUnwritten {
        operation_id: u64,
        request_id: NonZeroU64,
    },
    /// One complete bounded frame from this connection, or its read failure.
    /// Framing/short reads remain mechanical; the owner decodes and classifies.
    Read {
        operation_id: u64,
        result: Result<Vec<u8>, ingress_api::stream_batch::ReadError>,
    },
    /// The physical reader checked the full envelope. The owner still decodes
    /// the body, checks its limits and associates the reply with its exchange.
    VerifiedRead {
        operation_id: u64,
        frame: ingress_api::stream_batch::VerifiedFrame,
    },
    /// This connection still holds an incomplete reply at a scheduling turn.
    /// A concurrent completed write returns its exact request id on the same turn.
    PartialReply {
        operation_id: u64,
        cursor: IncompleteFrame,
        written: Option<NonZeroU64>,
    },
    /// Return of the exact full-frame Write effect. Partial writes stay with
    /// the driver until completion or connection disposal; they are not events.
    Written {
        operation_id: u64,
        request_id: NonZeroU64,
        result: std::io::Result<()>,
    },
    /// The physical Write failed before accepting any bytes, including prefixes.
    WriteUnwritten {
        operation_id: u64,
        request_id: NonZeroU64,
        source: std::io::Error,
    },
    Stop,
}

impl ClientEvent {
    fn operation_id(&self) -> Option<u64> {
        match self {
            Self::Connected { operation_id, .. }
            | Self::Closed { operation_id }
            | Self::ClosedUnwritten { operation_id, .. }
            | Self::Read { operation_id, .. }
            | Self::VerifiedRead { operation_id, .. }
            | Self::PartialReply { operation_id, .. }
            | Self::Written { operation_id, .. }
            | Self::WriteUnwritten { operation_id, .. } => Some(*operation_id),
            Self::Drive | Self::Stop => None,
        }
    }

    pub(crate) fn partial_reply(&self) -> Option<(u64, IncompleteFrame)> {
        match self {
            Self::PartialReply { operation_id, cursor, .. } => Some((*operation_id, *cursor)),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientStatus {
    #[non_exhaustive]
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
#[non_exhaustive]
pub enum ClientError {
    InvalidIdentity,
    InvalidLimit,
    InvalidEndpoints,
    InvalidStream,
    InvalidTime {
        source: ClockReadError,
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
        source: ClientStreamBatchError,
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
        source: ingress_api::stream_batch::FrameError,
    },
    /// A read request outside the frame or read bounds, or a read reply that
    /// does not decode under them.
    ReadFrame {
        source: ingress_api::stream_batch::FrameError,
    },
    /// A read reply for the sent attempt whose context or answer does not
    /// match the request: its identity, record count or minimum.
    ReadReplyMismatch,
    /// The read owner's state relations do not hold; nothing was changed.
    ReadStateInvariant,
    BatchReply {
        source: ReplyRequestError,
    },
    BatchRead {
        source: ingress_api::stream_batch::ReadError,
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
        cause: ClientError,
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
        cause: ClientError,
    },
    /// The event was not consumed. Time observation/expiry may still advance;
    /// rejection never claims the entire owner was rolled back.
    DriveRejected {
        event: ClientEvent,
        cause: ClientError,
    },
}

impl BatchClient {
    pub fn status(&self) -> ClientStatus {
        self.batch_status()
    }
}

/// A sans-IO owner served by `BatchDriverWorker`: the batch owner or the read
/// owner. The driver forwards observations and executes effects; the owner
/// alone decides connection, retry, deadline and terminal transitions.
pub trait DrivenClient: driven::Sealed {
    fn drive(&mut self, event: ClientEvent) -> ClientResult;
    /// Bind an established stream to the owner's current connection, or
    /// return it intact.
    fn bind_stream<S: std::io::Read + std::io::Write>(
        &self,
        operation_id: u64,
        stream: S,
    ) -> Result<BatchIoWorker<S>, (S, ClientError)>;
    /// The wait until the owner's next deadline, projected from its last turn.
    fn wait_timeout(&self) -> Result<Option<std::time::Duration>, ClientError>;
}

pub(crate) fn partial_reply_timeout(operation_id: u64, partial: Option<(u64, IncompleteFrame)>) -> Option<ClientResult> {
    let (_, cursor) = partial.filter(|(id, _)| *id == operation_id)?;
    let cause = ClientError::BatchRead { source: ingress_api::stream_batch::ReadError::FrameRead {
        frame: cursor.into(), source: std::io::ErrorKind::TimedOut.into(),
    } };
    Some(ClientResult::ConnectionFault { operation_id, cause })
}

mod driven {
    pub trait Sealed {}
    impl Sealed for super::BatchClient {}
    impl Sealed for crate::IngressReadOwner {}
}

impl DrivenClient for BatchClient {
    fn drive(&mut self, event: ClientEvent) -> ClientResult {
        BatchClient::drive(self, event)
    }

    fn bind_stream<S: std::io::Read + std::io::Write>(
        &self,
        operation_id: u64,
        stream: S,
    ) -> Result<BatchIoWorker<S>, (S, ClientError)> {
        self.bind_batch_stream(operation_id, stream)
    }

    fn wait_timeout(&self) -> Result<Option<std::time::Duration>, ClientError> {
        self.batch_wait_timeout()
    }
}

// Same domain root: no additional owner, adapter-held slots or completion queue.
include!("batch_client.rs");
// Stateless id and endpoint checks the read owner shares; no state or policy.
pub(crate) use adapter::{next_batch_connection_id, next_batch_request_id, validate_endpoints};
