// Direct client reads: one owner, one read at a time from acceptance until its
// terminal is consumed. Reads are best effort at the serving node's applied
// entry; nothing here grants commitment, linearizability or a write. The
// owner shares the batch client's events, effects and physical ports, not its
// state.
use crate::client::{
    BATCH_CLIENT_LIFETIME_MAX_MS, BatchIoWorker, ClientError, ClientEvent, ClientResult, ClockReadError, DrivenClient,
    MonotonicMillis, next_batch_connection_id, next_batch_request_id, partial_reply_timeout,
};
pub use ingress_api::stream_batch::{IngressRefusal, ReadAnswer, ReadRecord};
use ingress_api::stream_batch::{IncompleteFrame, ReadOutcome, ReadReply, ReadRequest};
pub use statevec_frame::read_protocol::{CLIENT_READ_LIMITS, ReadKey, ReadObservation, ReadRefusal, ReadRole};
use statevec_frame::stream_batch_protocol::RequestContext;
use std::net::SocketAddr;
use std::num::{NonZeroU64, NonZeroU128};

/// Round waits double up to the larger of the retry interval and this.
const READ_ROUND_BACKOFF_CEILING_MS: u64 = 5_000;

/// Whether the owner raises each read's minimum to what this session saw.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadSession {
    /// Each read carries only its own minimum.
    Independent,
    /// Each read's minimum is at least the session floor when it is accepted:
    /// the highest `applied_tx_seq` of an answer, or of a write reported
    /// through `observe_write`.
    Floor,
}

/// The caller's read: keys in order and an optional least `applied_tx_seq`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadQuery {
    pub keys: Vec<ReadKey>,
    pub min_applied_tx_seq: Option<u64>,
}

/// Correlation for one accepted read. It holds no custody.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ReadHandle(NonZeroU64);

impl ReadHandle {
    pub fn get(self) -> NonZeroU64 {
        self.0
    }
}

/// A read refused before acceptance, returned whole.
#[derive(Debug)]
#[must_use = "local refusal returns the complete original read"]
#[non_exhaustive]
pub struct ReadRefusedResult {
    pub reason: ClientError,
    pub query: ReadQuery,
}
pub use ReadRefusedResult as ReadRefused;

/// The one outcome of an accepted read, held until `poll_event` consumes it.
#[derive(Debug)]
#[must_use = "a read's outcome is consumed once"]
pub enum ReadEventResult {
    /// Every record at P: the observation's applied entry and the answer's
    /// frontier, one optional record per key in request order.
    #[non_exhaustive]
    Answered { handle: ReadHandle, observation: ReadObservation, answer: ReadAnswer },
    /// A refusal no other endpoint changes, or `ReadsDisabled` from every
    /// endpoint of one round.
    #[non_exhaustive]
    Refused { handle: ReadHandle, refusal: ReadRefusal, observation: Option<ReadObservation> },
    /// The node refused this request before decoding a protocol it does not support.
    #[non_exhaustive]
    IngressProtocolUnsupported { handle: ReadHandle, refusal: IngressRefusal },
    /// The node understood the protocol and identified the unsupported request by ID.
    #[non_exhaustive]
    IngressKindUnsupported { handle: ReadHandle, refusal: IngressRefusal },
    /// The read's deadline passed; the last refusal seen, if any.
    #[non_exhaustive]
    DeadlineExceeded { handle: ReadHandle, last_refusal: Option<ReadRefusal> },
    /// The session stopped first: an explicit stop or an unusable clock.
    #[non_exhaustive]
    Stopped { handle: ReadHandle, cause: ClientError },
}
pub use ReadEventResult as ReadEvent;

impl ReadEventResult {
    pub fn handle(&self) -> ReadHandle {
        match self {
            Self::Answered { handle, .. }
            | Self::Refused { handle, .. }
            | Self::IngressProtocolUnsupported { handle, .. }
            | Self::IngressKindUnsupported { handle, .. }
            | Self::DeadlineExceeded { handle, .. }
            | Self::Stopped { handle, .. } => *handle,
        }
    }
}

