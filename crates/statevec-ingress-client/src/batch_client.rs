// The bounded session is a variant of the existing owner, not a second client.
pub use IngressClientOwner as BatchClient;

pub const BATCH_CLIENT_MAX_ENDPOINTS: usize = 32;
pub const BATCH_CLIENT_MAX_UNAVAILABLE_REPLIES: u32 = 1024;
pub const BATCH_CLIENT_RETRY_MIN_MS: u64 = 1;
pub const BATCH_CLIENT_RETRY_MAX_MS: u64 = 60_000;
pub const BATCH_CLIENT_RETRY_DEFAULT_MS: u64 = 100;
pub const BATCH_CLIENT_OPERATION_MIN_MS: u64 = 1;
pub const BATCH_CLIENT_OPERATION_MAX_MS: u64 = 300_000;
pub const BATCH_CLIENT_OPERATION_DEFAULT_MS: u64 = 5_000;
pub const BATCH_CLIENT_LIFETIME_MIN_MS: u64 = 2;
pub const BATCH_CLIENT_LIFETIME_MAX_MS: u64 = statevec_frame::stream_batch_protocol::MAX_BATCH_LIFETIME_MILLIS;
pub const BATCH_CLIENT_LIFETIME_DEFAULT_MS: u64 = 120_000;

/// Shared correlation only, not a local admission or completion certificate.
/// Editing a returned copy cannot change the owner's slot or release custody.
pub use statevec_frame::stream_batch_protocol::ReplyBinding as BatchHandle;

#[derive(Debug)]
#[domain_result]
#[must_use = "local refusal returns the complete original batch"]
pub struct BatchRefusedResult {
    pub reason: ClientFailure,
    pub batch: ClientStreamBatch,
    /// Supplied import evidence is returned intact on local refusal.
    pub committed: Option<EntryBinding>,
}
pub use BatchRefusedResult as BatchRefused;

/// Notifications are coalesced in their accepted slot; terminals remain there
/// until consumed. Neither a progress notification nor a problem frees custody.
/// ```compile_fail,E0308
/// fn progress_cannot_settle(progress: statevec_ingress_client::BatchProgress) {
///     let _event = statevec_ingress_client::BatchEvent::Terminal(progress);
/// }
/// ```
/// ```compile_fail,E0308
/// fn terminal_cannot_be_coalesced(terminal: statevec_ingress_client::BatchTerminal) {
///     let _event = statevec_ingress_client::BatchEvent::Progress(terminal);
/// }
/// ```
#[derive(Debug)]
#[domain_result]
#[must_use = "retain the original intent and any known commitment"]
pub enum BatchEventResult {
    /// Consumption reports an observation; the original slot stays charged.
    Progress(BatchProgress),
    /// Consumption returns the latched result and releases exactly that slot.
    Terminal(BatchTerminal),
}
pub use BatchEventResult as BatchEvent;

impl BatchEventResult {
    pub fn handle(&self) -> BatchHandle {
        match self {
            Self::Progress(progress) => progress.handle(),
            Self::Terminal(terminal) => terminal.handle(),
        }
    }
}

/// Bounded nonterminal observations. No variant can settle or release a slot.
#[derive(Debug)]
#[domain_result]
#[must_use = "progress leaves the original batch in client custody"]
pub enum BatchProgressResult {
    IngressAdmitted {
        handle: BatchHandle,
        entry: EntryBinding,
    },
    CommittedAwaitingApply {
        handle: BatchHandle,
        entry: EntryBinding,
    },
    Problem {
        handle: BatchHandle,
        /// Shared diagnostic only; consuming it cannot change retry authority.
        cause: Arc<ClientFailure>,
    },
}
pub use BatchProgressResult as BatchProgress;

impl BatchProgressResult {
    pub fn handle(&self) -> BatchHandle {
        match self {
            Self::IngressAdmitted { handle, .. }
            | Self::CommittedAwaitingApply { handle, .. }
            | Self::Problem { handle, .. } => *handle,
        }
    }
}

/// Final local custody disposition. OutcomeUnknown does not prove that the
/// original command was unexecuted; it returns all retained recovery evidence.
/// ```compile_fail,E0599
/// fn duplicate(terminal: statevec_ingress_client::BatchTerminal) {
///     let _second = terminal.clone();
/// }
/// ```
#[derive(Debug)]
#[domain_result]
#[must_use = "retain terminal results, returned input and known commitment"]
pub enum BatchTerminalResult {
    Applied {
        handle: BatchHandle,
        entry: EntryBinding,
        results: Vec<MemberResult>,
        /// The first input/history problem survives exact settlement.
        problem: Option<Arc<ClientFailure>>,
    },
    ExactAppliedResultUnavailable {
        handle: BatchHandle,
        binding: AppliedStreamBatchBindingV1,
        /// The first input/history problem survives exact settlement.
        problem: Option<Arc<ClientFailure>>,
    },
    NotAdmitted {
        handle: BatchHandle,
        batch: ClientStreamBatch,
        cause: ClientFailure,
    },
    OutcomeUnknown {
        handle: BatchHandle,
        batch: ClientStreamBatch,
        committed: Option<EntryBinding>,
        cause: ClientFailure,
        /// The first input/history problem survives even after notification.
        problem: Option<Arc<ClientFailure>>,
    },
}
pub use BatchTerminalResult as BatchTerminal;

