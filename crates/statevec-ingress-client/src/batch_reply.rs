// Same IngressClientOwner domain. The connection owns one exact physical write;
// the slot owns the immutable intent, reply correlation and terminal custody.
impl IngressClientOwner {
    fn begin_batch_exchange(&mut self, now: MonotonicMillis) -> Option<ClientResult> {
        let ClientState::Batch {
            connection: BatchConnection::Connected { operation_id, write: None, .. },
            slot,
            maximum_frame_bytes,
            maximum_commands,
            operation_millis,
            context,
            ..
        } = &self.state
        else {
            return None;
        };
        let Some(BatchSlot::Pending { handle, batch, attempt, committed, submit_transmissions, .. }) = slot else {
            return None;
        };
        let eligible = match attempt {
            BatchAttempt::Eligible { at } => now >= *at,
            _ => false,
        };
        if !eligible {
            return None;
        }
        if let Err(cause) = validate_batch_state(&self.state, None, None) {
            return Some(ClientResult::DriveRejected { event: ClientEvent::Drive, cause });
        }
        // Immutable intent and server admission/history checks make uncertain
        // Submit retry safe. No query receipt, term floor or socket grants it.
        // Commitment remains irreversible and makes subsequent exchanges read-only.
        let submit = committed.is_none();
        // First dispatch consumes the unique ID reserved at local acceptance.
        // Every subsequent exchange consumes a fresh ID; the app handle stays put.
        let first = *submit_transmissions == 0;
        let request_context = if first { handle.context } else { *context };
        let next_request_id =
            if first { Ok(context.request_id) } else { adapter::next_batch_request_id(context.request_id) };
        let next_request_id = match next_request_id {
            Ok(next) => next,
            Err(cause) => {
                self.suspend_batch(cause);
                return Some(self.batch_effect());
            }
        };
        let deadline = match now.checked_add(*operation_millis) {
            Ok(deadline) => deadline,
            Err(source) => {
                self.stop_batches(ClientFailure::InvalidTime { source });
                return Some(ClientResult::ClockFailed { cause: ClientFailure::InvalidTime { source } });
            }
        };
        let encode =
            if submit { ingress_api::stream_batch::encode_submit } else { ingress_api::stream_batch::encode_reconcile };
        let bytes = match encode(&request_context, batch, *maximum_frame_bytes as usize, *maximum_commands as usize) {
            Ok(bytes) => bytes,
            Err(source) => {
                self.suspend_batch(ClientFailure::BatchFrame { source });
                return Some(self.batch_effect());
            }
        };
        let (operation_id, request_id) = (*operation_id, request_context.request_id);
        let ClientState::Batch { connection: BatchConnection::Connected { write, .. }, slot, context, .. } =
            &mut self.state
        else {
            unreachable!("non-yielding write publication")
        };
        let Some(BatchSlot::Pending { attempt, submit_route, submit_transmissions, .. }) = slot else {
            unreachable!("eligible slot")
        };
        if submit {
            // Keep strong evidence from every actual Submit on this live
            // connection. Only the last ID can end the current exchange wait.
            let first = submit_route.map_or(request_id, |(_, first, _)| first);
            *submit_route = Some((operation_id, first, request_id));
            *submit_transmissions = (*submit_transmissions + 1).min(2);
            *attempt = BatchAttempt::Awaiting { operation_id, deadline };
        } else {
            *attempt = BatchAttempt::Querying { operation_id, request_id, deadline };
        }
        context.request_id = next_request_id;
        *write = Some((request_id, deadline));
        Some(ClientResult::Write { operation_id, request_id, bytes })
    }

    fn consume_batch_write(
        &mut self,
        operation_id: u64,
        request_id: NonZeroU64,
        result: std::io::Result<()>,
        sample: Result<MonotonicMillis, ClockReadFailure>,
    ) -> ClientResult {
        // Physical buffer return precedes the time guard, even when the reply
        // already latched/was consumed. A stale ID cannot clear another write.
        let returned_deadline = match &mut self.state {
            ClientState::Batch {
                connection:
                    BatchConnection::Connected { operation_id: current, write, .. }
                    | BatchConnection::Switching { operation_id: current, write, .. },
                ..
            } if *current == operation_id && write.is_some_and(|(id, _)| id == request_id) => {
                write.take().map(|(_, deadline)| deadline)
            }
            _ => None,
        };
        let now = self.checked_batch_time(sample);
        if let Ok(now) = now {
            self.expire_batches(now);
        }
        if returned_deadline.is_some() {
            if let Err(source) = result {
                self.require_batch_close(operation_id, now.ok());
                return ClientResult::ConnectionFault { operation_id, cause: ClientFailure::BatchWrite { source } };
            }
        }
        match now {
            Ok(now) => {
                if returned_deadline.is_some_and(|deadline| now >= deadline) {
                    self.require_batch_close(operation_id, Some(now));
                }
                self.expire_batch_exchange(now);
                self.batch_effect()
            }
            Err(cause) => ClientResult::ClockFailed { cause },
        }
    }