/// What `observe_write` did to the session floor.
#[derive(Debug, PartialEq, Eq)]
#[must_use = "the session floor after the write"]
pub enum ReadFloorResult {
    /// The floor rose to the write's `tx_seq`.
    Raised { floor: u64 },
    /// The floor was already at or above the write's `tx_seq`.
    Unchanged { floor: u64 },
    /// An independent session keeps no floor.
    Independent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct ReadStatus {
    /// An accepted read is held, waiting or with an unconsumed outcome.
    pub occupied: bool,
    pub connected: bool,
    /// No more reads are accepted; a Close may still be outstanding.
    pub stopped: bool,
    /// The session floor, when the session keeps one.
    pub floor: Option<u64>,
    pub next_deadline: Option<MonotonicMillis>,
}

#[derive(Debug)]
enum ReadConnection {
    /// No socket. The next connect, to `peer`, waits for a read and for
    /// `retry_at`.
    Disconnected {
        retry_at: MonotonicMillis,
        peer: usize,
    },
    Connecting {
        operation_id: u64,
        peer: usize,
        deadline: MonotonicMillis,
    },
    /// At most one frame write is outstanding, with its own bound: it
    /// outlives a read that ends first.
    Connected {
        operation_id: u64,
        peer: usize,
        write: Option<(NonZeroU64, MonotonicMillis)>,
        // An ID-less refusal might still describe an expired request on this socket.
        first_unanswered: Option<NonZeroU64>,
    },
    /// The socket awaits physical retirement; then the next connect, or None
    /// when the session stopped.
    CloseRequired {
        operation_id: u64,
        next: Option<(MonotonicMillis, usize)>,
    },
    Closing {
        operation_id: u64,
        next: Option<(MonotonicMillis, usize)>,
    },
    Stopped,
}

#[derive(Debug)]
enum ReadAttempt {
    /// Waits for its endpoint's connection, and for the round wait.
    Unsent { not_before: MonotonicMillis },
    /// Sent on this connection with this request id, answered by `reply_by`.
    Sent {
        operation_id: u64,
        request_id: NonZeroU64,
        wire_tags: (u16, u16),
        reply_by: MonotonicMillis,
    },
}

#[derive(Debug)]
struct PendingRead {
    handle: ReadHandle,
    // The original keys and the minimum fixed at acceptance: every attempt
    // sends exactly these.
    query: ReadQuery,
    deadline: MonotonicMillis,
    attempt: ReadAttempt,
    // Endpoints that refused or failed in the current round, and how many of
    // them refused ReadsDisabled.
    round_visits: usize,
    round_disabled: usize,
    round_wait_millis: u64,
    last_refusal: Option<ReadRefusal>,
}

#[derive(Debug)]
enum ReadSlot {
    Pending(Box<PendingRead>),
    Terminal(ReadEvent),
}

/// The session's changing facts. Their relations are checked by
/// `validate_read_state` before a transition acts on them.
#[derive(Debug)]
struct ReadState {
    connection: ReadConnection,
    slot: Option<ReadSlot>,
    floor: u64,
    // Connection-fault backoff; a decoded reply resets it.
    reconnect_millis: u64,
    last_now: MonotonicMillis,
    next_connection_id: u64,
    next_request_id: NonZeroU64,
    next_handle: NonZeroU64,
}

/// The read owner, beside `BatchClient`, with no write stream. It
/// selects endpoints cyclically, never by leader hint, and answers each
/// outcome by the refusal-handling table: next endpoint, return to the
/// caller, or the deadline. Memory-only and never cloneable.
#[must_use = "retain the owner and its read; dropping it loses the read"]
pub struct IngressReadOwner {
    cluster_identity: crate::Digest32,
    genesis_identity: crate::Digest32,
    execution_lineage: crate::Digest32,
    session_incarnation: NonZeroU128,
    endpoints: Vec<(u64, SocketAddr)>,
    maximum_frame_bytes: u32,
    retry_millis: u64,
    operation_millis: u64,
    session: ReadSession,
    clock: Box<dyn FnMut() -> Result<MonotonicMillis, ClockReadError> + Send>,
    state: ReadState,
}

impl std::fmt::Debug for IngressReadOwner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IngressReadOwner")
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}

impl IngressReadOwner {
    /// A memory-only, disconnected read session. Nothing connects until a
    /// read is accepted. `maximum_frame_bytes` should be the nodes' transport
    /// frame limit: a larger answer is refused `ReplyTooLarge` by the node,
    /// while a frame above this cap fails as a connection fault. Durations
    /// are process-local milliseconds within the batch client bounds; values
    /// are rejected, never clamped.
    pub fn bounded(
        settings: crate::ReadClientSettings,
        mut clock: impl FnMut() -> Result<MonotonicMillis, ClockReadError> + Send + 'static,
    ) -> Result<Self, ClientError> {
        let crate::ReadClientSettings {
            identity, session_incarnation, endpoints, maximum_frame_bytes, retry_millis, operation_millis, session,
        } = settings;
        adapter::validate_read_construction(identity, &endpoints, maximum_frame_bytes, retry_millis, operation_millis)?;
        let now = clock().map_err(|source| ClientError::InvalidTime { source })?;
        let owner = Self {
            cluster_identity: identity.cluster_identity(),
            genesis_identity: identity.genesis_identity(),
            execution_lineage: identity.execution_lineage(),
            session_incarnation,
            endpoints,
            maximum_frame_bytes,
            retry_millis,
            operation_millis,
            session,
            clock: Box::new(clock),
            state: ReadState {
                connection: ReadConnection::Disconnected { retry_at: now, peer: 0 },
                slot: None,
                floor: 0,
                reconnect_millis: retry_millis,
                last_now: now,
                next_connection_id: 1,
                next_request_id: NonZeroU64::MIN,
                next_handle: NonZeroU64::MIN,
            },
        };
        validate_read_state(&owner.state, owner.endpoints.len())?;
        Ok(owner)
    }