impl BatchTerminalResult {
    pub fn handle(&self) -> BatchHandle {
        match self {
            Self::Applied { handle, .. }
            | Self::ExactAppliedResultUnavailable { handle, .. }
            | Self::NotAdmitted { handle, .. }
            | Self::OutcomeUnknown { handle, .. } => *handle,
        }
    }
}

/// Scheduling or one query's correlation/lifetime, not a state per wire
/// disposition. The slot separately retains its dispatched Submit interval;
/// physical write custody belongs to the connection.
#[derive(Debug)]
enum BatchAttempt {
    // Eligibility time is scheduling only. First-send uncertainty is the
    // independent, saturating submit_transmissions fact on the original slot.
    Eligible {
        at: MonotonicMillis,
    },
    Awaiting {
        operation_id: u64,
        deadline: MonotonicMillis,
    },
    Querying {
        operation_id: u64,
        request_id: NonZeroU64,
        deadline: MonotonicMillis,
    },
    // Suspension stops scheduling, not an already accepted query or late exact
    // Submit completion. The physical exchange keeps its original deadline.
    SuspendedUnreported {
        query: Option<(u64, NonZeroU64, MonotonicMillis)>,
        problem: Arc<ClientFailure>,
    },
    Suspended {
        query: Option<(u64, NonZeroU64, MonotonicMillis)>,
        // Immutable diagnosis survives notification consumption and retirement.
        problem: Arc<ClientFailure>,
    },
}
impl BatchAttempt {
    fn query(&self) -> Option<(u64, NonZeroU64, MonotonicMillis)> {
        match self {
            Self::Querying { operation_id, request_id, deadline } => Some((*operation_id, *request_id, *deadline)),
            Self::SuspendedUnreported { query, .. } | Self::Suspended { query, .. } => *query,
            _ => None,
        }
    }
    fn is_suspended(&self) -> bool {
        matches!(self, Self::SuspendedUnreported { .. } | Self::Suspended { .. })
    }
}

#[derive(Debug)]
enum BatchSlot {
    Pending {
        handle: BatchHandle,
        batch: ClientStreamBatch,
        deadline: MonotonicMillis,
        attempt: BatchAttempt,
        // Current connection and inclusive first/last dispatched Submit IDs.
        // Single-slot dispatch makes these contiguous; after commitment only
        // queries allocate IDs, so they cannot enter this fixed-size interval.
        submit_route: Option<(u64, NonZeroU64, NonZeroU64)>,
        // Saturates at two deliberately: zero / exactly one / retransmitted.
        // Fresh wire IDs never erase uncertainty from a prior transmission/import.
        submit_transmissions: u8,
        // Only exact committed/apply replies establish this irreversible fact.
        committed: Option<EntryBinding>,
        // Canonical notification custody, never read back as retry authority.
        notice: Option<BatchProgress>,
    },
    Terminal {
        event: BatchTerminal,
    },
}
impl BatchSlot {
    fn handle(&self) -> BatchHandle {
        match self {
            Self::Pending { handle, .. } => *handle,
            Self::Terminal { event, .. } => event.handle(),
        }
    }
}

#[derive(Debug)]
enum BatchConnection {
    Disconnected {
        retry_at: MonotonicMillis,
        peer: usize,
    },
    Connecting {
        operation_id: u64,
        peer: usize,
        deadline: MonotonicMillis,
    },
    Connected {
        operation_id: u64,
        peer: usize,
        // At most one physical frame write, even if its slot already settled.
        write: Option<(NonZeroU64, MonotonicMillis)>,
    },
    // Routing is decided, but the socket still owns its accepted exchanges.
    // No new exchanges start here; original deadlines bound their drain.
    Switching {
        operation_id: u64,
        peer: usize,
        write: Option<(NonZeroU64, MonotonicMillis)>,
    },
    // None means session stop; Some is a failed/timed-out connection awaiting
    // physical retirement before its already checked retry deadline.
    // Untrusted time stops the session; no socket identity is reused.
    CloseRequired {
        operation_id: u64,
        retry: Option<(MonotonicMillis, usize)>,
    },
    Closing {
        operation_id: u64,
        retry: Option<(MonotonicMillis, usize)>,
    },
    Stopped,
}
impl BatchConnection {
    fn stopped(&self) -> bool {
        matches!(self, Self::Stopped | Self::CloseRequired { retry: None, .. } | Self::Closing { retry: None, .. })
    }
}

impl IngressClientOwner {
    pub(crate) fn check_driver_assembly(&self) -> Result<(), ClientFailure> {
        match &self.state {
            ClientState::Batch {
                connection: BatchConnection::Disconnected { .. } | BatchConnection::Stopped, ..
            } => Ok(()),
            ClientState::Batch { .. } => Err(ClientFailure::BatchConnectionInvariant),
        }
    }