    fn read_batch_reply(
        &mut self,
        operation_id: u64,
        result: Result<Vec<u8>, ingress_api::stream_batch::ReadFailure>,
        sample: Result<MonotonicMillis, ClockReadFailure>,
    ) -> ClientResult {
        let matching = matches!(&self.state, ClientState::Batch {
            connection: BatchConnection::Connected { operation_id: current, .. }
                | BatchConnection::Switching { operation_id: current, .. }, ..
        } if *current == operation_id);
        let now = self.checked_batch_time(sample);
        if let Ok(now) = now {
            self.expire_batches(now);
        }
        if !matching {
            return match now {
                Ok(_) => self.batch_effect(),
                Err(cause) => ClientResult::ClockFailed { cause },
            };
        }
        let bytes = match result {
            Ok(bytes) => bytes,
            Err(source) => {
                self.require_batch_close(operation_id, now.ok());
                return ClientResult::ConnectionFault { operation_id, cause: ClientFailure::BatchRead { source } };
            }
        };
        let now = match now {
            Ok(now) => now,
            Err(cause) => {
                return ClientResult::ClockFailed { cause };
            }
        };
        self.expire_batch_exchange(now);
        // Expiry/Stop wins before a reply processed on this turn. An old socket
        // cannot supply bytes to a successor, including the same peer and term.
        let ClientState::Batch {
            connection:
                BatchConnection::Connected { operation_id: current, .. }
                | BatchConnection::Switching { operation_id: current, .. },
            maximum_frame_bytes,
            maximum_commands,
            slot,
            ..
        } = &self.state
        else {
            return self.batch_effect();
        };
        if *current != operation_id {
            return self.batch_effect();
        }
        if let Err(cause) = validate_batch_state(&self.state, None, None) {
            return ClientResult::DriveRejected { event: ClientEvent::Read { operation_id, result: Ok(bytes) }, cause };
        }
        let reply = match ingress_api::stream_batch::decode_reply(
            &bytes,
            *maximum_frame_bytes as usize,
            *maximum_commands as usize,
        ) {
            Ok(reply) => reply,
            Err(source) => {
                self.require_batch_close(operation_id, Some(now));
                return ClientResult::ConnectionFault { operation_id, cause: ClientFailure::BatchFrame { source } };
            }
        };
        // Associate an actually dispatched operation before validating context.
        // A same-session/connection reply alone does not prove a send: exclude
        // previous batches, reserved-but-unsent IDs, future IDs and retired queries.
        if !matches!(slot,
            Some(BatchSlot::Pending { handle, attempt, submit_route, .. })
                if handle.context.session_incarnation == reply.binding.context.session_incarnation
                    && (submit_route.is_some_and(|(connection, first, last)|
                            connection == operation_id && first <= reply.binding.context.request_id
                                && reply.binding.context.request_id <= last)
                        || attempt.query().is_some_and(|(connection, request, _)|
                            connection == operation_id && request == reply.binding.context.request_id))
        ) {
            return self.batch_effect();
        };
        let handle = slot.as_ref().expect("matched slot").handle();
        let query = matches!(slot, Some(BatchSlot::Pending { attempt, .. })
            if attempt.query().is_some_and(|(connection, request, _)|
                connection == operation_id && request == reply.binding.context.request_id));
        let current_exchange = query
            || matches!(slot,
            Some(BatchSlot::Pending { attempt: BatchAttempt::Awaiting { .. }, submit_route: Some((_, _, last)), .. })
                if *last == reply.binding.context.request_id);
        if !current_exchange
            && !matches!(
                &reply.disposition,
                ReplyDisposition::CommittedAwaitingApply(_)
                    | ReplyDisposition::Completed { .. }
                    | ReplyDisposition::ExactAppliedResultUnavailable(_)
            )
        {
            return self.batch_effect();
        }
        let expected_context = RequestContext { request_id: reply.binding.context.request_id, ..handle.context };
        let joined = if query {
            reply.check_reconcile(expected_context, handle.intent)
        } else {
            reply.check_submit(expected_context, handle.intent)
        };
        if let Err(source) = joined {
            if query {
                self.consume_batch_query();
            }
            self.suspend_batch(ClientFailure::BatchReply { source });
            return self.batch_effect();
        }
        self.reduce_batch_reply(reply, query, current_exchange, now);
        self.batch_effect()
    }