    /// Accept one read with its own deadline, `timeout_millis` from now. Its
    /// minimum is fixed here: the query's, raised to the session floor when
    /// the session keeps one. Refusal returns the query unchanged.
    pub fn try_read(&mut self, query: ReadQuery, timeout_millis: u64) -> Result<ReadHandle, ReadRefused> {
        let sample = (self.clock)();
        let now = match self.checked_read_time(sample) {
            Ok(now) => now,
            Err(reason) => return Err(ReadRefused { reason, query }),
        };
        self.expire_read(now);
        let accepted = (|| {
            validate_read_state(&self.state, self.endpoints.len())?;
            if self.stopped() {
                return Err(ClientError::Stopped);
            }
            if self.state.slot.is_some() {
                return Err(ClientError::Full);
            }
            if !(1..=BATCH_CLIENT_LIFETIME_MAX_MS).contains(&timeout_millis) {
                return Err(ClientError::InvalidLimit);
            }
            let request = ReadRequest {
                context: self.context(NonZeroU64::MIN),
                min_applied_tx_seq: query.min_applied_tx_seq,
                keys: query.keys.clone(),
            };
            ingress_api::stream_batch::encode_read_request(
                &request,
                self.maximum_frame_bytes as usize,
                CLIENT_READ_LIMITS,
            )
            .map_err(|source| ClientError::ReadFrame { source })?;
            let deadline = now
                .checked_add(timeout_millis)
                .map_err(|source| ClientError::InvalidTime { source })?;
            let next = next_batch_request_id(self.state.next_handle)?;
            Ok((deadline, next))
        })();
        let (deadline, next) = match accepted {
            Ok(values) => values,
            Err(reason) => return Err(ReadRefused { reason, query }),
        };
        let handle = ReadHandle(self.state.next_handle);
        self.state.next_handle = next;
        let minimum = match self.session {
            ReadSession::Independent => query.min_applied_tx_seq,
            ReadSession::Floor => query
                .min_applied_tx_seq
                .max(NonZeroU64::new(self.state.floor).map(NonZeroU64::get)),
        };
        self.state.slot = Some(ReadSlot::Pending(Box::new(PendingRead {
            handle,
            query: ReadQuery { keys: query.keys, min_applied_tx_seq: minimum },
            deadline,
            attempt: ReadAttempt::Unsent { not_before: now },
            round_visits: 0,
            round_disabled: 0,
            round_wait_millis: self.retry_millis,
            last_refusal: None,
        })));
        Ok(handle)
    }

    /// Raise the session floor to a write's `tx_seq`, such as a `Completed`
    /// batch's, so later reads include it. A read already accepted keeps its
    /// minimum.
    pub fn observe_write(&mut self, tx_seq: u64) -> ReadFloorResult {
        match self.session {
            ReadSession::Independent => ReadFloorResult::Independent,
            ReadSession::Floor if tx_seq > self.state.floor => {
                self.state.floor = tx_seq;
                ReadFloorResult::Raised { floor: tx_seq }
            }
            ReadSession::Floor => ReadFloorResult::Unchanged { floor: self.state.floor },
        }
    }

    /// Take the accepted read's outcome. This is the only release of the slot.
    pub fn poll_event(&mut self) -> Option<ReadEvent> {
        match self.state.slot.take() {
            Some(ReadSlot::Terminal(event)) => Some(event),
            pending => {
                self.state.slot = pending;
                None
            }
        }
    }

    pub fn status(&self) -> ReadStatus {
        ReadStatus {
            occupied: self.state.slot.is_some(),
            connected: matches!(self.state.connection, ReadConnection::Connected { .. }),
            stopped: self.stopped(),
            floor: (self.session == ReadSession::Floor).then_some(self.state.floor),
            next_deadline: self.read_deadline(),
        }
    }

    /// Bind an established nonblocking stream to the current connection; a
    /// mismatch returns the stream intact.
    #[doc(hidden)]
    pub fn bind_read_stream<S: std::io::Read + std::io::Write>(
        &self,
        operation_id: u64,
        stream: S,
    ) -> Result<BatchIoWorker<S>, (S, ClientError)> {
        if !matches!(self.state.connection, ReadConnection::Connected { operation_id: current, .. } if current == operation_id)
        {
            return Err((stream, ClientError::NotConnected));
        }
        let reader = match ingress_api::stream_batch::FrameReader::read_replies(self.maximum_frame_bytes as usize) {
            Ok(reader) => reader,
            Err(source) => return Err((stream, ClientError::BatchRead { source })),
        };
        Ok(BatchIoWorker::new(stream, operation_id, reader))
    }