    /// Read-only projection from the last owner turn. Mechanical drivers must
    /// deliver Drive immediately before using it to wait; this does not sample
    /// a clock or create another independently schedulable owner transition.
    pub(crate) fn batch_wait_timeout(&self) -> Result<Option<std::time::Duration>, ClientFailure> {
        let ClientState::Batch { last_now, .. } = &self.state;
        Ok(self
            .batch_deadline()
            .map(|deadline| std::time::Duration::from_millis(deadline.get().saturating_sub(last_now.get()))))
    }

    /// Bind an actually established nonblocking stream to its current operation
    /// and checked frame limits. This is resource assembly, not a transition or
    /// a replacement for delivering Connected; failure returns the intact stream.
    pub fn bind_batch_stream<S: std::io::Read + std::io::Write>(
        &self,
        operation_id: u64,
        stream: S,
    ) -> Result<BatchIoWorker<S>, (S, ClientFailure)> {
        let ClientState::Batch {
            connection: BatchConnection::Connected { operation_id: current, .. },
            maximum_frame_bytes,
            maximum_commands,
            ..
        } = &self.state
        else {
            return Err((stream, ClientFailure::NotConnected));
        };
        if *current != operation_id {
            return Err((stream, ClientFailure::NotConnected));
        }
        let reader = match ingress_api::stream_batch::FrameReader::new(
            ingress_api::stream_batch::StreamDirection::Replies,
            *maximum_frame_bytes as usize,
            *maximum_commands as usize,
        ) {
            Ok(reader) => reader,
            Err(source) => return Err((stream, ClientFailure::BatchRead { source })),
        };
        Ok(BatchIoWorker::new(stream, operation_id, reader))
    }

    /// Construct a memory-only, initially disconnected session. Starts are
    /// local reservations under exclusive stream use, not server certificates.
    /// Fresh submit requires a caller-known idle boundary; uncertain originals
    /// must enter through try_import. No file or socket is opened here.
    /// All duration arguments are process-local milliseconds. The common frame
    /// cap must fit requests and all replies up to maximum_commands, using the
    /// service's effective max_outcome_encoded_bytes as maximum_outcome_bytes.
    /// These are caller-provisioned limits, not negotiated ones. One slot holds
    /// the original or its unconsumed terminal. Original, encoded write, partial
    /// read and decoded reply may coexist, each bounded by the frame/count caps;
    /// this is not a one-frame total-memory promise. The constants define finite
    /// duration bounds/defaults; values are rejected, never clamped.
    #[domain_initial_owner_creation]
    pub fn bounded(
        identity: (Digest32, Digest32, Digest32),
        session_incarnation: NonZeroU128,
        stream: InputRef,
        endpoints: Vec<(u64, SocketAddr)>,
        maximum_commands: u32,
        maximum_frame_bytes: u32,
        maximum_outcome_bytes: u32,
        retry_millis: u64,
        operation_millis: u64,
        lifetime_millis: u64,
        unavailable_reply_limit: u32,
        mut clock: impl FnMut() -> Result<MonotonicMillis, ClockReadFailure> + Send + 'static,
    ) -> Result<Self, ClientFailure> {
        adapter::validate_batch_construction(
            identity,
            &stream,
            &endpoints,
            maximum_commands,
            maximum_frame_bytes,
            maximum_outcome_bytes,
            retry_millis,
            operation_millis,
            lifetime_millis,
            unavailable_reply_limit,
        )?;
        let now = clock().map_err(|source| ClientFailure::InvalidTime { source })?;
        now.checked_add(lifetime_millis)
            .map_err(|source| ClientFailure::InvalidTime { source })?;
        let client = Self {
            state: ClientState::Batch {
                context: RequestContext {
                    cluster_identity: identity.0,
                    genesis_identity: identity.1,
                    execution_profile: identity.2,
                    session_incarnation,
                    request_id: NonZeroU64::MIN,
                },
                stream,
                endpoints: endpoints.into_boxed_slice().into_vec(),
                maximum_commands,
                maximum_frame_bytes,
                retry_millis,
                operation_millis,
                lifetime_millis,
                unavailable_reply_limit,
                unavailable_replies: 0,
                clock: Box::new(clock),
                last_now: now,
                next_connection_id: 1,
                connection: BatchConnection::Disconnected { retry_at: now, peer: 0 },
                slot: None,
            },
        };
        validate_batch_state(&client.state, None, None)?;
        Ok(client)
    }

    /// The bound physical clock is sampled on this call, even after a long idle
    /// period. The non-yielding owner transition receives that exact observation.
    /// An unreadable/regressed clock or deadline overflow stops the session,
    /// even when this input is refused unchanged. Existing unresolved custody
    /// becomes OutcomeUnknown; latched terminals survive. Keep driving to retire
    /// the socket and consume the retained terminal with poll_event.
    #[domain_transition]
    #[statevec_domain_roles::semantic_contract_v1(owner = IngressClientOwner, requires = [OwnerPath])]
    pub fn try_submit(&mut self, batch: ClientStreamBatch) -> Result<BatchHandle, BatchRefused> {
        let ClientState::Batch { clock, .. } = &mut self.state;
        let now = clock();
        self.accept_batch(batch, None, 0, now)
    }