    fn reduce_batch_reply(
        &mut self,
        reply: ingress_api::stream_batch::Reply,
        query: bool,
        current_exchange: bool,
        now: MonotonicMillis,
    ) {
        let disposition = reply.disposition;
        if query {
            self.consume_batch_query();
        }
        let ClientState::Batch { slot, unavailable_replies, unavailable_reply_limit, .. } = &mut self.state;
        let next_unavailable = (*unavailable_replies + 1).min(*unavailable_reply_limit);
        let unavailable_limit = *unavailable_reply_limit;
        let Some(BatchSlot::Pending { handle, committed, attempt, submit_transmissions, .. }) = slot else {
            return;
        };
        let handle = *handle;
        // The retained Submit can still prove commitment/exact application,
        // but late weak replies are not responses to a newer exchange.
        let incoming_committed = match &disposition {
            ReplyDisposition::CommittedAwaitingApply(entry) | ReplyDisposition::Completed { entry, .. } => Some(*entry),
            ReplyDisposition::ExactAppliedResultUnavailable(binding) => Some(EntryBinding {
                index: NonZeroU64::new(binding.raft_index()).expect("checked applied index"),
                term: NonZeroU64::new(binding.raft_term()).expect("checked applied term"),
                digest: binding.entry_digest(),
            }),
            _ => None,
        };
        if !current_exchange && incoming_committed.is_none() {
            return;
        }
        if let (Some(known), Some(observed)) = (*committed, incoming_committed) {
            if known != observed {
                self.suspend_batch(ClientFailure::ConflictingCommittedEntry { known, observed });
                return;
            }
        }
        // First-send non-admission requires the sole exchange, no earlier
        // qualified admission/pending reply and no confirmed committed history.
        let first_reply = current_exchange
            && !query
            && *submit_transmissions == 1
            && matches!(attempt, BatchAttempt::Awaiting { .. })
            && committed.is_none();
        // Only ending the current exchange arms a new observation interval.
        // A late Submit reply cannot replace an outstanding query or refresh it.
        if current_exchange {
            self.schedule_batch_observation(now);
        }
        let ClientState::Batch { slot, .. } = &mut self.state;
        let Some(BatchSlot::Pending { committed, attempt, notice, .. }) = slot else { return };
        let newly_admitted = matches!(&disposition, ReplyDisposition::IngressAdmitted(_));
        let routing_count = match disposition {
            ReplyDisposition::IngressAdmitted(entry) | ReplyDisposition::PendingKnown(entry) => {
                if committed.is_none() && !attempt.is_suspended() {
                    *notice = Some(BatchProgress::IngressAdmitted { handle, entry });
                    // A current IngressAdmitted certifies a new proposal, even
                    // after retransmission/import. Repeated PendingKnown does not.
                    Some(if newly_admitted || first_reply { 0 } else { next_unavailable })
                } else {
                    // An older node's pending proposal cannot undo known
                    // commitment, but it also cannot pin read-only recovery.
                    committed.is_some().then_some(next_unavailable)
                }
            }
            ReplyDisposition::CommittedAwaitingApply(entry) => {
                if committed.is_none() {
                    *committed = Some(entry);
                    *notice = Some(BatchProgress::CommittedAwaitingApply { handle, entry });
                    Some(0)
                } else {
                    // Repeating the same committed fact is not new progress
                    // toward exact apply, even if this peer answers promptly.
                    Some(next_unavailable)
                }
            }
            ReplyDisposition::Completed { entry, results } => {
                Self::latch_batch_terminal(slot, BatchTerminal::Applied { handle, entry, results, problem: None });
                Some(0)
            }
            ReplyDisposition::ExactAppliedResultUnavailable(binding) => {
                Self::latch_batch_terminal(
                    slot,
                    BatchTerminal::ExactAppliedResultUnavailable { handle, binding, problem: None },
                );
                Some(0)
            }
            ReplyDisposition::ProcessedResultUnavailable { next_sequence } => {
                self.suspend_batch(ClientFailure::InsufficientAppliedEvidence { next_sequence });
                None
            }
            ReplyDisposition::Conflict => {
                self.suspend_batch(ClientFailure::BatchConflict);
                None
            }
            ReplyDisposition::StreamGapRejected { expected_seq } => {
                self.refuse_submit_reply(ClientFailure::StreamGap { expected_sequence: expected_seq }, first_reply);
                None
            }
            ReplyDisposition::Refused(reason) => {
                // A later refusal concerns this attempt, never undoing known
                // commitment or stopping its future read-only reconciliation.
                if committed.is_some() && !query {
                    return;
                }
                match reason {
                    BatchSubmitRefusal::NotReady | BatchSubmitRefusal::Busy => {
                        current_exchange.then_some(next_unavailable)
                    }
                    BatchSubmitRefusal::InternalFailure | BatchSubmitRefusal::ShuttingDown if query => {
                        // A failed query describes this node, not the earlier
                        // Submit. Drain accepted operations, then choose another
                        // configured node without granting another Submit.
                        Some(unavailable_limit)
                    }
                    BatchSubmitRefusal::ShuttingDown if !first_reply => {
                        // A draining peer cannot settle earlier transmissions.
                        // Keep the original intent, retire this route and retry
                        // against another node; no query permission is needed.
                        current_exchange.then_some(unavailable_limit)
                    }
                    BatchSubmitRefusal::InternalFailure
                        if reply.observation.is_some_and(|observation| {
                            observation.unavailable_reason() == Some(IngressUnavailableReason::Failed)
                        }) =>
                    {
                        // FatalDraining reports node unavailability. It does
                        // not prove this original intent failed or never ran.
                        Some(unavailable_limit)
                    }
                    BatchSubmitRefusal::InternalFailure => {
                        // Without the explicit node-failure observation, retain
                        // the execution/unknown fault rather than blindly retry.
                        self.suspend_batch(ClientFailure::ServerRefused { reason });
                        None
                    }
                    BatchSubmitRefusal::TooLarge
                    | BatchSubmitRefusal::InvalidInput
                    | BatchSubmitRefusal::Unauthorized
                    | BatchSubmitRefusal::ShuttingDown
                    | BatchSubmitRefusal::IncompatibleContext => {
                        self.refuse_submit_reply(ClientFailure::ServerRefused { reason }, first_reply);
                        None
                    }
                }
            }
            ReplyDisposition::Unknown => Some(next_unavailable),
            ReplyDisposition::RetryableNext => {
                if let Some(known) = *committed {
                    // An isolated older leader may still serve its own settled
                    // history. Its absence cannot contradict a later commit.
                    if reply
                        .observation
                        .is_some_and(|observation| observation.current_term() < known.term.get())
                    {
                        Some(unavailable_limit)
                    } else {
                        self.suspend_batch(ClientFailure::CommittedHistoryMissing { known });
                        None
                    }
                } else {
                    None
                }
            }
        };
        if let Some(count) = routing_count.filter(|_| current_exchange) {
            self.observe_batch_route(count);
        }
    }