    /// Every timed observation samples the bound clock; only Stop does not.
    #[doc(hidden)]
    pub fn drive(&mut self, event: ClientEvent) -> ClientResult {
        let sample = match &event {
            ClientEvent::Stop => Ok(self.state.last_now),
            _ => (self.clock)(),
        };
        let event = match event {
            ClientEvent::Stop => {
                self.stop_reads(ClientError::Stopped);
                return self.read_effect();
            }
            ClientEvent::WriteUnwritten { operation_id, request_id, source } => {
                ClientEvent::Written { operation_id, request_id, result: Err(source) }
            }
            ClientEvent::ClosedUnwritten { operation_id, .. } => ClientEvent::Closed { operation_id },
            event => event,
        };
        // Resource returns are consumed even with an unusable clock.
        match event {
            ClientEvent::Closed { operation_id } => return self.consume_read_close(operation_id, sample),
            ClientEvent::Connected { operation_id, result: Err(source) } => {
                return self.consume_failed_read_connect(operation_id, source, sample);
            }
            _ => (),
        }
        if let Err(cause) = validate_read_state(&self.state, self.endpoints.len()) {
            return ClientResult::DriveRejected { event, cause };
        }
        // An owed Close is sent whatever the clock: a failed sample stops the
        // reads and must not keep the socket. A closing connection's events
        // complete nothing.
        if matches!(self.state.connection, ReadConnection::CloseRequired { .. }) {
            if let Ok(now) = self.checked_read_time(sample) {
                self.expire_read(now);
            }
            return self.read_effect();
        }
        let now = match self.checked_read_time(sample) {
            Ok(now) => now,
            Err(cause) => return ClientResult::ClockFailed { cause },
        };
        self.expire_read(now);
        let partial = event.partial_reply();
        match event {
            ClientEvent::Drive | ClientEvent::PartialReply { written: None, .. } => (),
            ClientEvent::Connected { operation_id, result: Ok(()) } => {
                if let ReadConnection::Connecting { operation_id: pending, peer, .. } = self.state.connection
                    && pending == operation_id
                {
                    self.state.connection = ReadConnection::Connected { operation_id, peer, write: None, first_unanswered: None };
                    // The driver binds the stream after this turn; the first
                    // write belongs to the next one.
                    return self.read_effect();
                }
            }
            ClientEvent::Written { operation_id, request_id, result } => {
                if let Some(effect) = self.consume_read_write(operation_id, request_id, result, now) {
                    return effect;
                }
            }
            ClientEvent::PartialReply { operation_id, written: Some(request_id), .. } => {
                if let Some(effect) = self.consume_read_write(operation_id, request_id, Ok(()), now) {
                    return effect;
                }
            }
            event @ (ClientEvent::Read { operation_id, .. } | ClientEvent::VerifiedRead { operation_id, .. }) => {
                if let Some(effect) = self.consume_read_reply(operation_id, event, now) {
                    return effect;
                }
            }
            event => return ClientResult::DriveRejected { event, cause: ClientError::InvalidEvent },
        }
        if let Some(effect) = self.expire_read_exchange(now, partial) {
            return effect;
        }
        if matches!(self.state.connection, ReadConnection::CloseRequired { .. }) {
            return self.read_effect();
        }
        if let Some(effect) = self.advance_read(now) {
            return effect;
        }
        self.read_effect()
    }

    /// A new driver holds no socket: an owner whose connection is open, or
    /// still owes its retirement, cannot be handed to one.
    pub(crate) fn check_driver_assembly(&self) -> Result<(), ClientError> {
        match self.state.connection {
            ReadConnection::Disconnected { .. } | ReadConnection::Stopped => Ok(()),
            _ => Err(ClientError::BatchConnectionInvariant),
        }
    }

    fn stopped(&self) -> bool {
        matches!(
            self.state.connection,
            ReadConnection::Stopped
                | ReadConnection::CloseRequired { next: None, .. }
                | ReadConnection::Closing { next: None, .. }
        )
    }

    fn context(&self, request_id: NonZeroU64) -> RequestContext {
        RequestContext {
            cluster_identity: self.cluster_identity,
            genesis_identity: self.genesis_identity,
            execution_lineage: self.execution_lineage,
            session_incarnation: self.session_incarnation,
            request_id,
        }
    }

    fn pending(&mut self) -> Option<&mut PendingRead> {
        match &mut self.state.slot {
            Some(ReadSlot::Pending(read)) => Some(read),
            _ => None,
        }
    }

    fn checked_read_time(
        &mut self,
        sample: Result<MonotonicMillis, ClockReadError>,
    ) -> Result<MonotonicMillis, ClientError> {
        match sample {
            Ok(now) if now >= self.state.last_now => {
                self.state.last_now = now;
                Ok(now)
            }
            Ok(observed) => {
                let previous = self.state.last_now;
                self.stop_reads(ClientError::TimeRegressed { previous, observed });
                Err(ClientError::TimeRegressed { previous, observed })
            }
            Err(source) => {
                self.stop_reads(ClientError::InvalidTime { source });
                Err(ClientError::InvalidTime { source })
            }
        }
    }

    /// The deadline ends the read with the last refusal seen. A sent attempt's
    /// connection stays: a late reply carries a retired request id. An
    /// unfinished write keeps its own bound on that connection.
    fn expire_read(&mut self, now: MonotonicMillis) {
        if let Some(read) = self.pending()
            && now >= read.deadline
        {
            let event = ReadEvent::DeadlineExceeded { handle: read.handle, last_refusal: read.last_refusal };
            self.state.slot = Some(ReadSlot::Terminal(event));
        }
    }

    fn stop_reads(&mut self, cause: ClientError) {
        if let Some(read) = self.pending() {
            let handle = read.handle;
            self.state.slot = Some(ReadSlot::Terminal(ReadEvent::Stopped { handle, cause }));
        }
        self.state.connection = match self.state.connection {
            ReadConnection::Disconnected { .. } | ReadConnection::Stopped => ReadConnection::Stopped,
            ReadConnection::Connecting { operation_id, .. }
            | ReadConnection::Connected { operation_id, .. }
            | ReadConnection::CloseRequired { operation_id, .. } => {
                ReadConnection::CloseRequired { operation_id, next: None }
            }
            ReadConnection::Closing { operation_id, .. } => ReadConnection::Closing { operation_id, next: None },
        };
    }

    fn current_peer(&self) -> Option<usize> {
        match self.state.connection {
            ReadConnection::Disconnected { peer, .. }
            | ReadConnection::Connecting { peer, .. }
            | ReadConnection::Connected { peer, .. } => Some(peer),
            _ => None,
        }
    }