    /// Import externally reconstructed input, not a new command at an idle
    /// frontier. The configured position reserves its original start; the caller
    /// supplies the original cluster/profile and guarantees exclusive stream use.
    /// A new session cannot erase prior delivery uncertainty or commitment.
    /// Time failure has the same session-wide stop/retirement side effects as
    /// try_submit; refusal returns this original and its supplied commitment.
    #[domain_transition]
    #[statevec_domain_roles::semantic_contract_v1(owner = IngressClientOwner, requires = [OwnerPath])]
    pub fn try_import(
        &mut self,
        batch: ClientStreamBatch,
        committed: Option<EntryBinding>,
    ) -> Result<BatchHandle, BatchRefused> {
        let ClientState::Batch { clock, .. } = &mut self.state;
        let now = clock();
        self.accept_batch(batch, committed, 2, now)
    }

    fn accept_batch(
        &mut self,
        batch: ClientStreamBatch,
        committed: Option<EntryBinding>,
        submit_transmissions: u8,
        now: Result<MonotonicMillis, ClockReadFailure>,
    ) -> Result<BatchHandle, BatchRefused> {
        if let Err(reason) = validate_batch_state(&self.state, None, None) {
            return Err(BatchRefused { reason, batch, committed });
        }
        let now = match self.checked_batch_time(now) {
            Ok(now) => now,
            Err(reason) => return Err(BatchRefused { reason, batch, committed }),
        };
        let ClientState::Batch { lifetime_millis, .. } = &mut self.state;
        if let Err(source) = now.checked_add(*lifetime_millis) {
            self.stop_batches(ClientFailure::InvalidTime { source });
            return Err(BatchRefused { reason: ClientFailure::InvalidTime { source }, batch, committed });
        }
        self.expire_batches(now);
        let ClientState::Batch {
            context,
            stream,
            maximum_commands,
            maximum_frame_bytes,
            lifetime_millis,
            connection,
            slot,
            ..
        } = &self.state;
        let accepted = (|| {
            if connection.stopped() {
                return Err(ClientFailure::Stopped);
            }
            if !matches!(connection, BatchConnection::Connected { .. } | BatchConnection::Switching { .. }) {
                return Err(ClientFailure::NotConnected);
            }
            let first = batch.first_input();
            if (stream.client_id, stream.stream_id) != (first.client_id, first.stream_id) {
                return Err(ClientFailure::InvalidInput);
            }
            if slot.is_some() {
                return Err(ClientFailure::Full);
            }
            if stream.client_seq == u64::MAX {
                return Err(ClientFailure::SequenceExhausted);
            }
            if stream.client_seq != first.client_seq {
                return Err(ClientFailure::BatchSequence { expected: stream.client_seq, observed: first.client_seq });
            }
            adapter::batch_frame_bytes(&batch, *maximum_commands, *maximum_frame_bytes)?;
            let next_id = adapter::next_batch_request_id(context.request_id)?;
            let deadline = now
                .checked_add(*lifetime_millis)
                .map_err(|source| ClientFailure::InvalidTime { source })?;
            Ok((next_id, deadline))
        })();
        let (next_id, deadline) = match accepted {
            Ok(values) => values,
            Err(reason) => return Err(BatchRefused { reason, batch, committed }),
        };
        let handle = BatchHandle { context: *context, intent: batch.commitment_v1() };
        let candidate = BatchSlot::Pending {
            handle,
            batch,
            deadline,
            attempt: BatchAttempt::Eligible { at: now },
            submit_route: None,
            // Two denotes prior uncertainty, not an exact transmission count.
            // Import cannot acquire the fresh request's NotAdmitted proof.
            submit_transmissions,
            committed,
            notice: None,
        };
        self.reserve_batch_slot(candidate, next_id).map_err(|(reason, candidate)| {
            let BatchSlot::Pending { batch, committed, .. } = candidate else { unreachable!("admission candidate") };
            BatchRefused { reason, batch, committed }
        })
    }

    fn reserve_batch_slot(
        &mut self,
        candidate: BatchSlot,
        next_id: NonZeroU64,
    ) -> Result<BatchHandle, (ClientFailure, BatchSlot)> {
        // Validate borrowed candidate facts before installing exclusive custody.
        // No owner clone, second pending map, or repeated payload hashing.
        if let Err(reason) = validate_batch_state(&self.state, Some((&candidate, next_id)), None) {
            return Err((reason, candidate));
        }
        let handle = candidate.handle();
        let ClientState::Batch { slot, context, .. } = &mut self.state;
        *slot = Some(candidate);
        context.request_id = next_id;
        Ok(handle)
    }

    /// Consuming a latched event is the only capacity-release operation. There
    /// is no completion queue and no callback/future that can free this slot.
    #[domain_transition]
    #[statevec_domain_roles::semantic_contract_v1(owner = IngressClientOwner, requires = [OwnerPath])]
    pub fn poll_event(&mut self) -> Option<BatchEvent> {
        self.consume_batch_event()
    }

