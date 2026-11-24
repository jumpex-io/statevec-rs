// Same owner and state: these routing facts cannot mint slot retry authority.
impl BatchClient {
    fn cap_handover_deadline(&self, ordinary: MonotonicMillis) -> MonotonicMillis {
        match &self.state {
            ClientState::Batch {
                slot: Some(BatchSlot::Pending { handover: Some((_, deadline, _)), .. }),
                ..
            } => ordinary.min(*deadline),
            _ => ordinary,
        }
    }

    fn handover_hint(
        &mut self,
        reply: &ingress_api::stream_batch::Reply,
        operation_id: u64,
        query: bool,
        current: bool,
        now: MonotonicMillis,
    ) -> Option<(usize, NonZeroU64, MonotonicMillis)> {
        let o = reply.observation?;
        let ClientState::Batch { connection, endpoints, slot, observed_leader, handover_window_millis, .. } =
            &mut self.state;
        let BatchConnection::Connected { peer, .. } = connection else { return None };
        let source = endpoints[*peer].0;
        let Some(BatchSlot::Pending { deadline, last_handover_term, handover, admitted, attempt, .. }) = slot else { return None };
        let observed = o.current_term();
        if observed < observed_leader.0 {
            return None;
        }
        let leader = o.leader_hint();
        let conflicting = observed == observed_leader.0
            && observed_leader.1.is_some()
            && leader.is_some()
            && leader != observed_leader.1;
        if observed > observed_leader.0 || (observed_leader.1.is_none() && leader.is_some()) {
            *observed_leader = (observed, leader);
        }
        if let (Some(term), Some((target, _, _))) = (*last_handover_term, handover.as_ref())
            && (conflicting
                || observed > term.get() + 1
                || (observed == term.get() + 1 && leader.is_some_and(|p| p.get() != endpoints[*target].0)))
        {
            *handover = None;
            self.require_batch_close(operation_id, Some(now));
            return None;
        }
        if conflicting || query || attempt.is_suspended() {
            return None;
        }
        let advice = o.handover()?;
        let term = advice.starting_term.get();
        let successor = term.checked_add(1)?;
        let target = advice.target_peer_id.get();
        if source == target
            || (observed_leader.0 == successor && observed_leader.1.is_some_and(|p| p.get() != target))
            || handover.is_some()
            || last_handover_term.is_some_and(|used| used >= advice.starting_term)
        {
            return None;
        }
        let index = endpoints.iter().position(|(peer, _)| *peer == target)?;
        let valid = match &reply.disposition {
            ReplyDisposition::Refused(BatchSubmitRefusal::NotReady) if current => {
                (observed == term
                    && leader.map(NonZeroU64::get) == Some(source)
                    && o.unavailable_reason() == Some(IngressUnavailableReason::LeaderNotReady))
                    || (observed == successor
                        && leader.is_none_or(|p| p.get() == target)
                        && o.unavailable_reason() == Some(IngressUnavailableReason::NotLeader))
            }
            ReplyDisposition::PendingKnown(entry)
            | ReplyDisposition::CommittedAwaitingApply(entry)
            | ReplyDisposition::Completed { entry, .. } => {
                observed == successor
                    && leader.map(NonZeroU64::get) == Some(target)
                    && *admitted == Some((operation_id, reply.binding.context.request_id, *entry))
                    && entry.term == advice.starting_term
            }
            _ => false,
        };
        let until = now.checked_add(*handover_window_millis).ok()?.min(*deadline);
        (valid && now < until).then_some((index, advice.starting_term, until))
    }

    fn start_handover(
        &mut self,
        operation_id: u64,
        target: usize,
        term: NonZeroU64,
        deadline: MonotonicMillis,
        now: MonotonicMillis,
    ) {
        let ClientState::Batch { slot, .. } = &mut self.state;
        if let Some(BatchSlot::Pending { last_handover_term, handover, attempt, .. }) = slot {
            if attempt.is_suspended() {
                return;
            }
            *last_handover_term = Some(term);
            *handover = Some((target, deadline, BatchNextExchange::Reconcile));
        }
        self.require_batch_close(operation_id, Some(now));
        if let ClientState::Batch { connection: BatchConnection::CloseRequired { retry, .. }, .. } = &mut self.state {
            *retry = Some((now, target));
        }
    }