    /// Count this endpoint's refusal or failure in the read's round. A full
    /// round returns its wait and starts the next round with a doubled wait.
    fn visit(&mut self, refusal: Option<ReadRefusal>) -> Option<u64> {
        let endpoints = self.endpoints.len();
        let ceiling = self.retry_millis.max(READ_ROUND_BACKOFF_CEILING_MS);
        let read = self.pending()?;
        if refusal.is_some() {
            read.last_refusal = refusal;
        }
        read.round_visits += 1;
        if refusal == Some(ReadRefusal::ReadsDisabled) {
            read.round_disabled += 1;
        }
        if read.round_visits < endpoints {
            return Some(0);
        }
        let wait = read.round_wait_millis;
        read.round_visits = 0;
        read.round_disabled = 0;
        read.round_wait_millis = wait.saturating_mul(2).min(ceiling);
        Some(wait)
    }

    /// Next endpoint after a refusal: immediate within a round, after the
    /// round wait at its end. The same endpoint keeps its connection.
    fn rotate(&mut self, refusal: ReadRefusal, observation: Option<ReadObservation>, now: MonotonicMillis) {
        let endpoints = self.endpoints.len();
        let Some(read) = self.pending() else { return };
        // This refusal completes a round in which every endpoint refused
        // ReadsDisabled.
        let full_round_disabled = refusal == ReadRefusal::ReadsDisabled && read.round_disabled + 1 == endpoints;
        let handle = read.handle;
        let Some(wait) = self.visit(Some(refusal)) else { return };
        if full_round_disabled {
            // No configured node serves client reads, which is the default.
            let event = ReadEvent::Refused { handle, refusal, observation };
            self.state.slot = Some(ReadSlot::Terminal(event));
            return;
        }
        let not_before = now.checked_add(wait).unwrap_or(MonotonicMillis::new(u64::MAX));
        let next = self.current_peer().map_or(0, |peer| (peer + 1) % self.endpoints.len());
        if let Some(read) = self.pending() {
            read.attempt = ReadAttempt::Unsent { not_before };
        }
        match self.state.connection {
            ReadConnection::Connected { peer, .. } if peer == next => (),
            ReadConnection::Connected { operation_id, .. } => {
                self.state.connection = ReadConnection::CloseRequired { operation_id, next: Some((not_before, next)) };
            }
            _ => (),
        }
    }

    /// A connection fault: retire the socket and move to the next endpoint
    /// after the reconnect backoff. A fault of the read's own connect or sent
    /// attempt is its visit, and waits the round wait when that is longer. An
    /// idle connection the read was waiting to use is no visit: the read keeps
    /// its own wait.
    fn read_connection_fault(&mut self, now: MonotonicMillis) {
        let attempted = match (&self.state.connection, &self.state.slot) {
            // Disconnected only after its failed connect.
            (ReadConnection::Connecting { .. } | ReadConnection::Disconnected { .. }, _) => true,
            (ReadConnection::Connected { operation_id, .. }, Some(ReadSlot::Pending(read))) => {
                matches!(read.attempt, ReadAttempt::Sent { operation_id: sent, .. } if sent == *operation_id)
            }
            _ => false,
        };
        let wait = if attempted { self.visit(None).unwrap_or(0) } else { 0 };
        let backoff = self.state.reconnect_millis;
        self.state.reconnect_millis = backoff
            .saturating_mul(2)
            .min(self.retry_millis.max(READ_ROUND_BACKOFF_CEILING_MS));
        let retry_at = now.checked_add(backoff.max(wait)).unwrap_or(MonotonicMillis::new(u64::MAX));
        let next = self.current_peer().map_or(0, |peer| (peer + 1) % self.endpoints.len());
        if attempted && let Some(read) = self.pending() {
            read.attempt = ReadAttempt::Unsent { not_before: retry_at };
        }
        self.state.connection = match self.state.connection {
            ReadConnection::Connecting { operation_id, .. } | ReadConnection::Connected { operation_id, .. } => {
                ReadConnection::CloseRequired { operation_id, next: Some((retry_at, next)) }
            }
            ReadConnection::Disconnected { .. } => ReadConnection::Disconnected { retry_at, peer: next },
            // Already retiring or stopped: the retirement decides what follows.
            _ => return,
        };
    }

    fn consume_read_close(
        &mut self,
        operation_id: u64,
        sample: Result<MonotonicMillis, ClockReadError>,
    ) -> ClientResult {
        let now = self.checked_read_time(sample);
        if let Ok(now) = now {
            self.expire_read(now);
        }
        if let ReadConnection::Closing { operation_id: pending, next } = self.state.connection
            && pending == operation_id
        {
            self.state.connection = match next {
                Some((retry_at, peer)) => ReadConnection::Disconnected { retry_at, peer },
                None => ReadConnection::Stopped,
            };
        }
        match now {
            Ok(_) => self.read_effect(),
            Err(cause) => ClientResult::ClockFailed { cause },
        }
    }

    fn consume_failed_read_connect(
        &mut self,
        operation_id: u64,
        source: std::io::Error,
        sample: Result<MonotonicMillis, ClockReadError>,
    ) -> ClientResult {
        let now = self.checked_read_time(sample);
        match self.state.connection {
            ReadConnection::Connecting { operation_id: pending, peer, .. } if pending == operation_id => {
                // Failure returns all physical custody: no Close is owed.
                self.state.connection = ReadConnection::Disconnected { retry_at: self.state.last_now, peer };
                if let Ok(now) = now {
                    self.expire_read(now);
                    self.read_connection_fault(now);
                }
            }
            ReadConnection::CloseRequired { operation_id: pending, next }
            | ReadConnection::Closing { operation_id: pending, next }
                if pending == operation_id =>
            {
                self.state.connection = match next {
                    Some((retry_at, peer)) => ReadConnection::Disconnected { retry_at, peer },
                    None => ReadConnection::Stopped,
                };
            }
            _ => (),
        }
        match now {
            Ok(_) => ClientResult::ConnectionFailed { operation_id, source, next_wake: self.read_deadline() },
            Err(_) => ClientResult::ConnectionFailed { operation_id, source, next_wake: None },
        }
    }