    fn consume_batch_event(&mut self) -> Option<BatchEvent> {
        let ClientState::Batch { slot, stream, .. } = &mut self.state;
        if !matches!(slot, Some(BatchSlot::Terminal { .. })) {
            return slot.as_mut().and_then(|slot| match slot {
                BatchSlot::Pending { handle, attempt, notice, .. } => {
                    if let BatchAttempt::SuspendedUnreported { query, problem } = attempt {
                        let cause = Arc::clone(problem);
                        *attempt = BatchAttempt::Suspended { query: *query, problem: Arc::clone(problem) };
                        Some(BatchEvent::Progress(BatchProgress::Problem { handle: *handle, cause }))
                    } else {
                        notice.take().map(BatchEvent::Progress)
                    }
                }
                BatchSlot::Terminal { .. } => None,
            });
        };
        let Some(BatchSlot::Terminal { event, .. }) = slot.take() else { unreachable!("terminal slot") };
        if matches!(event, BatchTerminal::Applied { .. } | BatchTerminal::ExactAppliedResultUnavailable { .. }) {
            let intent = event.handle().intent;
            stream.client_seq = intent.next_sequence();
        }
        // Removing a sealed terminal cannot add a reservation or change its
        // retained handle's binding. Draining must
        // remain possible even if a later admission detects an invariant fault.
        Some(BatchEvent::Terminal(event))
    }

    /// Bounded owner turns, including first Submit and exact reply consumption.
    /// PoC uncertainty retries the same intent with a fresh exchange identity;
    /// confirmed commitment permits only read-only Reconcile. Physical driver
    /// assembly remains separate; routing never changes intent or known facts.
    #[domain_transition]
    #[statevec_domain_roles::semantic_contract_v1(owner = IngressClientOwner, requires = [OwnerPath, ProductionBoundDst, PhysicalIt])]
    pub fn drive(&mut self, event: ClientEvent) -> ClientResult {
        let ClientState::Batch { clock, last_now, .. } = &mut self.state;
        // All timed entries sample the same bound source. The private semantic
        // transitions receive that observation and never read an ambient clock.
        let now = match &event {
            ClientEvent::Drive
            | ClientEvent::Connected { .. }
            | ClientEvent::Closed { .. }
            | ClientEvent::Read { .. }
            | ClientEvent::Written { .. } => clock(),
            _ => Ok(*last_now),
        };
        self.drive_batch(event, now)
    }

    #[domain_transition]
    fn drive_batch(&mut self, event: ClientEvent, sample: Result<MonotonicMillis, ClockReadFailure>) -> ClientResult {
        if matches!(event, ClientEvent::Stop) {
            self.stop_batches(ClientFailure::Stopped);
            return self.batch_effect();
        }
        let event = match event {
            ClientEvent::Read { operation_id, result } => return self.read_batch_reply(operation_id, result, sample),
            ClientEvent::Written { operation_id, request_id, result } => {
                return self.consume_batch_write(operation_id, request_id, result, sample);
            }
            other => other,
        };
        // Resource-return observations cannot be rejected by the time guard.
        // An old operation still cannot retire the current connection.
        let event = match event {
            ClientEvent::Closed { operation_id } => return self.consume_batch_close(operation_id, sample),
            ClientEvent::Connected { operation_id, result: Err(source) } => {
                return self.consume_failed_connect(operation_id, source, sample);
            }
            event @ (ClientEvent::Drive | ClientEvent::Connected { result: Ok(()), .. }) => event,
            _ => return ClientResult::DriveRejected { event, cause: ClientFailure::InvalidEvent },
        };
        if matches!(self.state, ClientState::Batch { connection: BatchConnection::CloseRequired { .. }, .. }) {
            let _ = self.checked_batch_time(sample);
            return self.batch_effect();
        }
        if let Err(cause) = validate_batch_state(&self.state, None, None) {
            return ClientResult::DriveRejected { event, cause };
        }
        let now = match self.checked_batch_time(sample) {
            Ok(now) => now,
            Err(cause) => return ClientResult::ClockFailed { cause },
        };
        self.expire_batches(now);
        if self.expire_batch_exchange(now) {
            // Timeout consumed this clock sample even though Close exits early.
            return self.batch_effect();
        }
        if self.finish_batch_switch(now) {
            return self.batch_effect();
        }
        let ClientState::Batch { connection, .. } = &mut self.state;
        match event {
            ClientEvent::Drive => {
                if let BatchConnection::Disconnected { retry_at, peer } = connection {
                    if now >= *retry_at {
                        let peer = *peer;
                        return match self.begin_batch_connect(peer, now) {
                            Ok(effect) => effect,
                            Err(cause @ ClientFailure::InvalidTime { .. }) => ClientResult::ClockFailed { cause },
                            Err(cause) => ClientResult::DriveRejected { event: ClientEvent::Drive, cause },
                        };
                    }
                }
            }
            ClientEvent::Connected { operation_id, result: Ok(()) } => {
                if let BatchConnection::Connecting { operation_id: pending, peer, .. } = connection {
                    if operation_id == *pending {
                        *connection = BatchConnection::Connected { operation_id, peer: *peer, write: None };
                    }
                }
            }
            _ => unreachable!("batch observations checked above"),
        }
        if matches!(event, ClientEvent::Drive) {
            if let Some(effect) = self.begin_batch_exchange(now) {
                return effect;
            }
        }
        self.batch_effect()
    }

