pub const BATCH_CLIENT_MAX_ENDPOINTS: usize = 32;
pub const BATCH_CLIENT_MAX_UNAVAILABLE_REPLIES: u32 = 1024;
pub const BATCH_CLIENT_RETRY_MIN_MS: u64 = 1;
pub const BATCH_CLIENT_RETRY_MAX_MS: u64 = 60_000;
pub const BATCH_CLIENT_RETRY_DEFAULT_MS: u64 = 100;
pub const BATCH_CLIENT_HANDOVER_RETRY_DEFAULT_MS: u64 = 5;
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
#[must_use = "local refusal returns the complete original batch"]
#[non_exhaustive]
pub struct BatchRefusedResult {
    pub reason: ClientError,
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
#[must_use = "progress leaves the original batch in client custody"]
pub enum BatchProgressResult {
    #[non_exhaustive]
    IngressAdmitted {
        handle: BatchHandle,
        entry: EntryBinding,
    },
    #[non_exhaustive]
    CommittedAwaitingApply {
        handle: BatchHandle,
        entry: EntryBinding,
    },
    #[non_exhaustive]
    Problem {
        handle: BatchHandle,
        /// Shared diagnostic only; consuming it cannot change retry authority.
        cause: Arc<ClientError>,
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
#[must_use = "retain terminal results, returned input and known commitment"]
pub enum BatchTerminalResult {
    #[non_exhaustive]
    Applied {
        handle: BatchHandle,
        entry: EntryBinding,
        results: Vec<MemberResult>,
        /// The first input/history problem survives exact settlement.
        problem: Option<Arc<ClientError>>,
    },
    #[non_exhaustive]
    ExactAppliedResultUnavailable {
        handle: BatchHandle,
        binding: AppliedStreamBatchBinding,
        /// The first input/history problem survives exact settlement.
        problem: Option<Arc<ClientError>>,
    },
    #[non_exhaustive]
    NotAdmitted {
        handle: BatchHandle,
        batch: ClientStreamBatch,
        cause: ClientError,
    },
    #[non_exhaustive]
    OutcomeUnknown {
        handle: BatchHandle,
        batch: ClientStreamBatch,
        committed: Option<EntryBinding>,
        cause: ClientError,
        /// The first input/history problem survives even after notification.
        problem: Option<Arc<ClientError>>,
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
    // independent delivery fact on the original slot.
    Eligible {
        at: MonotonicMillis,
    },
    Awaiting {
        operation_id: u64,
        request_id: NonZeroU64,
        wire_tags: (u16, u16),
        deadline: MonotonicMillis,
        probe_after: Option<MonotonicMillis>,
    },
    Querying {
        operation_id: u64,
        request_id: NonZeroU64,
        wire_tags: (u16, u16),
        deadline: MonotonicMillis,
        probe_after: Option<MonotonicMillis>,
    },
    // Suspension stops scheduling, not an already accepted query or late exact
    // Submit completion. The physical exchange keeps its original deadline.
    SuspendedUnreported {
        query: Option<(u64, NonZeroU64, MonotonicMillis, (u16, u16))>,
        problem: Arc<ClientError>,
    },
    Suspended {
        query: Option<(u64, NonZeroU64, MonotonicMillis, (u16, u16))>,
        // Immutable diagnosis survives notification consumption and retirement.
        problem: Arc<ClientError>,
    },
}
impl BatchAttempt {
    fn query(&self) -> Option<(u64, NonZeroU64, MonotonicMillis, (u16, u16))> {
        match self {
            Self::Querying { operation_id, request_id, deadline, wire_tags, .. } => {
                Some((*operation_id, *request_id, *deadline, *wire_tags))
            }
            Self::SuspendedUnreported { query, .. } | Self::Suspended { query, .. } => *query,
            _ => None,
        }
    }
    fn is_suspended(&self) -> bool {
        matches!(self, Self::SuspendedUnreported { .. } | Self::Suspended { .. })
    }
}

#[derive(Debug)]
enum SubmitDelivery {
    Unsent,
    Pending { operation_id: u64, request_id: NonZeroU64 },
    Sole { operation_id: u64, request_id: NonZeroU64 },
    // Multiple possibly exposed writes, or imported uncertainty. Never reset.
    Uncertain,
}

impl SubmitDelivery {
    fn for_admission(submit_transmissions: u8) -> Self {
        if submit_transmissions == 0 { Self::Unsent } else { Self::Uncertain }
    }

    fn dispatch(&mut self, operation_id: u64, request_id: NonZeroU64) {
        *self = match self {
            Self::Unsent => Self::Pending { operation_id, request_id },
            _ => Self::Uncertain,
        };
    }

    fn returned(&mut self, operation_id: u64, request_id: NonZeroU64) {
        if let Self::Pending { operation_id: held, request_id: request } = self {
            if *held == operation_id && *request == request_id {
                *self = Self::Sole { operation_id, request_id };
            }
        }
    }

    fn unwritten(&mut self, operation_id: u64, request_id: NonZeroU64) {
        if let Self::Pending { operation_id: held, request_id: request } = self {
            if *held == operation_id && *request == request_id {
                *self = Self::Unsent;
            }
        }
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
        admitted: Option<(u64, NonZeroU64, EntryBinding)>,
        // The term remains spent after the visit ends.
        last_handover_term: Option<NonZeroU64>,
        handover: Option<(usize, MonotonicMillis, BatchNextExchange)>,
        /// Visits left in the batch's one discovery episode.
        probe_budget: u8,
        // Dispatch/ID history, not delivery: zero / first / later or import.
        submit_transmissions: u8,
        delivery: SubmitDelivery,
        // Only exact committed/apply replies establish this irreversible fact.
        committed: Option<EntryBinding>,
        // A checked refusal or discovered successor leaves only a query
        // until the current leader makes the original intent retryable.
        compatibility_exchange: BatchNextExchange,
        // Canonical notification custody, never read back as retry authority.
        notice: Option<BatchProgress>,
    },
    Terminal {
        event: BatchTerminal,
    },
}

#[derive(Debug, PartialEq, Eq)]
enum BatchNextExchange {
    Reconcile,
    Submit,
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
        connected_at: MonotonicMillis,
        // At most one physical frame write, even if its slot already settled.
        write: Option<(NonZeroU64, MonotonicMillis)>,
        // An earlier Submit can settle the slot while a later exchange is unanswered.
        first_unanswered: Option<NonZeroU64>,
    },
    // Routing is decided, but the socket still owns its accepted exchanges.
    // No new exchanges start here; original deadlines bound their drain.
    Switching {
        operation_id: u64,
        peer: usize,
        write: Option<(NonZeroU64, MonotonicMillis)>,
        first_unanswered: Option<NonZeroU64>,
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

impl BatchClient {
    pub(crate) fn check_driver_assembly(&self) -> Result<(), ClientError> {
        match &self.state {
            ClientState::Batch {
                connection: BatchConnection::Disconnected { .. } | BatchConnection::Stopped,
                route_probe: RouteProbe::Idle,
                ..
            } => Ok(()),
            ClientState::Batch { .. } => Err(ClientError::BatchConnectionInvariant),
        }
    }

    /// Read-only projection from the last owner turn. Mechanical drivers must
    /// deliver Drive immediately before using it to wait; this does not sample
    /// a clock or create another independently schedulable owner transition.
    pub(crate) fn batch_wait_timeout(&self) -> Result<Option<std::time::Duration>, ClientError> {
        let ClientState::Batch { last_now, .. } = &self.state;
        Ok(self
            .batch_deadline()
            .map(|deadline| std::time::Duration::from_millis(deadline.get().saturating_sub(last_now.get()))))
    }

    /// Bind an actually established nonblocking stream to its current operation
    /// and checked frame limits. This is resource assembly, not a transition or
    /// a replacement for delivering Connected; failure returns the intact stream.
    #[doc(hidden)]
    pub fn bind_batch_stream<S: std::io::Read + std::io::Write>(
        &self,
        operation_id: u64,
        stream: S,
    ) -> Result<BatchIoWorker<S>, (S, ClientError)> {
        let ClientState::Batch {
            connection,
            route_probe,
            maximum_frame_bytes,
            ..
        } = &self.state;
        if !matches!(connection, BatchConnection::Connected { operation_id: current, .. } if *current == operation_id)
            && !matches!(route_probe, RouteProbe::Ready { operation_id: current, .. } if *current == operation_id)
        {
            return Err((stream, ClientError::NotConnected));
        }
        let reader = match ingress_api::stream_batch::FrameReader::batch_replies(
            *maximum_frame_bytes as usize,
            usize::from(statevec_frame::release::BATCH_COMMAND_CEILING),
        ) {
            Ok(reader) => reader,
            Err(source) => return Err((stream, ClientError::BatchRead { source })),
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
    /// A saved original can exceed this fresh send count when its request and
    /// maximum reply fit the frame and its count fits the release ceiling.
    /// These are caller-provisioned limits, not negotiated ones. One slot holds
    /// the original or its unconsumed terminal. Original, encoded write, partial
    /// read and decoded reply may coexist, each bounded by the frame/count caps;
    /// this is not a one-frame total-memory promise. The constants define finite
    /// duration bounds/defaults; values are rejected, never clamped.
    pub fn bounded(
        settings: crate::BatchClientSettings,
        mut clock: impl FnMut() -> Result<MonotonicMillis, ClockReadError> + Send + 'static,
    ) -> Result<Self, ClientError> {
        let crate::BatchClientSettings {
            identity, session_incarnation, stream, endpoints, maximum_commands, maximum_frame_bytes,
            maximum_outcome_bytes, retry, operation_millis, lifetime_millis, unavailable_reply_limit,
        } = settings;
        let crate::BatchRetryConfig { retry_millis, handover_retry_millis, raft_tick_millis, routing_poll_millis } = retry;
        let handover_window_millis = raft_tick_millis
            .checked_mul(crate::HANDOVER_WINDOW_TICKS)
            .ok_or(ClientError::InvalidLimit)?;
        if handover_retry_millis == 0 || handover_retry_millis > handover_window_millis {
            return Err(ClientError::InvalidLimit);
        }
        if !(crate::ROUTING_POLL_MIN_MS..=crate::ROUTING_POLL_MAX_MS).contains(&routing_poll_millis) {
            return Err(ClientError::InvalidLimit);
        }
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
        let now = clock().map_err(|source| ClientError::InvalidTime { source })?;
        now.checked_add(lifetime_millis)
            .map_err(|source| ClientError::InvalidTime { source })?;
        let client = Self {
            state: ClientState::Batch {
                context: RequestContext {
                    cluster_identity: identity.cluster_identity(),
                    genesis_identity: identity.genesis_identity(),
                    execution_lineage: identity.execution_lineage(),
                    session_incarnation,
                    request_id: NonZeroU64::MIN,
                },
                stream,
                previous_applied_index: 0,
                endpoints: endpoints.into_boxed_slice().into_vec(),
                maximum_commands,
                maximum_frame_bytes,
                maximum_outcome_bytes,
                retry_millis,
                handover_retry_millis,
                handover_window_millis,
                observed_leader: (0, None),
                routing_poll_millis,
                route_observation: None,
                route_visits: 0,
                route_successors: 0,
                route_probe: RouteProbe::Idle,
                idle_reconnect_millis: retry_millis,
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
        now: Result<MonotonicMillis, ClockReadError>,
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
            self.stop_batches(ClientError::InvalidTime { source });
            return Err(BatchRefused { reason: ClientError::InvalidTime { source }, batch, committed });
        }
        self.expire_batches(now);
        let ClientState::Batch {
            context,
            stream,
            maximum_commands,
            maximum_frame_bytes,
            maximum_outcome_bytes,
            lifetime_millis,
            connection,
            slot,
            ..
        } = &self.state;
        let accepted = (|| {
            if connection.stopped() {
                return Err(ClientError::Stopped);
            }
            if !matches!(connection, BatchConnection::Connected { .. } | BatchConnection::Switching { .. }) {
                return Err(ClientError::NotConnected);
            }
            let first = batch.first_input();
            if (stream.client_id, stream.stream_id) != (first.client_id, first.stream_id) {
                return Err(ClientError::InvalidInput);
            }
            if slot.is_some() {
                return Err(ClientError::Full);
            }
            if stream.client_seq == u64::MAX {
                return Err(ClientError::SequenceExhausted);
            }
            if stream.client_seq != first.client_seq {
                return Err(ClientError::BatchSequence { expected: stream.client_seq, observed: first.client_seq });
            }
            let commands = if submit_transmissions == 0 {
                *maximum_commands
            } else {
                u32::from(statevec_frame::release::BATCH_COMMAND_CEILING)
            };
            adapter::batch_frame_bytes(&batch, commands, *maximum_frame_bytes)?;
            if batch.commands().len() > *maximum_commands as usize {
                // Require room for the original's largest completed reply before taking custody.
                let reply = statevec_frame::stream_batch_protocol::completed_reply_byte_bound(
                    batch.commands().len() as u32, *maximum_outcome_bytes,
                ).ok_or(ClientError::InvalidLimit)?;
                if reply > u64::from(*maximum_frame_bytes) {
                    return Err(ClientError::TooLarge {
                        source: ClientStreamBatchError::EncodedBytesExceeded {
                            observed: usize::try_from(reply).unwrap_or(usize::MAX), limit: *maximum_frame_bytes as usize,
                        },
                    });
                }
            }
            let next_id = adapter::next_batch_request_id(context.request_id)?;
            let deadline = now
                .checked_add(*lifetime_millis)
                .map_err(|source| ClientError::InvalidTime { source })?;
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
            admitted: None,
            last_handover_term: None,
            handover: None,
            probe_budget: ROUTE_PROBE_LIMIT,
            // Two denotes prior uncertainty, not an exact transmission count.
            // Import cannot acquire the fresh request's NotAdmitted proof.
            submit_transmissions,
            delivery: SubmitDelivery::for_admission(submit_transmissions),
            committed,
            compatibility_exchange: BatchNextExchange::Submit,
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
    ) -> Result<BatchHandle, (ClientError, BatchSlot)> {
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
    pub fn poll_event(&mut self) -> Option<BatchEvent> {
        self.consume_batch_event()
    }

    fn consume_batch_event(&mut self) -> Option<BatchEvent> {
        let ClientState::Batch { slot, stream, previous_applied_index, .. } = &mut self.state;
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
        let applied = match &event {
            BatchTerminal::Applied { entry, .. } => Some(entry.index.get()),
            BatchTerminal::ExactAppliedResultUnavailable { binding, .. } => Some(binding.raft_index()),
            BatchTerminal::NotAdmitted { .. } | BatchTerminal::OutcomeUnknown { .. } => None,
        };
        if let Some(index) = applied {
            *previous_applied_index = index;
            stream.client_seq = event.handle().intent.next_sequence();
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
    #[doc(hidden)]
    pub fn drive(&mut self, event: ClientEvent) -> ClientResult {
        let ClientState::Batch { clock, last_now, .. } = &mut self.state;
        // All timed entries sample the same bound source. The private semantic
        // transitions receive that observation and never read an ambient clock.
        let now = match &event {
            ClientEvent::Drive
            | ClientEvent::Connected { .. }
            | ClientEvent::Closed { .. }
            | ClientEvent::ClosedUnwritten { .. }
            | ClientEvent::Read { .. }
            | ClientEvent::VerifiedRead { .. }
            | ClientEvent::PartialReply { .. }
            | ClientEvent::Written { .. }
            | ClientEvent::WriteUnwritten { .. } => clock(),
            _ => Ok(*last_now),
        };
        self.drive_batch(event, now)
    }

    /// Latest membership and leader observation obtained during recovery.
    pub fn route_observation(&self) -> Option<&crate::RouteObservation> {
        let ClientState::Batch { route_observation, .. } = &self.state;
        route_observation.as_ref()
    }

    /// Routing discovery visits and successor switches since construction.
    pub fn route_discovery(&self) -> crate::RouteDiscoveryCounts {
        let ClientState::Batch { route_visits, route_successors, .. } = &self.state;
        crate::RouteDiscoveryCounts { visits: *route_visits, successors: *route_successors }
    }

    fn drive_batch(&mut self, event: ClientEvent, sample: Result<MonotonicMillis, ClockReadError>) -> ClientResult {
        if matches!(event, ClientEvent::Stop) {
            self.stop_batches(ClientError::Stopped);
            return self.batch_effect();
        }
        let ClientState::Batch { route_probe, .. } = &self.state;
        if event.operation_id().is_some_and(|id| route_probe.operation_id() == Some(id)) {
            return self.consume_route_event(event, sample);
        }
        let event = match event {
            ClientEvent::WriteUnwritten { operation_id, request_id, source } => {
                if matches!(&self.state, ClientState::Batch {
                    connection: BatchConnection::Connected { operation_id: current, write: Some((id, _)), .. }
                        | BatchConnection::Switching { operation_id: current, write: Some((id, _)), .. }, ..
                } if *current == operation_id && *id == request_id)
                {
                    self.observe_unwritten(operation_id, request_id);
                }
                ClientEvent::Written { operation_id, request_id, result: Err(source) }
            }
            ClientEvent::ClosedUnwritten { operation_id, request_id } => {
                if matches!(&self.state, ClientState::Batch {
                    connection: BatchConnection::Closing { operation_id: current, .. }, ..
                } if *current == operation_id)
                {
                    self.observe_unwritten(operation_id, request_id);
                }
                ClientEvent::Closed { operation_id }
            }
            event => event,
        };
        let event = match event {
            event @ (ClientEvent::Read { operation_id, .. } | ClientEvent::VerifiedRead { operation_id, .. }) => {
                return self.read_batch_reply(operation_id, event, sample);
            }
            ClientEvent::Written { operation_id, request_id, result } => {
                return self.consume_batch_write(operation_id, request_id, result, sample, None);
            }
            ClientEvent::PartialReply { operation_id, written: Some(request_id), cursor } => {
                return self.consume_batch_write(operation_id, request_id, Ok(()), sample, Some((operation_id, cursor)));
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
            event @ (ClientEvent::Drive | ClientEvent::PartialReply { written: None, .. } | ClientEvent::Connected { result: Ok(()), .. }) => event,
            _ => return ClientResult::DriveRejected { event, cause: ClientError::InvalidEvent },
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
        if let Some(effect) = self.expire_batch_exchange(now, event.partial_reply()) {
            return effect;
        }
        self.poll_for_silence(now, event.partial_reply());
        if self.finish_batch_switch(now) {
            return self.batch_effect();
        }
        if matches!(event, ClientEvent::Drive | ClientEvent::PartialReply { .. })
            && let Some(effect) = self.drive_route_probe(now)
        {
            return effect;
        }
        let ClientState::Batch { connection, .. } = &mut self.state;
        match event {
            ClientEvent::Drive | ClientEvent::PartialReply { .. } => {
                if let BatchConnection::Disconnected { retry_at, peer } = connection {
                    if now >= *retry_at {
                        let peer = *peer;
                        return match self.begin_batch_connect(peer, now) {
                            Ok(effect) => effect,
                            Err(cause @ ClientError::InvalidTime { .. }) => ClientResult::ClockFailed { cause },
                            Err(cause) => ClientResult::DriveRejected { event, cause },
                        };
                    }
                }
                if let Some(effect) = self.begin_batch_exchange(now, event) {
                    return effect;
                }
            }
            ClientEvent::Connected { operation_id, result: Ok(()) } => {
                if let BatchConnection::Connecting { operation_id: pending, peer, .. } = connection {
                    if operation_id == *pending {
                        *connection =
                            BatchConnection::Connected { operation_id, peer: *peer, connected_at: now, write: None, first_unanswered: None };
                    }
                }
            }
            _ => unreachable!("batch observations checked above"),
        }
        self.batch_effect()
    }

    fn begin_batch_connect(&mut self, peer: usize, now: MonotonicMillis) -> Result<ClientResult, ClientError> {
        let ClientState::Batch { next_connection_id, operation_millis, .. } = &self.state;
        let deadline = match now.checked_add(*operation_millis) {
            Ok(deadline) => deadline,
            Err(source) => {
                self.stop_batches(ClientError::InvalidTime { source });
                return Err(ClientError::InvalidTime { source });
            }
        };
        let deadline = self.cap_handover_deadline(deadline);
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
        sample: Result<MonotonicMillis, ClockReadError>,
    ) -> ClientResult {
        let now = self.checked_batch_time(sample);
        if let Ok(now) = now {
            self.expire_batches(now);
        }
        let ClientState::Batch { connection, slot, route_probe, last_now, unavailable_replies, .. } = &mut self.state;
        if let BatchConnection::Closing { operation_id: pending, retry } = connection {
            if operation_id == *pending {
                if let Some(BatchSlot::Pending { delivery, .. }) = slot {
                    if let SubmitDelivery::Pending { request_id, .. } = delivery {
                        let request_id = *request_id;
                        delivery.returned(operation_id, request_id);
                    }
                }
                *connection = match *retry {
                    Some((retry_at, peer)) => match *route_probe {
                        RouteProbe::Successor { operation_id, peer: target } if target == peer => {
                            *route_probe = RouteProbe::Idle;
                            *unavailable_replies = 0;
                            BatchConnection::Connected {
                                operation_id, peer, connected_at: *last_now, write: None, first_unanswered: None,
                            }
                        }
                        _ => BatchConnection::Disconnected { retry_at, peer },
                    },
                    None => BatchConnection::Stopped,
                };
            }
        }
        match now {
            Ok(_) => self.batch_effect(),
            Err(cause) => ClientResult::ClockFailed { cause },
        }
    }

    fn observe_submit_write(&mut self, operation_id: u64, request_id: NonZeroU64) {
        let ClientState::Batch { slot, .. } = &mut self.state;
        if let Some(BatchSlot::Pending { delivery, .. }) = slot {
            delivery.returned(operation_id, request_id);
        }
    }

    fn observe_unwritten(&mut self, operation_id: u64, request_id: NonZeroU64) {
        let ClientState::Batch { slot, .. } = &mut self.state;
        if let Some(BatchSlot::Pending { delivery, .. }) = slot {
            delivery.unwritten(operation_id, request_id);
        }
    }

    fn consume_failed_connect(
        &mut self,
        operation_id: u64,
        source: std::io::Error,
        sample: Result<MonotonicMillis, ClockReadError>,
    ) -> ClientResult {
        let now = self.checked_batch_time(sample);
        if let Ok(now) = now {
            self.expire_batches(now);
        }
        let next_peer = self.next_batch_peer();
        let ClientState::Batch { connection, .. } = &mut self.state;
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
            let retry_at = match self.next_reconnect_at(now) {
                Ok(due) => due,
                Err(time_failure) => {
                    self.stop_batches(ClientError::InvalidTime { source: time_failure });
                    return ClientResult::ConnectionFailed { operation_id, source, next_wake: None };
                }
            };
            let ClientState::Batch { connection, .. } = &mut self.state;
            *connection = BatchConnection::Disconnected { retry_at, peer };
        }
        ClientResult::ConnectionFailed { operation_id, source, next_wake: self.batch_deadline() }
    }

    fn checked_batch_time(
        &mut self,
        sample: Result<MonotonicMillis, ClockReadError>,
    ) -> Result<MonotonicMillis, ClientError> {
        let ClientState::Batch { last_now, .. } = &mut self.state;
        match sample {
            Ok(now) if now >= *last_now => {
                *last_now = now;
                Ok(now)
            }
            Ok(observed) => {
                let previous = *last_now;
                self.stop_batches(ClientError::TimeRegressed { previous, observed });
                Err(ClientError::TimeRegressed { previous, observed })
            }
            Err(source) => {
                self.stop_batches(ClientError::InvalidTime { source });
                Err(ClientError::InvalidTime { source })
            }
        }
    }

    fn expire_batches(&mut self, now: MonotonicMillis) {
        self.expire_handover(now);
        let ClientState::Batch { slot, .. } = &self.state;
        if slot
            .iter()
            .any(|slot| matches!(slot, BatchSlot::Pending { deadline, .. } if now >= *deadline))
        {
            self.stop_batches(ClientError::RequestLifetimeExpired);
        }
    }

    fn stop_batches(&mut self, cause: ClientError) {
        self.cancel_route_probe();
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
        let ClientState::Batch { slot, route_probe, .. } = &self.state;
        if route_probe.has_pending_discovery() && !batch_can_probe(slot) {
            self.cancel_route_probe();
        }
        // Stop can retire the other socket while a physical return is still
        // pending. The port consumes one exact Close before issuing another.
        if self.batch_close_in_flight() {
            return ClientResult::Waiting { wake_at: self.batch_deadline() };
        }
        if let Some(effect) = self.route_close_effect() {
            return effect;
        }
        let ClientState::Batch { connection, .. } = &mut self.state;
        if let BatchConnection::CloseRequired { operation_id, retry } = *connection {
            *connection = BatchConnection::Closing { operation_id, retry };
            return ClientResult::Close { operation_id };
        }
        ClientResult::Waiting { wake_at: self.batch_deadline() }
    }

    fn batch_close_in_flight(&self) -> bool {
        let ClientState::Batch { connection, route_probe, .. } = &self.state;
        matches!(connection, BatchConnection::Closing { .. }) || matches!(route_probe, RouteProbe::Closing { .. })
    }

    fn batch_deadline(&self) -> Option<MonotonicMillis> {
        let ClientState::Batch { connection, slot, last_now, route_probe, .. } = &self.state;
        let close_in_flight = self.batch_close_in_flight();
        let connection_deadline = match connection {
            BatchConnection::Disconnected { retry_at, .. } => Some(*retry_at),
            BatchConnection::Connecting { deadline, .. } => Some(*deadline),
            BatchConnection::Connected { write: Some((_, deadline)), .. }
            | BatchConnection::Switching { write: Some((_, deadline)), .. } => Some(*deadline),
            BatchConnection::Switching { write: None, .. } if !batch_has_accepted_exchange(slot) => Some(*last_now),
            // try_submit can expire the session while returning the refused
            // input, so it cannot also return the Close effect. Keep that debt
            // runnable until drive actually hands it to the physical port.
            BatchConnection::CloseRequired { .. } if !close_in_flight => Some(*last_now),
            _ => None,
        };
        let discovery_deadline = match route_probe {
            RouteProbe::CloseRequired { .. } if close_in_flight => None,
            probe => probe.wake_at(*last_now),
        };
        connection_deadline
            .into_iter()
            .chain(discovery_deadline)
            .chain(slot.iter().filter_map(|s| match s {
                BatchSlot::Pending { handover: Some((_, deadline, _)), .. } => Some(*deadline),
                _ => None,
            }))
            .chain(slot.iter().filter_map(|slot| match slot {
                BatchSlot::Pending { deadline, attempt, .. } => Some(match attempt {
                    BatchAttempt::Eligible { at }
                        if matches!(connection, BatchConnection::Connected { write: None, .. }) =>
                    {
                        (*deadline).min(*at)
                    }
                    BatchAttempt::Awaiting { deadline: exchange, probe_after, .. }
                    | BatchAttempt::Querying { deadline: exchange, probe_after, .. } =>
                        (*deadline).min(*exchange).min(
                            probe_after.filter(|_| matches!(route_probe, RouteProbe::Idle)).unwrap_or(*exchange)
                        ),
                    BatchAttempt::SuspendedUnreported { query: Some((_, _, exchange, _)), .. }
                    | BatchAttempt::Suspended { query: Some((_, _, exchange, _)), .. } => (*deadline).min(*exchange),
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
fn validate_batch_state(
    state: &ClientState,
    admission: Option<(&BatchSlot, NonZeroU64)>,
    connecting: Option<(&BatchConnection, u64)>,
) -> Result<(), ClientError> {
    let ClientState::Batch {
        context,
        stream,
        endpoints,
        next_connection_id,
        connection,
        route_probe,
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
                    delivery: SubmitDelivery::Unsent,
                    committed: None,
                    compatibility_exchange: BatchNextExchange::Submit,
                    last_handover_term: None,
                    handover: None,
                    probe_budget: ROUTE_PROBE_LIMIT,
                    notice: None,
                    submit_route: None,
                    ..
                } | BatchSlot::Pending {
                    attempt: BatchAttempt::Eligible { .. },
                    submit_transmissions: 2,
                    delivery: SubmitDelivery::Uncertain,
                    compatibility_exchange: BatchNextExchange::Submit,
                    last_handover_term: None,
                    handover: None,
                    probe_budget: ROUTE_PROBE_LIMIT,
                    notice: None,
                    submit_route: None,
                    ..
                }
            )
            || context.request_id.get().checked_add(1) != Some(next.get())
            || candidate.handle().context.request_id != context.request_id
        {
            return Err(ClientError::BatchReservationInvariant);
        }
    }
    if *next_connection_id == 0 || *unavailable_replies > *unavailable_reply_limit {
        return Err(ClientError::BatchConnectionInvariant);
    }
    if !route_probe.valid_for(connection, slot, endpoints.len(), *next_connection_id) {
        return Err(ClientError::BatchConnectionInvariant);
    }
    if let Some((candidate, next)) = connecting {
        if !matches!(connection, BatchConnection::Disconnected { .. })
            || next_connection_id.checked_add(1) != Some(next)
            || !matches!(candidate, BatchConnection::Connecting { operation_id, .. } if operation_id == next_connection_id)
        {
            return Err(ClientError::BatchConnectionInvariant);
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
            return Err(ClientError::BatchConnectionInvariant);
        }
        BatchConnection::CloseRequired { retry: Some((_, peer)), .. }
        | BatchConnection::Closing { retry: Some((_, peer)), .. }
            if *peer >= endpoints.len() =>
        {
            return Err(ClientError::BatchConnectionInvariant);
        }
        _ => {}
    }
    if let BatchConnection::Connected { write: Some((id, _)), .. }
    | BatchConnection::Switching { write: Some((id, _)), .. } = connection
    {
        if *id >= request_id {
            return Err(ClientError::BatchConnectionInvariant);
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
            return Err(ClientError::BatchConnectionInvariant);
        }
        _ => {}
    }
    if let Some(slot) = slot {
        let handle = slot.handle();
        if (stream.client_id, stream.stream_id) != (handle.intent.client_id(), handle.intent.stream_id())
            || stream.client_seq != handle.intent.first_sequence()
            || handle.context.cluster_identity != context.cluster_identity
            || handle.context.genesis_identity != context.genesis_identity
            || handle.context.execution_lineage != context.execution_lineage
            || handle.context.session_incarnation != context.session_incarnation
            || handle.context.request_id >= request_id
        {
            return Err(ClientError::BatchReservationInvariant);
        }
        if matches!(slot, BatchSlot::Pending { .. }) && connection.stopped() {
            return Err(ClientError::BatchReservationInvariant);
        }
        match slot {
            BatchSlot::Pending {
                attempt,
                committed,
                compatibility_exchange,
                notice,
                submit_route,
                submit_transmissions,
                delivery,
                last_handover_term,
                handover,
                probe_budget,
                ..
            } => {
                if *submit_transmissions > 2
                    || *probe_budget > ROUTE_PROBE_LIMIT
                    || handover.as_ref().is_some_and(|(target, _, _)| {
                        *target >= endpoints.len() || last_handover_term.is_none_or(|term| term.get() == u64::MAX)
                    })
                    || (*submit_transmissions == 0
                        && (!matches!(attempt, BatchAttempt::Eligible { .. })
                            || committed.is_some()
                            || *compatibility_exchange != BatchNextExchange::Submit
                            || notice.is_some()
                            || submit_route.is_some()
                            || !matches!(delivery, SubmitDelivery::Unsent)))
                    || (attempt.query().is_some()
                        && committed.is_none()
                        && *compatibility_exchange == BatchNextExchange::Submit
                        && handover.is_none())
                {
                    return Err(ClientError::BatchReservationInvariant);
                }
                if let SubmitDelivery::Sole { operation_id, request_id: exposed }
                | SubmitDelivery::Pending { operation_id, request_id: exposed } = delivery
                {
                    if *operation_id == 0
                        || *operation_id >= next_connection_id
                        || *exposed < handle.context.request_id
                        || *exposed >= request_id
                    {
                        return Err(ClientError::BatchReservationInvariant);
                    }
                }
                if matches!(attempt, BatchAttempt::Awaiting { operation_id, .. }
                    if submit_route.map(|(id, _, _)| id) != Some(*operation_id))
                {
                    return Err(ClientError::BatchConnectionInvariant);
                }
                for id in submit_route
                    .iter()
                    .map(|(id, _, _)| *id)
                    .chain(attempt.query().map(|(id, ..)| id))
                {
                    if !matches!(connection, BatchConnection::Connected { operation_id, .. }
                        | BatchConnection::Switching { operation_id, .. } if id == *operation_id)
                    {
                        return Err(ClientError::BatchConnectionInvariant);
                    }
                }
                if let Some((_, first, last)) = submit_route {
                    if *first > *last
                        || *last >= request_id
                        || *first < handle.context.request_id
                        || matches!(attempt, BatchAttempt::Awaiting { request_id, .. } if request_id != last)
                        || (*submit_transmissions == 1 && (*first != handle.context.request_id || first != last))
                    {
                        return Err(ClientError::BatchReservationInvariant);
                    }
                }
                if let Some((_, id, ..)) = attempt.query() {
                    if id >= request_id
                        || id <= handle.context.request_id
                        || submit_route.is_some_and(|(_, _, last)| id <= last)
                    {
                        return Err(ClientError::BatchReservationInvariant);
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
                    return Err(ClientError::BatchReservationInvariant);
                }
            }
            // Only BatchTerminal can occupy this variant. Progress cannot
            // become a settled slot even before the invariant is checked.
            BatchSlot::Terminal { event: BatchTerminal::OutcomeUnknown { .. }, .. } if !connection.stopped() => {
                return Err(ClientError::BatchReservationInvariant);
            }
            BatchSlot::Terminal { .. } => {}
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "ut_batch_client.rs"]
mod ut_batch_client;

#[cfg(kani)]
#[path = "kani_submit_delivery.rs"]
mod kani_submit_delivery;

include!("batch_reply.rs");