    fn consume_read_write(
        &mut self,
        operation_id: u64,
        request_id: NonZeroU64,
        result: std::io::Result<()>,
        now: MonotonicMillis,
    ) -> Option<ClientResult> {
        let ReadConnection::Connected { operation_id: current, write, .. } = &mut self.state.connection else {
            return None;
        };
        if *current != operation_id || !matches!(*write, Some((id, _)) if id == request_id) {
            return None;
        }
        *write = None;
        let source = result.err()?;
        self.read_connection_fault(now);
        Some(ClientResult::ConnectionFault { operation_id, cause: ClientError::BatchWrite { source } })
    }

    fn consume_read_reply(
        &mut self,
        operation_id: u64,
        event: ClientEvent,
        now: MonotonicMillis,
    ) -> Option<ClientResult> {
        let ReadConnection::Connected { operation_id: current, first_unanswered, .. } = self.state.connection else {
            return None;
        };
        if current != operation_id {
            return None; // A retired connection's frame completes nothing.
        }
        let maximum = self.maximum_frame_bytes as usize;
        let decoded = match event {
            ClientEvent::Read { result: Err(source), .. } => return self.read_reply_fault(operation_id, ClientError::BatchRead { source }, now),
            ClientEvent::Read { result: Ok(bytes), .. } => ingress_api::stream_batch::decode_read_reply(&bytes, maximum, CLIENT_READ_LIMITS),
            ClientEvent::VerifiedRead { frame, .. } => frame.decode_read_reply(maximum, CLIENT_READ_LIMITS),
            event => return Some(ClientResult::DriveRejected { event, cause: ClientError::InvalidEvent }),
        };
        let reply = match decoded {
            Ok(reply) => reply,
            Err(source) => {
                if let Some(refusal) = source.ingress_refusal() {
                    let Some(PendingRead {
                        handle, attempt: ReadAttempt::Sent { operation_id: sent, request_id, wire_tags, .. }, ..
                    }) = self.pending() else {
                        return None;
                    };
                    if *sent != operation_id || !crate::matches_refusal(*request_id, *wire_tags, refusal, first_unanswered) {
                        return None;
                    }
                    let handle = *handle;
                    let protocol = matches!(source, ingress_api::stream_batch::FrameError::IngressProtocolUnsupported { .. });
                    self.state.slot = Some(ReadSlot::Terminal(if protocol {
                        ReadEvent::IngressProtocolUnsupported { handle, refusal }
                    } else {
                        ReadEvent::IngressKindUnsupported { handle, refusal }
                    }));
                    self.confirm_read_exchange();
                    if protocol {
                        // The peer closes after a protocol refusal. Retire this socket before the next read.
                        self.read_connection_fault(now);
                    }
                    return None;
                }
                return self.read_reply_fault(operation_id, ClientError::ReadFrame { source }, now);
            }
        };
        let expected = match self.pending() {
            Some(PendingRead { attempt: ReadAttempt::Sent { operation_id: sent, request_id, .. }, .. })
                if *sent == operation_id && *request_id == reply.context.request_id =>
            {
                *request_id
            }
            // A reply to an earlier attempt or read never completes this one.
            _ => return None,
        };
        if reply.context != self.context(expected) || !self.answers_query(&reply) {
            return self.read_reply_fault(operation_id, ClientError::ReadReplyMismatch, now);
        }
        // Only the current attempt's reply shows the endpoint serving again,
        // as the batch owner resets on its current exchange.
        self.confirm_read_exchange();
        self.settle_reply(reply, now);
        None
    }

    fn confirm_read_exchange(&mut self) {
        self.state.reconnect_millis = self.retry_millis;
        if let ReadConnection::Connected { first_unanswered, .. } = &mut self.state.connection {
            // An answer to the latest request proves this socket processed the earlier frames.
            *first_unanswered = None;
        }
    }

    fn read_reply_fault(
        &mut self,
        operation_id: u64,
        cause: ClientError,
        now: MonotonicMillis,
    ) -> Option<ClientResult> {
        self.read_connection_fault(now);
        Some(ClientResult::ConnectionFault { operation_id, cause })
    }

    /// An answer holds the observation, one entry per key at or above the
    /// minimum, and each found record has its key's kind (and sys_id).
    fn answers_query(&mut self, reply: &ReadReply) -> bool {
        let Some(read) = self.pending() else { return false };
        let ReadOutcome::Answered(answer) = &reply.outcome else { return true };
        let found_matches = |(key, record): (&ReadKey, &Option<ReadRecord>)| match (key, record) {
            (_, None) => true,
            (ReadKey::SysId { kind, sys_id }, Some(record)) => record.kind == *kind && record.sys_id == *sys_id,
            (ReadKey::UniqueKey { kind, .. }, Some(record)) => record.kind == *kind,
        };
        reply.observation.is_some()
            && answer.records.len() == read.query.keys.len()
            && read
                .query
                .min_applied_tx_seq
                .is_none_or(|minimum| answer.applied_tx_seq >= minimum)
            && read.query.keys.iter().zip(&answer.records).all(found_matches)
    }