    fn expire_handover(&mut self, now: MonotonicMillis) {
        let ClientState::Batch { slot, connection, retry_millis, endpoints, .. } = &mut self.state;
        let Some(BatchSlot::Pending { handover, .. }) = slot else { return };
        let Some((target, deadline, _)) = handover.as_ref() else { return };
        let target = *target;
        if now < *deadline {
            return;
        }
        *handover = None;
        match connection {
            BatchConnection::Connecting { operation_id, .. }
            | BatchConnection::Connected { operation_id, .. }
            | BatchConnection::Switching { operation_id, .. } => {
                let id = *operation_id;
                self.require_batch_close(id, Some(now));
            }
            BatchConnection::Disconnected { retry_at, peer } => {
                *peer = (target + 1) % endpoints.len();
                if let Ok(at) = now.checked_add(*retry_millis) {
                    *retry_at = at
                } else {
                    self.stop_batches(ClientError::InvalidTime { source: ClockReadError::OutOfRange })
                }
            }
            BatchConnection::Closing { retry: retry @ Some(_), .. }
            | BatchConnection::CloseRequired { retry: retry @ Some(_), .. } => {
                if let Ok(at) = now.checked_add(*retry_millis) {
                    *retry = Some((at, (target + 1) % endpoints.len()))
                } else {
                    self.stop_batches(ClientError::InvalidTime { source: ClockReadError::OutOfRange })
                }
            }
            _ => (),
        }
    }

    fn next_reconnect_at(&mut self, now: MonotonicMillis) -> Result<MonotonicMillis, ClockReadError> {
        let ClientState::Batch { slot, connection, retry_millis, idle_reconnect_millis, handover_retry_millis, .. } =
            &mut self.state;
        if matches!(slot, Some(BatchSlot::Pending { handover: Some(_), .. })) {
            return now.checked_add(*handover_retry_millis);
        }
        if matches!(slot, Some(BatchSlot::Pending { .. })) {
            return now.checked_add(*retry_millis);
        }
        // A stable transport also retires old idle failures without a business
        // reply. Immediate EOF must retain backoff; no extra polling is needed.
        if matches!(connection, BatchConnection::Connected { connected_at, .. }
            if now.get().checked_sub(connected_at.get()).is_some_and(|age| age >= (*retry_millis).max(5_000)))
        {
            *idle_reconnect_millis = *retry_millis;
        }
        let due = now.checked_add(*idle_reconnect_millis)?;
        *idle_reconnect_millis = idle_reconnect_millis.saturating_mul(2).min((*retry_millis).max(5_000));
        Ok(due)
    }

    fn observe_batch_route(&mut self, unavailable: u32) {
        let ClientState::Batch { unavailable_replies, unavailable_reply_limit, connection, .. } = &mut self.state;
        *unavailable_replies = unavailable;
        if unavailable >= *unavailable_reply_limit {
            if let BatchConnection::Connected { operation_id, peer, write, first_unanswered, .. } = *connection {
                *connection = BatchConnection::Switching { operation_id, peer, write, first_unanswered };
            }
        }
    }

    // Strict cyclic selection uses only the current endpoint, never a hint. It is
    // sealed into CloseRequired/Disconnected before the driver sees an effect.
    fn next_batch_peer(&self) -> usize {
        let ClientState::Batch { endpoints, connection, slot, .. } = &self.state;
        if let Some(BatchSlot::Pending { handover: Some((target, ..)), .. }) = slot {
            return *target;
        }
        match connection {
            BatchConnection::Connecting { peer, .. }
            | BatchConnection::Connected { peer, .. }
            | BatchConnection::Switching { peer, .. } => (peer + 1) % endpoints.len(),
            // A selected successor is already sealed into the retirement/backoff.
            BatchConnection::Disconnected { peer, .. }
            | BatchConnection::CloseRequired { retry: Some((_, peer)), .. }
            | BatchConnection::Closing { retry: Some((_, peer)), .. } => *peer,
            _ => 0, // No further connection is authorized in stopped states.
        }
    }

    fn finish_batch_switch(&mut self, now: MonotonicMillis) -> bool {
        let ClientState::Batch {
            connection: BatchConnection::Switching { operation_id, write: None, .. }, slot, ..
        } = &self.state
        else {
            return false;
        };
        if batch_has_accepted_exchange(slot) {
            return false;
        }
        // Planned switches and physical failures use the same retirement and
        // correlation retirement. Actual Closed is still required.
        self.require_batch_close(*operation_id, Some(now));
        true
    }
}

fn batch_has_accepted_exchange(slot: &Option<BatchSlot>) -> bool {
    slot.iter().any(|slot| {
        matches!(slot, BatchSlot::Pending { attempt, .. }
        if matches!(attempt, BatchAttempt::Awaiting { .. }) || attempt.query().is_some())
    })
}