    fn latch_batch_terminal(slot: &mut Option<BatchSlot>, mut event: BatchTerminal) {
        let Some(BatchSlot::Pending { attempt, .. }) = slot.as_ref() else { return };
        if let BatchTerminal::Applied { problem, .. } | BatchTerminal::ExactAppliedResultUnavailable { problem, .. } =
            &mut event
        {
            *problem = match attempt {
                BatchAttempt::SuspendedUnreported { problem, .. } | BatchAttempt::Suspended { problem, .. } => {
                    Some(problem.clone())
                }
                _ => None,
            };
        }
        // The original is no longer unresolved only after exact application.
        // Capacity and stream advancement still wait for application consumption.
        *slot = Some(BatchSlot::Terminal { event });
    }

    fn refuse_submit_reply(&mut self, cause: ClientFailure, proven: bool) {
        if !proven {
            self.suspend_batch(cause);
            return;
        }
        let ClientState::Batch { slot, .. } = &mut self.state;
        let Some(BatchSlot::Pending { handle, batch, .. }) = slot.take() else {
            unreachable!("qualified pending refusal")
        };
        *slot = Some(BatchSlot::Terminal { event: BatchTerminal::NotAdmitted { handle, batch, cause } });
    }

    fn suspend_batch(&mut self, cause: ClientFailure) {
        let ClientState::Batch { slot, .. } = &mut self.state;
        let Some(BatchSlot::Pending { attempt, notice, .. }) = slot else { return };
        if attempt.is_suspended() {
            return;
        }
        *attempt = BatchAttempt::SuspendedUnreported { query: attempt.query(), problem: Arc::new(cause) };
        // Known commitment stays observable; earlier reversible admission need
        // not follow the diagnostic. Neither notification can resume work.
        if matches!(notice, Some(BatchProgress::IngressAdmitted { .. })) {
            *notice = None;
        }
    }