    /// The refusal-handling table.
    fn settle_reply(&mut self, reply: ReadReply, now: MonotonicMillis) {
        let Some(read) = self.pending() else { return };
        let handle = read.handle;
        let ReadReply { observation, outcome, .. } = reply;
        let refusal = match (outcome, observation) {
            (ReadOutcome::Answered(answer), Some(observation)) => {
                if self.session == ReadSession::Floor {
                    self.state.floor = self.state.floor.max(answer.applied_tx_seq);
                }
                self.state.slot = Some(ReadSlot::Terminal(ReadEvent::Answered { handle, observation, answer }));
                return;
            }
            // answers_query admits no answer without its observation.
            (ReadOutcome::Answered(_), None) => return,
            (ReadOutcome::Refused(refusal), _) => refusal,
        };
        match refusal {
            ReadRefusal::NotVoter
            | ReadRefusal::LeaderReadsDisabled
            | ReadRefusal::ReadsDisabled
            | ReadRefusal::RecoveryPending
            | ReadRefusal::SnapshotInstalling
            | ReadRefusal::NotYetVisible { .. }
            | ReadRefusal::Busy
            | ReadRefusal::Draining
            | ReadRefusal::Fatal
            | ReadRefusal::Unavailable => self.rotate(refusal, observation, now),
            // Rotation requires a recognized, endpoint-specific reason.
            _ => {
                self.state.slot = Some(ReadSlot::Terminal(ReadEvent::Refused { handle, refusal, observation }));
            }
        }
    }

    /// A connect, a write or a sent attempt past its own bound is a connection
    /// fault. A write's bound holds whether or not its read is still pending.
    fn expire_read_exchange(&mut self, now: MonotonicMillis, partial: Option<(u64, IncompleteFrame)>) -> Option<ClientResult> {
        let (operation_id, reply_due) = match (&self.state.connection, &self.state.slot) {
            (ReadConnection::Connected { operation_id, .. }, Some(ReadSlot::Pending(read)))
                if matches!(read.attempt, ReadAttempt::Sent { operation_id: sent, reply_by, .. }
                if sent == *operation_id && now >= reply_by) =>
            {
                (*operation_id, true)
            }
            (ReadConnection::Connecting { operation_id, deadline, .. }, _) if now >= *deadline => (*operation_id, false),
            (ReadConnection::Connected { operation_id, write: Some((_, write_by)), .. }, _) if now >= *write_by => (*operation_id, false),
            _ => return None,
        };
        self.read_connection_fault(now);
        if reply_due && let Some(diagnostic) = partial_reply_timeout(operation_id, partial) {
            return Some(diagnostic);
        }
        let cause = ClientError::BatchRead {
            source: ingress_api::stream_batch::ReadError::Io { source: std::io::ErrorKind::TimedOut.into() },
        };
        Some(ClientResult::ConnectionFault { operation_id, cause })
    }

    /// Connect to the read's endpoint, or send its next attempt.
    fn advance_read(&mut self, now: MonotonicMillis) -> Option<ClientResult> {
        let Some(PendingRead { attempt: ReadAttempt::Unsent { not_before }, .. }) = self.pending() else {
            return None;
        };
        if now < *not_before {
            return None;
        }
        match self.state.connection {
            ReadConnection::Disconnected { retry_at, peer } if now >= retry_at => {
                Some(self.begin_read_connect(peer, now))
            }
            ReadConnection::Connected { operation_id, write: None, .. } => Some(self.send_read(operation_id, now)),
            _ => None,
        }
    }

    fn begin_read_connect(&mut self, peer: usize, now: MonotonicMillis) -> ClientResult {
        let started = (|| {
            let deadline = now
                .checked_add(self.operation_millis)
                .map_err(|source| ClientError::InvalidTime { source })?;
            let next = next_batch_connection_id(self.state.next_connection_id)?;
            Ok((deadline, next))
        })();
        let (deadline, next) = match started {
            Ok(values) => values,
            Err(cause) => {
                self.stop_reads(cause);
                return self.read_effect();
            }
        };
        let operation_id = self.state.next_connection_id;
        self.state.next_connection_id = next;
        self.state.connection = ReadConnection::Connecting { operation_id, peer, deadline };
        let (peer_id, address) = self.endpoints[peer];
        ClientResult::Connect { operation_id, peer_id, address }
    }

    fn send_read(&mut self, operation_id: u64, now: MonotonicMillis) -> ClientResult {
        let request_id = self.state.next_request_id;
        let prepared = (|| {
            let next = next_batch_request_id(request_id)?;
            let reply_by = now
                .checked_add(self.operation_millis)
                .map_err(|source| ClientError::InvalidTime { source })?;
            let read = match &self.state.slot {
                Some(ReadSlot::Pending(read)) => read,
                _ => unreachable!("advance requires a pending read"),
            };
            let request = ReadRequest {
                context: self.context(request_id),
                min_applied_tx_seq: read.query.min_applied_tx_seq,
                keys: read.query.keys.clone(),
            };
            let bytes = ingress_api::stream_batch::encode_read_request(
                &request,
                self.maximum_frame_bytes as usize,
                CLIENT_READ_LIMITS,
            )
            .map_err(|source| ClientError::ReadFrame { source })?;
            let wire_tags = crate::encoded_wire_tags(&bytes).map_err(|source| ClientError::ReadFrame { source })?;
            Ok((next, reply_by, bytes, wire_tags))
        })();
        let (next, reply_by, bytes, wire_tags) = match prepared {
            Ok(values) => values,
            Err(cause) => {
                self.stop_reads(cause);
                return self.read_effect();
            }
        };
        self.state.next_request_id = next;
        if let Some(read) = self.pending() {
            read.attempt = ReadAttempt::Sent { operation_id, request_id, wire_tags, reply_by };
        }
        if let ReadConnection::Connected { write, first_unanswered, .. } = &mut self.state.connection {
            *write = Some((request_id, reply_by));
            first_unanswered.get_or_insert(request_id);
        }
        ClientResult::Write { operation_id, request_id, bytes }
    }