    fn begin_batch_connect(&mut self, peer: usize, now: MonotonicMillis) -> Result<ClientResult, ClientFailure> {
        let ClientState::Batch { next_connection_id, operation_millis, .. } = &self.state;
        let deadline = match now.checked_add(*operation_millis) {
            Ok(deadline) => deadline,
            Err(source) => {
                self.stop_batches(ClientFailure::InvalidTime { source });
                return Err(ClientFailure::InvalidTime { source });
            }
        };
        let next = adapter::next_batch_connection_id(*next_connection_id)?;
        let operation_id = *next_connection_id;
        let candidate = BatchConnection::Connecting { operation_id, peer, deadline };
        validate_batch_state(&self.state, None, Some((&candidate, next)))?;
        let ClientState::Batch { connection, next_connection_id, endpoints, unavailable_replies, .. } = &mut self.state;
        let (peer_id, address) = endpoints[peer];
        // A new owner-selected visit gets its own budget, even at the same
        // address. Connected observations and hints cannot replenish it.
        *unavailable_replies = 0;
        *connection = candidate;
        *next_connection_id = next;
        Ok(ClientResult::Connect { operation_id, peer_id, address })
    }

    fn consume_batch_close(
        &mut self,
        operation_id: u64,
        sample: Result<MonotonicMillis, ClockReadFailure>,
    ) -> ClientResult {
        let now = self.checked_batch_time(sample);
        if let Ok(now) = now {
            self.expire_batches(now);
        }
        let ClientState::Batch { connection, .. } = &mut self.state;
        if let BatchConnection::Closing { operation_id: pending, retry } = connection {
            if operation_id == *pending {
                *connection = match *retry {
                    Some((retry_at, peer)) => BatchConnection::Disconnected { retry_at, peer },
                    None => BatchConnection::Stopped,
                };
            }
        }
        match now {
            Ok(_) => self.batch_effect(),
            Err(cause) => ClientResult::ClockFailed { cause },
        }
    }

    fn consume_failed_connect(
        &mut self,
        operation_id: u64,
        source: std::io::Error,
        sample: Result<MonotonicMillis, ClockReadFailure>,
    ) -> ClientResult {
        let now = self.checked_batch_time(sample);
        if let Ok(now) = now {
            self.expire_batches(now);
        }
        let next_peer = self.next_batch_peer();
        let ClientState::Batch { connection, retry_millis, .. } = &mut self.state;
        // Failure returns physical custody, including after Stop/timeout. A bad
        // clock has already made retry None, but cannot retain that resource.
        let retry_peer = match connection {
            BatchConnection::Connecting { operation_id: pending, .. } if *pending == operation_id => Some(next_peer),
            BatchConnection::CloseRequired { operation_id: pending, retry }
            | BatchConnection::Closing { operation_id: pending, retry }
                if *pending == operation_id =>
            {
                retry.map(|(_, peer)| peer)
            }
            _ => {
                return match now {
                    Ok(_) => self.batch_effect(),
                    Err(cause) => ClientResult::ClockFailed { cause },
                };
            }
        };
        *connection = BatchConnection::Stopped;
        let now = match now {
            Ok(now) => now,
            // The terminal already retains the typed time failure. This
            // consumed physical failure still reports its original I/O cause.
            Err(_) => return ClientResult::ConnectionFailed { operation_id, source, next_wake: None },
        };
        if let Some(peer) = retry_peer {
            let retry_at = match now.checked_add(*retry_millis) {
                Ok(due) => due,
                Err(time_failure) => {
                    self.stop_batches(ClientFailure::InvalidTime { source: time_failure });
                    return ClientResult::ConnectionFailed { operation_id, source, next_wake: None };
                }
            };
            *connection = BatchConnection::Disconnected { retry_at, peer };
        }
        ClientResult::ConnectionFailed { operation_id, source, next_wake: self.batch_deadline() }
    }

    fn checked_batch_time(
        &mut self,
        sample: Result<MonotonicMillis, ClockReadFailure>,
    ) -> Result<MonotonicMillis, ClientFailure> {
        let ClientState::Batch { last_now, .. } = &mut self.state;
        match sample {
            Ok(now) if now >= *last_now => {
                *last_now = now;
                Ok(now)
            }
            Ok(observed) => {
                let previous = *last_now;
                self.stop_batches(ClientFailure::TimeRegressed { previous, observed });
                Err(ClientFailure::TimeRegressed { previous, observed })
            }
            Err(source) => {
                self.stop_batches(ClientFailure::InvalidTime { source });
                Err(ClientFailure::InvalidTime { source })
            }
        }
    }

    fn expire_batches(&mut self, now: MonotonicMillis) {
        let ClientState::Batch { slot, .. } = &self.state;
        if slot
            .iter()
            .any(|slot| matches!(slot, BatchSlot::Pending { deadline, .. } if now >= *deadline))
        {
            self.stop_batches(ClientFailure::RequestLifetimeExpired);
        }
    }