    fn consume_batch_query(&mut self) {
        let ClientState::Batch { slot, last_now, .. } = &mut self.state;
        let Some(BatchSlot::Pending { attempt, .. }) = slot else { return };
        match attempt {
            BatchAttempt::SuspendedUnreported { query, .. } | BatchAttempt::Suspended { query, .. } => *query = None,
            _ => *attempt = BatchAttempt::Eligible { at: *last_now },
        }
    }

    fn schedule_batch_observation(&mut self, now: MonotonicMillis) {
        let ClientState::Batch { slot, retry_millis, .. } = &mut self.state;
        let Some(BatchSlot::Pending { attempt, .. }) = slot else { return };
        if attempt.is_suspended() || matches!(attempt, BatchAttempt::Querying { .. }) {
            return;
        }
        match now.checked_add(*retry_millis) {
            Ok(retry_at) => *attempt = BatchAttempt::Eligible { at: retry_at },
            Err(source) => self.stop_batches(ClientFailure::InvalidTime { source }),
        }
    }

    fn expire_batch_exchange(&mut self, now: MonotonicMillis) -> bool {
        let ClientState::Batch { connection, slot, .. } = &self.state;
        if let BatchConnection::Connecting { operation_id, deadline, .. } = connection {
            if now >= *deadline {
                self.require_batch_close(*operation_id, Some(now));
                return true;
            }
        }
        let (BatchConnection::Connected { operation_id, write, .. }
        | BatchConnection::Switching { operation_id, write, .. }) = connection
        else {
            return false;
        };
        if write.is_some_and(|(_, deadline)| now >= deadline)
            || slot.iter().any(|slot| {
                matches!(slot, BatchSlot::Pending { attempt, .. } if
                matches!(attempt, BatchAttempt::Awaiting { deadline, .. } if now >= *deadline)
                || attempt.query().is_some_and(|(_, _, deadline)| now >= deadline))
            })
        {
            self.require_batch_close(*operation_id, Some(now));
            true
        } else {
            false
        }
    }

    fn require_batch_close(&mut self, operation_id: u64, now: Option<MonotonicMillis>) {
        let next_peer = self.next_batch_peer();
        let ClientState::Batch { connection, slot, retry_millis, last_now, .. } = &mut self.state;
        let (BatchConnection::Connecting { operation_id: current, .. }
        | BatchConnection::Connected { operation_id: current, .. }
        | BatchConnection::Switching { operation_id: current, .. }) = connection
        else {
            return;
        };
        if *current != operation_id {
            return;
        }
        // A missing sample has already stopped the session in checked_batch_time.
        let Some(now) = now else { return };
        let retry_at = match now.checked_add(*retry_millis) {
            Ok(due) => due,
            Err(source) => {
                self.stop_batches(ClientFailure::InvalidTime { source });
                return;
            }
        };
        *connection = BatchConnection::CloseRequired { operation_id, retry: Some((retry_at, next_peer)) };
        for slot in slot.iter_mut() {
            if let BatchSlot::Pending { attempt, submit_route, .. } = slot {
                if submit_route.is_some_and(|(id, _, _)| id == operation_id)
                    || attempt.query().is_some_and(|(id, _, _)| id == operation_id)
                {
                    *submit_route = None;
                    match attempt {
                        BatchAttempt::SuspendedUnreported { query, .. } | BatchAttempt::Suspended { query, .. } => {
                            *query = None
                        }
                        _ => *attempt = BatchAttempt::Eligible { at: *last_now },
                    }
                }
            }
        }
    }
}

include!("batch_routing.rs");