    fn read_effect(&mut self) -> ClientResult {
        if let ReadConnection::CloseRequired { operation_id, next } = self.state.connection {
            self.state.connection = ReadConnection::Closing { operation_id, next };
            return ClientResult::Close { operation_id };
        }
        ClientResult::Waiting { wake_at: self.read_deadline() }
    }

    /// The next time a turn can change anything: a due Close, the connect or
    /// write bound, the read's deadline, or its next step on the current
    /// connection.
    fn read_deadline(&self) -> Option<MonotonicMillis> {
        let connection = match self.state.connection {
            ReadConnection::CloseRequired { .. } => return Some(self.state.last_now),
            ReadConnection::Connecting { deadline, .. } => Some(deadline),
            ReadConnection::Connected { write: Some((_, write_by)), .. } => Some(write_by),
            _ => None,
        };
        let read = match &self.state.slot {
            Some(ReadSlot::Pending(read)) => {
                let step = match (&read.attempt, &self.state.connection) {
                    (ReadAttempt::Unsent { not_before }, ReadConnection::Disconnected { retry_at, .. }) => {
                        Some((*not_before).max(*retry_at))
                    }
                    (ReadAttempt::Unsent { not_before }, ReadConnection::Connected { write: None, .. }) => {
                        Some(*not_before)
                    }
                    (ReadAttempt::Sent { reply_by, .. }, ReadConnection::Connected { .. }) => Some(*reply_by),
                    // Connecting, Closing or an outstanding write: their own
                    // observation or bound is the next step.
                    _ => None,
                };
                Some(step.map_or(read.deadline, |step| step.min(read.deadline)))
            }
            _ => None,
        };
        connection.into_iter().chain(read).min()
    }
}

impl DrivenClient for IngressReadOwner {
    fn drive(&mut self, event: ClientEvent) -> ClientResult {
        IngressReadOwner::drive(self, event)
    }

    fn bind_stream<S: std::io::Read + std::io::Write>(
        &self,
        operation_id: u64,
        stream: S,
    ) -> Result<BatchIoWorker<S>, (S, ClientError)> {
        self.bind_read_stream(operation_id, stream)
    }

    fn wait_timeout(&self) -> Result<Option<std::time::Duration>, ClientError> {
        Ok(self
            .read_deadline()
            .map(|deadline| std::time::Duration::from_millis(deadline.get().saturating_sub(self.state.last_now.get()))))
    }
}

/// The read state's relations: issued connection and request ids, endpoint
/// indices in range, a sent attempt on the current connection, and no pending
/// read once the session stops.
fn validate_read_state(state: &ReadState, endpoints: usize) -> Result<(), ClientError> {
    let issued = |operation_id: u64| operation_id != 0 && operation_id < state.next_connection_id;
    let next_in_range = |next: &Option<(MonotonicMillis, usize)>| next.is_none_or(|(_, peer)| peer < endpoints);
    let connection = match &state.connection {
        ReadConnection::Disconnected { peer, .. } => *peer < endpoints,
        ReadConnection::Connecting { operation_id, peer, .. } => issued(*operation_id) && *peer < endpoints,
        ReadConnection::Connected { operation_id, peer, write, .. } => {
            issued(*operation_id) && *peer < endpoints && write.is_none_or(|(id, _)| id < state.next_request_id)
        }
        ReadConnection::CloseRequired { operation_id, next } | ReadConnection::Closing { operation_id, next } => {
            issued(*operation_id) && next_in_range(next)
        }
        ReadConnection::Stopped => true,
    };
    let stopped = matches!(
        state.connection,
        ReadConnection::Stopped
            | ReadConnection::CloseRequired { next: None, .. }
            | ReadConnection::Closing { next: None, .. }
    );
    let read = match &state.slot {
        Some(ReadSlot::Pending(read)) => {
            !stopped
                && read.handle.get() < state.next_handle
                && read.round_visits < endpoints
                && read.round_disabled <= read.round_visits
                && match read.attempt {
                    ReadAttempt::Unsent { .. } => true,
                    ReadAttempt::Sent { operation_id, request_id, .. } => {
                        request_id < state.next_request_id
                            && matches!(state.connection,
                                ReadConnection::Connected { operation_id: current, .. } if current == operation_id)
                    }
                }
        }
        Some(ReadSlot::Terminal(event)) => event.handle().get() < state.next_handle,
        None => true,
    };
    if connection && read { Ok(()) } else { Err(ClientError::ReadStateInvariant) }
}

#[cfg(test)]
#[path = "ut_read_client.rs"]
mod ut_read_client;