    fn stop_batches(&mut self, cause: ClientFailure) {
        let ClientState::Batch { connection, slot, .. } = &mut self.state;
        if matches!(slot, Some(BatchSlot::Pending { .. })) {
            let Some(BatchSlot::Pending { handle, batch, committed, attempt, .. }) = slot.take() else {
                unreachable!("pending")
            };
            *slot = Some(BatchSlot::Terminal {
                event: BatchTerminal::OutcomeUnknown {
                    handle,
                    batch,
                    committed,
                    problem: match attempt {
                        BatchAttempt::SuspendedUnreported { problem, .. } => Some(problem),
                        BatchAttempt::Suspended { problem, .. } => Some(problem),
                        _ => None,
                    },
                    cause,
                },
            });
        }
        *connection = match *connection {
            BatchConnection::Disconnected { .. } | BatchConnection::Stopped => BatchConnection::Stopped,
            BatchConnection::Connecting { operation_id, .. }
            | BatchConnection::Connected { operation_id, .. }
            | BatchConnection::Switching { operation_id, .. }
            | BatchConnection::CloseRequired { operation_id, .. } => {
                BatchConnection::CloseRequired { operation_id, retry: None }
            }
            BatchConnection::Closing { operation_id, .. } => BatchConnection::Closing { operation_id, retry: None },
        };
    }

    fn batch_effect(&mut self) -> ClientResult {
        let ClientState::Batch { connection, .. } = &mut self.state;
        if let BatchConnection::CloseRequired { operation_id, retry } = *connection {
            *connection = BatchConnection::Closing { operation_id, retry };
            return ClientResult::Close { operation_id };
        }
        ClientResult::Waiting { wake_at: self.batch_deadline() }
    }

    fn batch_deadline(&self) -> Option<MonotonicMillis> {
        let ClientState::Batch { connection, slot, last_now, .. } = &self.state;
        let connection_deadline = match connection {
            BatchConnection::Disconnected { retry_at, .. } => Some(*retry_at),
            BatchConnection::Connecting { deadline, .. } => Some(*deadline),
            BatchConnection::Connected { write: Some((_, deadline)), .. }
            | BatchConnection::Switching { write: Some((_, deadline)), .. } => Some(*deadline),
            BatchConnection::Switching { write: None, .. } if !batch_has_accepted_exchange(slot) => Some(*last_now),
            // try_submit can expire the session while returning the refused
            // input, so it cannot also return the Close effect. Keep that debt
            // runnable until drive actually hands it to the physical port.
            BatchConnection::CloseRequired { .. } => Some(*last_now),
            _ => None,
        };
        connection_deadline
            .into_iter()
            .chain(slot.iter().filter_map(|slot| match slot {
                BatchSlot::Pending { deadline, attempt, .. } => Some(match attempt {
                    BatchAttempt::Eligible { at }
                        if matches!(connection, BatchConnection::Connected { write: None, .. }) =>
                    {
                        (*deadline).min(*at)
                    }
                    BatchAttempt::Awaiting { deadline: exchange, .. }
                    | BatchAttempt::Querying { deadline: exchange, .. } => (*deadline).min(*exchange),
                    BatchAttempt::SuspendedUnreported { query: Some((_, _, exchange)), .. }
                    | BatchAttempt::Suspended { query: Some((_, _, exchange)), .. } => (*deadline).min(*exchange),
                    _ => *deadline,
                }),
                BatchSlot::Terminal { .. } => None,
            }))
            .min()
    }

    fn batch_status(&self) -> ClientStatus {
        let ClientState::Batch { connection, slot, .. } = &self.state;
        ClientStatus::Batch {
            occupied: usize::from(slot.is_some()),
            connected: matches!(connection, BatchConnection::Connected { .. } | BatchConnection::Switching { .. }),
            stopped: connection.stopped(),
            next_deadline: self.batch_deadline(),
        }
    }
}

