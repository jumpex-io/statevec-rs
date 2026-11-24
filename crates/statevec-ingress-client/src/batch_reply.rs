// Same BatchClient domain. The connection owns one exact physical write;
// the slot owns the immutable intent, reply correlation and terminal custody.
impl BatchClient {
    fn begin_batch_exchange(&mut self, now: MonotonicMillis, event: ClientEvent) -> Option<ClientResult> {
        let ClientState::Batch {
            connection: BatchConnection::Connected { operation_id, write: None, .. },
            slot,
            maximum_frame_bytes,
            operation_millis,
            context,
            ..
        } = &self.state
        else {
            return None;
        };
        let Some(BatchSlot::Pending {
            handle,
            batch,
            attempt,
            committed,
            compatibility_exchange,
            submit_transmissions,
            handover,
            submit_route,
            ..
        }) = slot
        else {
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
            return Some(ClientResult::DriveRejected { event, cause });
        }
        // Immutable intent and server admission/history checks make uncertain
        // Submit retry safe. No query receipt, term floor or socket grants it.
        // Commitment remains irreversible and makes subsequent exchanges read-only.
        let submit = committed.is_none() && *compatibility_exchange == BatchNextExchange::Submit
            && !matches!(handover, Some((_, _, BatchNextExchange::Reconcile)));
        if committed.is_none() && !submit && submit_route.is_some() {
            // Retire the contiguous Submit-ID interval before another query.
            self.require_batch_close(*operation_id, Some(now));
            return Some(self.batch_effect());
        }
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
                self.stop_batches(ClientError::InvalidTime { source });
                return Some(ClientResult::ClockFailed { cause: ClientError::InvalidTime { source } });
            }
        };
        let deadline = self.cap_handover_deadline(deadline);
        let encode =
            if submit { ingress_api::stream_batch::encode_submit } else { ingress_api::stream_batch::encode_reconcile };
        let encoded = encode(&request_context, batch, *maximum_frame_bytes as usize, usize::from(statevec_frame::release::BATCH_COMMAND_CEILING));
        let (wire_tags, bytes) = match encoded.and_then(|bytes| Ok((crate::encoded_wire_tags(&bytes)?, bytes))) {
            Ok(sent) => sent,
            Err(source) => {
                self.suspend_batch(ClientError::BatchFrame { source });
                return Some(self.batch_effect());
            }
        };
        let (operation_id, request_id) = (*operation_id, request_context.request_id);
        let ClientState::Batch { connection: BatchConnection::Connected { write, first_unanswered, .. }, slot, context, .. } =
            &mut self.state
        else {
            unreachable!("non-yielding write publication")
        };
        let Some(BatchSlot::Pending { attempt, submit_route, submit_transmissions, delivery, handover, .. }) = slot
        else {
            unreachable!("eligible slot")
        };
        if submit {
            if let Some((_, _, next)) = handover {
                *next = BatchNextExchange::Reconcile;
            }
            // Keep strong evidence from every actual Submit on this live
            // connection. Only the last ID can end the current exchange wait.
            let first = submit_route.map_or(request_id, |(_, first, _)| first);
            *submit_route = Some((operation_id, first, request_id));
            *submit_transmissions = (*submit_transmissions + 1).min(2);
            delivery.dispatch(operation_id, request_id);
            *attempt = BatchAttempt::Awaiting { operation_id, request_id, wire_tags, deadline, probe_after: None };
        } else {
            *attempt = BatchAttempt::Querying { operation_id, request_id, wire_tags, deadline, probe_after: None };
        }
        context.request_id = next_request_id;
        *write = Some((request_id, deadline));
        first_unanswered.get_or_insert(request_id);
        Some(ClientResult::Write { operation_id, request_id, bytes })
    }

    fn consume_batch_write(
        &mut self,
        operation_id: u64,
        request_id: NonZeroU64,
        result: std::io::Result<()>,
        sample: Result<MonotonicMillis, ClockReadError>,
        partial: Option<(u64, IncompleteFrame)>,
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
        if returned_deadline.is_some() {
            self.observe_submit_write(operation_id, request_id);
        }
        let now = self.checked_batch_time(sample);
        if let Ok(now) = now {
            self.expire_batches(now);
        }
        if returned_deadline.is_some() {
            if let Err(source) = result {
                self.require_batch_close(operation_id, now.ok());
                return ClientResult::ConnectionFault { operation_id, cause: ClientError::BatchWrite { source } };
            }
        }
        match now {
            Ok(now) => {
                if let Some(effect) = self.expire_batch_exchange(now, partial) {
                    return effect;
                }
                if returned_deadline.is_some_and(|deadline| now >= deadline) {
                    self.require_batch_close(operation_id, Some(now));
                } else if returned_deadline.is_some() && partial.is_none() {
                    self.arm_silence_probe(operation_id, request_id, now);
                }
                self.batch_effect()
            }
            Err(cause) => ClientResult::ClockFailed { cause },
        }
    }

    fn read_batch_reply(
        &mut self,
        operation_id: u64,
        event: ClientEvent,
        sample: Result<MonotonicMillis, ClockReadError>,
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
        if let ClientEvent::Read { result: Err(source), .. } = event {
            self.require_batch_close(operation_id, now.ok());
            return ClientResult::ConnectionFault { operation_id, cause: ClientError::BatchRead { source } };
        }
        let now = match now {
            Ok(now) => now,
            Err(cause) => {
                return ClientResult::ClockFailed { cause };
            }
        };
        if let Some(effect) = self.expire_batch_exchange(now, None) {
            return effect;
        }
        // Expiry/Stop wins before a reply processed on this turn. An old socket
        // cannot supply bytes to a successor, including the same peer and term.
        let ClientState::Batch {
            connection:
                BatchConnection::Connected { operation_id: current, first_unanswered, .. }
                | BatchConnection::Switching { operation_id: current, first_unanswered, .. },
            maximum_frame_bytes,
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
            return ClientResult::DriveRejected { event, cause };
        }
        let (maximum, commands) = (*maximum_frame_bytes as usize, usize::from(statevec_frame::release::BATCH_COMMAND_CEILING));
        let decoded = match event {
            ClientEvent::Read { result: Ok(bytes), .. } => ingress_api::stream_batch::decode_reply(&bytes, maximum, commands),
            ClientEvent::VerifiedRead { frame, .. } => frame.decode_reply(maximum, commands),
            event => return ClientResult::DriveRejected { event, cause: ClientError::InvalidEvent },
        };
        let reply = match decoded {
            Ok(reply) => reply,
            Err(source) => {
                let refusal = source.ingress_refusal();
                if refusal.is_some() || matches!(source, ingress_api::stream_batch::FrameError::UnrecognizedDisposition { .. }) {
                    let Some(BatchSlot::Pending { handle, attempt, .. }) = slot else {
                        return self.batch_effect();
                    };
                    let exchange = match attempt {
                        BatchAttempt::Awaiting { operation_id, request_id, wire_tags, .. } => {
                            Some((*operation_id, *request_id, *wire_tags))
                        }
                        attempt => attempt.query().map(|(socket, id, _, wire_tags)| (socket, id, wire_tags)),
                    };
                    let matches_exchange = exchange.is_some_and(|(socket, id, wire_tags)| {
                        socket == operation_id && match &source {
                            ingress_api::stream_batch::FrameError::UnrecognizedDisposition {
                                session_incarnation, request_id, ..
                            } => *session_incarnation == handle.context.session_incarnation && *request_id == id,
                            _ => refusal.is_some_and(|refusal| crate::matches_refusal(id, wire_tags, refusal, *first_unanswered)),
                        }
                    });
                    if !matches_exchange {
                        return self.batch_effect();
                    }
                    if let ClientState::Batch { slot: Some(BatchSlot::Pending { compatibility_exchange, .. }), .. } = &mut self.state {
                        *compatibility_exchange = BatchNextExchange::Reconcile;
                    }
                }
                self.require_batch_close(operation_id, Some(now));
                return ClientResult::ConnectionFault { operation_id, cause: ClientError::BatchFrame { source } };
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
                        || attempt.query().is_some_and(|(socket, id, ..)|
                            socket == operation_id && id == reply.binding.context.request_id))
        ) {
            return self.batch_effect();
        };
        let handle = slot.as_ref().expect("matched slot").handle();
        let query = matches!(slot, Some(BatchSlot::Pending { attempt, .. })
            if attempt.query().is_some_and(|(socket, id, ..)|
                socket == operation_id && id == reply.binding.context.request_id));
        let current_exchange = query
            || matches!(slot,
            Some(BatchSlot::Pending { attempt: BatchAttempt::Awaiting { .. }, submit_route: Some((_, _, last)), .. })
                if *last == reply.binding.context.request_id);
        let admitted_exchange = matches!(slot, Some(BatchSlot::Pending { admitted: Some((op, id, _)), .. })
            if *op == operation_id && *id == reply.binding.context.request_id);
        if !current_exchange
            && !(admitted_exchange && reply.observation.is_some_and(|o| o.handover().is_some()))
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
            self.suspend_batch(ClientError::BatchReply { source });
            return self.batch_effect();
        }
        if current_exchange {
            self.cancel_route_probe();
            if let ClientState::Batch {
                connection: BatchConnection::Connected { first_unanswered, .. } | BatchConnection::Switching { first_unanswered, .. }, ..
            } = &mut self.state {
                *first_unanswered = None;
            }
        }
        self.observe_submit_write(operation_id, reply.binding.context.request_id);
        let hint = self.handover_hint(&reply, operation_id, query, current_exchange, now);
        let request_id = reply.binding.context.request_id;
        if !query
            && let ReplyDisposition::IngressAdmitted(entry) = &reply.disposition
            && let ClientState::Batch { slot: Some(BatchSlot::Pending { admitted, .. }), .. } = &mut self.state
        {
            *admitted = Some((operation_id, request_id, *entry));
        }
        let ClientState::Batch { retry_millis, idle_reconnect_millis, .. } = &mut self.state;
        *idle_reconnect_millis = *retry_millis;
        self.reduce_batch_reply(reply, query, current_exchange, now);
        if let Some((target, term, deadline)) = hint {
            self.start_handover(operation_id, target, term, deadline, now);
        }
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
        let ClientState::Batch { slot, endpoints, unavailable_replies, unavailable_reply_limit, .. } = &mut self.state;
        let next_unavailable = (*unavailable_replies + 1).min(*unavailable_reply_limit);
        let unavailable_limit = *unavailable_reply_limit;
        let Some(BatchSlot::Pending { handle, committed, attempt, delivery, last_handover_term, handover, .. }) = slot else {
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
                self.suspend_batch(ClientError::ConflictingCommittedEntry { known, observed });
                return;
            }
        }
        // First-send non-admission requires the sole exchange, no earlier
        // qualified admission/pending reply and no confirmed committed history.
        let first_reply = current_exchange
            && !query
            && matches!((delivery, &*attempt), (SubmitDelivery::Sole { operation_id, request_id, .. }, BatchAttempt::Awaiting { operation_id: sent, .. })
                if operation_id == sent && *request_id == reply.binding.context.request_id)
            && committed.is_none();
        // Only ending the current exchange arms a new observation interval.
        // A late Submit reply cannot replace an outstanding query or refresh it.
        // The named successor now holds a live route for this Submit. Keep
        // its original exchange deadline; a disconnect still reconciles first.
        let successor_admitted = !query
            && matches!((&disposition, &*handover, *last_handover_term),
            (ReplyDisposition::IngressAdmitted(entry), Some((target, ..)), Some(term))
                if term.get().checked_add(1) == Some(entry.term.get())
                && reply.observation.is_some_and(|o| o.current_term() == entry.term.get()
                    && o.leader_hint().map(NonZeroU64::get) == Some(endpoints[*target].0)));
        if current_exchange && !successor_admitted {
            self.schedule_batch_observation(now);
        }
        let ClientState::Batch { slot, endpoints: endpoints_for_hint, previous_applied_index, .. } = &mut self.state;
        let Some(BatchSlot::Pending { committed, compatibility_exchange, attempt, notice, last_handover_term, handover, probe_budget, .. }) = slot else { return };
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
                self.suspend_batch(ClientError::InsufficientAppliedEvidence { next_sequence });
                None
            }
            ReplyDisposition::Conflict => {
                self.suspend_batch(ClientError::BatchConflict);
                None
            }
            ReplyDisposition::StreamGapRejected { expected_seq }
                if expected_seq < handle.intent.first_sequence()
                    && reply.observation.is_some_and(|peer| peer.applied_index() < *previous_applied_index) =>
            {
                // This peer has not applied the exact predecessor of our stream
                // cursor. Keep the original and try the next configured peer.
                Some(unavailable_limit)
            }
            ReplyDisposition::StreamGapRejected { expected_seq } => {
                self.refuse_submit_reply(ClientError::StreamGap { expected_sequence: expected_seq }, first_reply);
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
                    BatchSubmitRefusal::TooLarge
                    | BatchSubmitRefusal::InvalidInput
                    | BatchSubmitRefusal::Unauthorized
                    | BatchSubmitRefusal::ShuttingDown
                    | BatchSubmitRefusal::IncompatibleContext => {
                        self.refuse_submit_reply(ClientError::ServerRefused { reason }, first_reply);
                        None
                    }
                    _ => {
                        // InternalFailure without node failure, Core failures
                        // and unrecognized reasons suspend the retained intent.
                        self.suspend_batch(ClientError::ServerRefused { reason });
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
                        self.suspend_batch(ClientError::CommittedHistoryMissing { known });
                        None
                    }
                } else {
                    if query && current_exchange {
                        *compatibility_exchange = BatchNextExchange::Submit;
                        if *probe_budget < ROUTE_PROBE_LIMIT {
                            *attempt = BatchAttempt::Eligible { at: now };
                        }
                    }
                    if query
                        && let (Some(term), Some((target, _, next))) = (*last_handover_term, handover)
                        && reply.observation.is_some_and(|o| {
                            o.current_term() == term.get() + 1
                                && o.leader_hint().map(NonZeroU64::get) == Some(endpoints_for_hint[*target].0)
                        })
                    {
                        *next = BatchNextExchange::Submit;
                    }
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

    fn refuse_submit_reply(&mut self, cause: ClientError, proven: bool) {
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

    fn suspend_batch(&mut self, cause: ClientError) {
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
        let ClientState::Batch { slot, retry_millis, handover_retry_millis, .. } = &mut self.state;
        let retry_millis =
            if matches!(slot, Some(BatchSlot::Pending { handover: Some(_), .. })) {
                handover_retry_millis
            } else {
                retry_millis
            };
        let Some(BatchSlot::Pending { attempt, .. }) = slot else { return };
        if attempt.is_suspended() || matches!(attempt, BatchAttempt::Querying { .. }) {
            return;
        }
        match now.checked_add(*retry_millis) {
            Ok(retry_at) => *attempt = BatchAttempt::Eligible { at: retry_at },
            Err(source) => self.stop_batches(ClientError::InvalidTime { source }),
        }
    }

    fn expire_batch_exchange(&mut self, now: MonotonicMillis, partial: Option<(u64, IncompleteFrame)>) -> Option<ClientResult> {
        let ClientState::Batch { connection, slot, .. } = &self.state;
        let (operation_id, reply_due) = match connection {
            BatchConnection::Connecting { operation_id, deadline, .. } if now >= *deadline => (*operation_id, false),
            BatchConnection::Connected { operation_id, write, .. } | BatchConnection::Switching { operation_id, write, .. } => {
                let reply_due = slot.iter().any(|slot| matches!(slot,
                    BatchSlot::Pending { attempt, .. } if
                    matches!(attempt, BatchAttempt::Awaiting { deadline, .. } if now >= *deadline)
                    || attempt.query().is_some_and(|(_, _, deadline, _)| now >= deadline)));
                if !reply_due && !write.is_some_and(|(_, deadline)| now >= deadline) {
                    return None;
                }
                (*operation_id, reply_due)
            }
            _ => return None,
        };
        self.require_batch_close(operation_id, Some(now));
        if reply_due && let Some(diagnostic) = partial_reply_timeout(operation_id, partial) {
            return Some(diagnostic);
        }
        Some(self.batch_effect())
    }

    fn require_batch_close(&mut self, operation_id: u64, now: Option<MonotonicMillis>) {
        let next_peer = self.next_batch_peer();
        let ClientState::Batch { connection, .. } = &self.state;
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
        let retry_at = match self.next_reconnect_at(now) {
            Ok(due) => due,
            Err(source) => {
                self.stop_batches(ClientError::InvalidTime { source });
                return;
            }
        };
        self.close_batch_for_retry(operation_id, retry_at, next_peer);
    }

    fn close_batch_for_retry(&mut self, operation_id: u64, retry_at: MonotonicMillis, next_peer: usize) {
        self.cancel_route_probe();
        let ClientState::Batch { connection, slot, last_now, .. } = &mut self.state;
        *connection = BatchConnection::CloseRequired { operation_id, retry: Some((retry_at, next_peer)) };
        for slot in slot.iter_mut() {
            if let BatchSlot::Pending { attempt, submit_route, .. } = slot {
                if submit_route.is_some_and(|(id, _, _)| id == operation_id)
                    || attempt.query().is_some_and(|(socket, ..)| socket == operation_id)
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
include!("batch_route_poll.rs");