// Overrides borrow an exact candidate,
// never a writable shadow state. Configuration and immutable intent/byte seals
// are checked once at construction/admission, not on every control turn.
#[domain_invariant("ingress_client")]
fn validate_batch_state(
    state: &ClientState,
    admission: Option<(&BatchSlot, NonZeroU64)>,
    connecting: Option<(&BatchConnection, u64)>,
) -> Result<(), ClientFailure> {
    let ClientState::Batch {
        context,
        stream,
        endpoints,
        next_connection_id,
        connection,
        slot,
        unavailable_replies,
        unavailable_reply_limit,
        ..
    } = state;
    if let Some((candidate, next)) = admission {
        if !matches!(connection, BatchConnection::Connected { .. } | BatchConnection::Switching { .. })
            || slot.is_some()
            || !matches!(
                candidate,
                BatchSlot::Pending {
                    attempt: BatchAttempt::Eligible { .. },
                    submit_transmissions: 0,
                    committed: None,
                    notice: None,
                    submit_route: None,
                    ..
                } | BatchSlot::Pending {
                    attempt: BatchAttempt::Eligible { .. },
                    submit_transmissions: 2,
                    notice: None,
                    submit_route: None,
                    ..
                }
            )
            || context.request_id.get().checked_add(1) != Some(next.get())
            || candidate.handle().context.request_id != context.request_id
        {
            return Err(ClientFailure::BatchReservationInvariant);
        }
    }
    if *next_connection_id == 0 || *unavailable_replies > *unavailable_reply_limit {
        return Err(ClientFailure::BatchConnectionInvariant);
    }
    if let Some((candidate, next)) = connecting {
        if !matches!(connection, BatchConnection::Disconnected { .. })
            || next_connection_id.checked_add(1) != Some(next)
            || !matches!(candidate, BatchConnection::Connecting { operation_id, .. } if operation_id == next_connection_id)
        {
            return Err(ClientFailure::BatchConnectionInvariant);
        }
    }
    let (connection, next_connection_id) = connecting.unwrap_or((connection, *next_connection_id));
    let request_id = admission.map_or(context.request_id, |(_, next)| next);
    let slot = admission.map(|(candidate, _)| candidate).or(slot.as_ref());
    match connection {
        BatchConnection::Connecting { peer, .. }
        | BatchConnection::Connected { peer, .. }
        | BatchConnection::Switching { peer, .. }
        | BatchConnection::Disconnected { peer, .. }
            if *peer >= endpoints.len() =>
        {
            return Err(ClientFailure::BatchConnectionInvariant);
        }
        BatchConnection::CloseRequired { retry: Some((_, peer)), .. }
        | BatchConnection::Closing { retry: Some((_, peer)), .. }
            if *peer >= endpoints.len() =>
        {
            return Err(ClientFailure::BatchConnectionInvariant);
        }
        _ => {}
    }
    if let BatchConnection::Connected { write: Some((id, _)), .. }
    | BatchConnection::Switching { write: Some((id, _)), .. } = connection
    {
        if *id >= request_id {
            return Err(ClientFailure::BatchConnectionInvariant);
        }
    }
    match connection {
        BatchConnection::Connecting { operation_id, .. }
        | BatchConnection::Connected { operation_id, .. }
        | BatchConnection::Switching { operation_id, .. }
        | BatchConnection::CloseRequired { operation_id, .. }
        | BatchConnection::Closing { operation_id, .. }
            if *operation_id == 0 || *operation_id >= next_connection_id =>
        {
            return Err(ClientFailure::BatchConnectionInvariant);
        }
        _ => {}
    }
    if let Some(slot) = slot {
        let handle = slot.handle();
        if (stream.client_id, stream.stream_id) != (handle.intent.client_id(), handle.intent.stream_id())
            || stream.client_seq != handle.intent.first_sequence()
            || handle.context.cluster_identity != context.cluster_identity
            || handle.context.genesis_identity != context.genesis_identity
            || handle.context.execution_profile != context.execution_profile
            || handle.context.session_incarnation != context.session_incarnation
            || handle.context.request_id >= request_id
        {
            return Err(ClientFailure::BatchReservationInvariant);
        }
        if matches!(slot, BatchSlot::Pending { .. }) && connection.stopped() {
            return Err(ClientFailure::BatchReservationInvariant);
        }
        match slot {
            BatchSlot::Pending { attempt, committed, notice, submit_route, submit_transmissions, .. } => {
                if *submit_transmissions > 2
                    || (*submit_transmissions == 0
                        && (!matches!(attempt, BatchAttempt::Eligible { .. })
                            || committed.is_some()
                            || notice.is_some()
                            || submit_route.is_some()))
                    || (attempt.query().is_some() && committed.is_none())
                {
                    return Err(ClientFailure::BatchReservationInvariant);
                }
                if matches!(attempt, BatchAttempt::Awaiting { operation_id, .. } if submit_route.map(|(id, _, _)| id) != Some(*operation_id))
                {
                    return Err(ClientFailure::BatchConnectionInvariant);
                }
                for id in submit_route
                    .iter()
                    .map(|(id, _, _)| *id)
                    .chain(attempt.query().map(|(id, _, _)| id))
                {
                    if !matches!(connection, BatchConnection::Connected { operation_id, .. }
                        | BatchConnection::Switching { operation_id, .. } if id == *operation_id)
                    {
                        return Err(ClientFailure::BatchConnectionInvariant);
                    }
                }
                if let Some((_, first, last)) = submit_route {
                    if *first > *last
                        || *last >= request_id
                        || *first < handle.context.request_id
                        || (*submit_transmissions == 1 && (*first != handle.context.request_id || first != last))
                    {
                        return Err(ClientFailure::BatchReservationInvariant);
                    }
                }
                if let Some((_, id, _)) = attempt.query() {
                    if id >= request_id
                        || id <= handle.context.request_id
                        || submit_route.is_some_and(|(_, _, last)| id <= last)
                    {
                        return Err(ClientFailure::BatchReservationInvariant);
                    }
                }
                if notice.as_ref().is_some_and(|event| {
                    event.handle() != handle
                        || match event {
                            BatchProgress::CommittedAwaitingApply { entry, .. } => *committed != Some(*entry),
                            BatchProgress::IngressAdmitted { .. } => committed.is_some() || attempt.is_suspended(),
                            BatchProgress::Problem { .. } => true,
                        }
                }) {
                    return Err(ClientFailure::BatchReservationInvariant);
                }
            }
            // Only BatchTerminal can occupy this variant. Progress cannot
            // become a settled slot even before the invariant is checked.
            BatchSlot::Terminal { event: BatchTerminal::OutcomeUnknown { .. }, .. } if !connection.stopped() => {
                return Err(ClientFailure::BatchReservationInvariant);
            }
            BatchSlot::Terminal { .. } => {}
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "ut_batch_client.rs"]
mod ut_batch_client;

include!("batch_reply.rs");
