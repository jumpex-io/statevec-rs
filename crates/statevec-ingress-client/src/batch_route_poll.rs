const ROUTE_PROBE_LIMIT: u8 = 4;
// Each probe has its own socket and carries exactly one request. Keeping probe
// IDs off the batch counter preserves the primary's contiguous Submit range.
const ROUTE_REQUEST_ID: NonZeroU64 = NonZeroU64::MIN;

/// One optional read-only socket alongside the ordinary connection. Its
/// scheduling and resource returns are independent of the batch exchange.
#[derive(Debug)]
enum RouteProbe {
    Idle,
    Scheduled {
        peer: usize,
        at: MonotonicMillis,
    },
    Connecting {
        operation_id: u64,
        peer: usize,
        deadline: MonotonicMillis,
    },
    Ready {
        operation_id: u64,
        peer: usize,
        deadline: MonotonicMillis,
    },
    Awaiting {
        operation_id: u64,
        peer: usize,
        deadline: MonotonicMillis,
    },
    CloseRequired {
        operation_id: u64,
        next: Option<(usize, MonotonicMillis)>,
    },
    Closing {
        operation_id: u64,
        next: Option<(usize, MonotonicMillis)>,
    },
    /// Positive, correlated proof on this socket. Only the old primary's
    /// physical retirement can transfer it to the ordinary connection.
    Successor {
        operation_id: u64,
        peer: usize,
    },
}

impl RouteProbe {
    fn operation_id(&self) -> Option<u64> {
        match *self {
            Self::Connecting { operation_id, .. }
            | Self::Ready { operation_id, .. }
            | Self::Awaiting { operation_id, .. }
            | Self::CloseRequired { operation_id, .. }
            | Self::Closing { operation_id, .. }
            | Self::Successor { operation_id, .. } => Some(operation_id),
            Self::Idle | Self::Scheduled { .. } => None,
        }
    }

    fn visit(&self) -> Option<(usize, MonotonicMillis)> {
        match *self {
            Self::Connecting { peer, deadline, .. }
            | Self::Ready { peer, deadline, .. }
            | Self::Awaiting { peer, deadline, .. } => Some((peer, deadline)),
            _ => None,
        }
    }

    fn wake_at(&self, now: MonotonicMillis) -> Option<MonotonicMillis> {
        match *self {
            Self::Scheduled { at, .. } => Some(at),
            Self::Connecting { deadline, .. } | Self::Awaiting { deadline, .. } => Some(deadline),
            Self::Ready { .. } | Self::CloseRequired { .. } => Some(now),
            Self::Idle | Self::Closing { .. } | Self::Successor { .. } => None,
        }
    }

    fn has_pending_discovery(&self) -> bool {
        matches!(
            self,
            Self::Scheduled { .. }
                | Self::Connecting { .. }
                | Self::Ready { .. }
                | Self::Awaiting { .. }
                | Self::CloseRequired { next: Some(_), .. }
                | Self::Closing { next: Some(_), .. }
        )
    }

    fn valid_for(
        &self,
        primary: &BatchConnection,
        slot: &Option<BatchSlot>,
        endpoints: usize,
        next_connection_id: u64,
    ) -> bool {
        if matches!(self, Self::Idle) {
            return true;
        }
        let (original, peer) = match *primary {
            BatchConnection::Connected { operation_id, peer, .. }
            | BatchConnection::Switching { operation_id, peer, .. } => (Some(operation_id), Some(peer)),
            BatchConnection::CloseRequired { operation_id, .. }
            | BatchConnection::Closing { operation_id, .. }
            | BatchConnection::Connecting { operation_id, .. } => (Some(operation_id), None),
            _ => (None, None),
        };
        if self.operation_id().is_some_and(|id| id == 0 || id >= next_connection_id || Some(id) == original) {
            return false;
        }
        let target = match *self {
            Self::Scheduled { peer, .. }
            | Self::Connecting { peer, .. }
            | Self::Ready { peer, .. }
            | Self::Awaiting { peer, .. }
            | Self::CloseRequired { next: Some((peer, _)), .. }
            | Self::Closing { next: Some((peer, _)), .. } => Some(peer),
            Self::Successor { peer, .. } => {
                return peer < endpoints
                    && matches!(
                        primary,
                        BatchConnection::CloseRequired { retry: Some((_, target)), .. }
                            | BatchConnection::Closing { retry: Some((_, target)), .. }
                            if peer == *target
                    )
                    && matches!(slot, Some(BatchSlot::Pending {
                        probe_budget: 0, compatibility_exchange: BatchNextExchange::Reconcile, ..
                    }));
            }
            _ => None,
        };
        target.is_none_or(|target| {
            peer.is_some_and(|origin| origin != target)
                && target < endpoints
                && batch_can_probe(slot)
                && matches!(slot, Some(BatchSlot::Pending { probe_budget, .. })
                    if (*probe_budget > 0 || self.visit().is_some())
                        && (*probe_budget < ROUTE_PROBE_LIMIT || matches!(self, Self::Scheduled { .. })))
        })
    }
}

fn batch_can_probe(slot: &Option<BatchSlot>) -> bool {
    matches!(slot, Some(BatchSlot::Pending {
        attempt: BatchAttempt::Awaiting { .. } | BatchAttempt::Querying { .. },
        handover: None,
        ..
    }))
}

impl BatchClient {
    fn arm_silence_probe(&mut self, socket: u64, request: NonZeroU64, now: MonotonicMillis) {
        let ClientState::Batch { endpoints, routing_poll_millis, slot, .. } = &mut self.state;
        if endpoints.len() < 2 || !batch_can_probe(slot) {
            return;
        }
        if let Some(BatchSlot::Pending { attempt, probe_budget: ROUTE_PROBE_LIMIT, .. }) = slot
            && let BatchAttempt::Awaiting { operation_id, request_id, deadline, probe_after, .. }
                | BatchAttempt::Querying { operation_id, request_id, deadline, probe_after, .. } = attempt
            && *operation_id == socket
            && *request_id == request
        {
            *probe_after = now.checked_add(*routing_poll_millis).ok().filter(|candidate| candidate < deadline);
        }
    }

    fn poll_for_silence(&mut self, now: MonotonicMillis, partial: Option<(u64, IncompleteFrame)>) {
        let ClientState::Batch { connection, endpoints, slot, route_probe, .. } = &mut self.state;
        let eligible = batch_can_probe(slot);
        let (BatchConnection::Connected { operation_id, peer, .. }
            | BatchConnection::Switching { operation_id, peer, .. }) = *connection
        else {
            return;
        };
        if partial.is_some_and(|(id, _)| id == operation_id) {
            if let Some(BatchSlot::Pending { attempt, .. }) = slot
                && let BatchAttempt::Awaiting { probe_after, .. } | BatchAttempt::Querying { probe_after, .. } = attempt
            {
                *probe_after = None;
            }
            self.cancel_route_probe();
            return;
        }
        if eligible
            && matches!(route_probe, RouteProbe::Idle)
            && let Some(BatchSlot::Pending { attempt, probe_budget: ROUTE_PROBE_LIMIT, .. }) = slot
            && let BatchAttempt::Awaiting { probe_after, .. } | BatchAttempt::Querying { probe_after, .. } = attempt
            && probe_after.is_some_and(|deadline| now >= deadline)
        {
            *probe_after = None;
            *route_probe = RouteProbe::Scheduled { peer: (peer + 1) % endpoints.len(), at: now };
        }
    }

    fn drive_route_probe(&mut self, now: MonotonicMillis) -> Option<ClientResult> {
        let ClientState::Batch {
            slot, route_probe, context, endpoints, next_connection_id, routing_poll_millis, maximum_frame_bytes,
            route_visits, ..
        } = &mut self.state;
        if route_probe.has_pending_discovery() && !batch_can_probe(slot) {
            self.cancel_route_probe();
            return Some(self.batch_effect());
        }
        if let Some((peer, deadline)) = route_probe.visit()
            && now >= deadline
        {
            self.retire_route_probe(peer, now);
            return Some(self.batch_effect());
        }
        match *route_probe {
            RouteProbe::CloseRequired { .. } => Some(self.batch_effect()),
            RouteProbe::Scheduled { peer, at } if now >= at => {
                let Some(BatchSlot::Pending { probe_budget, .. }) = slot else { return None };
                // This optional visit may be omitted if a bounded deadline or
                // connection ID cannot be represented. The original is live.
                let Some(deadline) = now.checked_add(*routing_poll_millis).ok() else {
                    self.cancel_route_probe();
                    return None;
                };
                let Ok(next) = adapter::next_batch_connection_id(*next_connection_id) else {
                    self.cancel_route_probe();
                    return None;
                };
                *probe_budget -= 1;
                *route_visits = route_visits.saturating_add(1);
                let operation_id = *next_connection_id;
                *next_connection_id = next;
                *route_probe = RouteProbe::Connecting { operation_id, peer, deadline };
                let (peer_id, address) = endpoints[peer];
                Some(ClientResult::Connect { operation_id, peer_id, address })
            }
            RouteProbe::Ready { operation_id, peer, deadline } => {
                let context = RequestContext { request_id: ROUTE_REQUEST_ID, ..*context };
                let bytes = match ingress_api::stream_batch::encode_route_request(&context, *maximum_frame_bytes as usize) {
                    Ok(bytes) => bytes,
                    Err(source) => {
                        self.cancel_route_probe();
                        return Some(ClientResult::ConnectionFault { operation_id, cause: ClientError::BatchFrame { source } });
                    }
                };
                *route_probe = RouteProbe::Awaiting { operation_id, peer, deadline };
                Some(ClientResult::Write { operation_id, request_id: ROUTE_REQUEST_ID, bytes })
            }
            _ => None,
        }
    }

    fn consume_route_event(
        &mut self, event: ClientEvent, sample: Result<MonotonicMillis, ClockReadError>,
    ) -> ClientResult {
        let now = self.checked_batch_time(sample);
        if let Ok(now) = now {
            self.expire_batches(now);
        }
        // Returned resources remain returned even if this clock sample failed.
        match event {
            ClientEvent::Closed { .. } | ClientEvent::ClosedUnwritten { .. } => {
                let ClientState::Batch { route_probe, .. } = &mut self.state;
                if let RouteProbe::Closing { next, .. } = *route_probe {
                    *route_probe = next.map_or(RouteProbe::Idle, |(peer, at)| RouteProbe::Scheduled { peer, at });
                }
                return match now {
                    Ok(_) => self.batch_effect(),
                    Err(cause) => ClientResult::ClockFailed { cause },
                };
            }
            ClientEvent::Connected { operation_id, result: Err(source) } => {
                let ClientState::Batch { route_probe, .. } = &self.state;
                let next = match *route_probe {
                    RouteProbe::Connecting { peer, deadline, .. } =>
                        self.next_probe(peer, deadline.max(now.unwrap_or(deadline)), None),
                    RouteProbe::CloseRequired { next, .. } | RouteProbe::Closing { next, .. } => next,
                    _ => return self.batch_effect(),
                };
                let ClientState::Batch { route_probe, .. } = &mut self.state;
                *route_probe = next.map_or(RouteProbe::Idle, |(peer, at)| RouteProbe::Scheduled { peer, at });
                return ClientResult::ConnectionFailed { operation_id, source, next_wake: self.batch_deadline() };
            }
            _ => (),
        }
        let now = match now {
            Ok(now) => now,
            Err(cause) => return ClientResult::ClockFailed { cause },
        };
        if let Some(effect) = self.expire_batch_exchange(now, None) {
            return effect;
        }
        if let Err(cause) = validate_batch_state(&self.state, None, None) {
            return ClientResult::DriveRejected { event, cause };
        }
        let ClientState::Batch { route_probe, maximum_frame_bytes, .. } = &mut self.state;
        if let Some((peer, deadline)) = route_probe.visit()
            && now >= deadline
        {
            self.retire_route_probe(peer, now);
            return self.batch_effect();
        }
        let maximum = *maximum_frame_bytes as usize;
        match event {
            ClientEvent::Connected { operation_id, result: Ok(()) } => {
                if let RouteProbe::Connecting { peer, deadline, .. } = *route_probe {
                    *route_probe = RouteProbe::Ready { operation_id, peer, deadline };
                }
            }
            ClientEvent::VerifiedRead { frame, .. } if matches!(route_probe, RouteProbe::Awaiting { .. }) => {
                return self.consume_route_reply(now, frame.decode_route_reply(maximum));
            }
            ClientEvent::Read { result: Ok(bytes), .. } if matches!(route_probe, RouteProbe::Awaiting { .. }) => {
                return self.consume_route_reply(now, ingress_api::stream_batch::decode_route_reply(&bytes, maximum));
            }
            ClientEvent::Read { operation_id, result: Err(source) } => {
                return self.fail_route_probe(operation_id, now, ClientError::BatchRead { source });
            }
            ClientEvent::Written { operation_id, request_id: ROUTE_REQUEST_ID, result: Err(source) }
            | ClientEvent::WriteUnwritten { operation_id, request_id: ROUTE_REQUEST_ID, source } => {
                return self.fail_route_probe(operation_id, now, ClientError::BatchWrite { source });
            }
            _ => (),
        }
        self.batch_effect()
    }

    fn consume_route_reply(
        &mut self,
        now: MonotonicMillis,
        decoded: Result<(RequestContext, crate::RouteObservation), ingress_api::stream_batch::FrameError>,
    ) -> ClientResult {
        let ClientState::Batch {
            route_probe: RouteProbe::Awaiting { operation_id, peer, deadline },
            context, endpoints, observed_leader, route_observation, connection, ..
        } = &mut self.state else {
            return self.batch_effect();
        };
        let operation_id = *operation_id;
        let (observed_context, observation) = match decoded {
            Ok(answer) => answer,
            Err(source) => return self.fail_route_probe(operation_id, now, ClientError::BatchFrame { source }),
        };
        if observed_context != (RequestContext { request_id: ROUTE_REQUEST_ID, ..*context })
            || observation.peer_id.get() != endpoints[*peer].0
        {
            return self.batch_effect();
        }
        let (BatchConnection::Connected { operation_id: original, peer: origin, .. }
            | BatchConnection::Switching { operation_id: original, peer: origin, .. }) = *connection
        else {
            self.cancel_route_probe();
            return self.batch_effect();
        };
        let current = observation.term >= observed_leader.0
            && !(observation.term == observed_leader.0
                && observed_leader.1.zip(observation.leader).is_some_and(|(known, seen)| known != seen));
        let successor = current && observation.writable && *peer != origin;
        let new_hint = observation.term > observed_leader.0 || observed_leader.1.is_none();
        let suggested = if current && new_hint {
            observation.leader.and_then(|leader| endpoints.iter().position(|(id, _)| *id == leader.get()))
                .filter(|target| *target != *peer && *target != origin)
        } else {
            None
        };
        let poll_here = current && observation.leader == Some(observation.peer_id);
        if current {
            if new_hint {
                *observed_leader = (observation.term, observation.leader);
            }
            if route_observation.as_ref().is_none_or(|last| last.applied_index <= observation.applied_index) {
                *route_observation = Some(observation);
            }
        }
        let (peer, deadline) = (*peer, *deadline);
        if successor {
            self.close_batch_for_retry(original, now, peer);
            let ClientState::Batch { route_probe, slot, route_successors, .. } = &mut self.state;
            *route_successors = route_successors.saturating_add(1);
            *route_probe = RouteProbe::Successor { operation_id, peer };
            if let Some(BatchSlot::Pending { compatibility_exchange, .. }) = slot {
                *compatibility_exchange = BatchNextExchange::Reconcile;
            }
        } else {
            let at = if suggested.is_some() { now } else { deadline };
            let next = self.next_probe(peer, at, suggested.or(poll_here.then_some(peer)));
            let ClientState::Batch { route_probe, .. } = &mut self.state;
            *route_probe = RouteProbe::CloseRequired { operation_id, next };
        }
        self.batch_effect()
    }

    fn next_probe(
        &self, peer: usize, at: MonotonicMillis, target: Option<usize>,
    ) -> Option<(usize, MonotonicMillis)> {
        let ClientState::Batch { connection, endpoints, slot, .. } = &self.state;
        if !matches!(slot, Some(BatchSlot::Pending { probe_budget, .. }) if *probe_budget > 0) {
            return None;
        }
        let (BatchConnection::Connected { peer: origin, .. } | BatchConnection::Switching { peer: origin, .. }) = *connection
        else {
            return None;
        };
        let next = (peer + 1) % endpoints.len();
        let next = target.unwrap_or_else(|| if next == origin { (next + 1) % endpoints.len() } else { next });
        Some((next, at))
    }

    fn retire_route_probe(&mut self, peer: usize, at: MonotonicMillis) {
        let next = self.next_probe(peer, at, None);
        let ClientState::Batch { route_probe, .. } = &mut self.state;
        if let Some(operation_id) = route_probe.operation_id() {
            *route_probe = RouteProbe::CloseRequired { operation_id, next };
        }
    }

    fn fail_route_probe(&mut self, operation_id: u64, now: MonotonicMillis, cause: ClientError) -> ClientResult {
        let ClientState::Batch { route_probe, .. } = &self.state;
        if let Some((peer, deadline)) = route_probe.visit() {
            self.retire_route_probe(peer, now.max(deadline));
        } else if matches!(route_probe, RouteProbe::Successor { .. }) {
            self.cancel_route_probe();
        }
        ClientResult::ConnectionFault { operation_id, cause }
    }

    fn cancel_route_probe(&mut self) {
        let ClientState::Batch { route_probe, slot, .. } = &mut self.state;
        if route_probe.has_pending_discovery()
            && let Some(BatchSlot::Pending { probe_budget, .. }) = slot
        {
            *probe_budget = 0;
        }
        *route_probe = match *route_probe {
            RouteProbe::Closing { operation_id, .. } => RouteProbe::Closing { operation_id, next: None },
            _ => route_probe.operation_id().map_or(RouteProbe::Idle, |operation_id| {
                RouteProbe::CloseRequired { operation_id, next: None }
            }),
        };
    }

    fn route_close_effect(&mut self) -> Option<ClientResult> {
        let ClientState::Batch { route_probe, .. } = &mut self.state;
        if let RouteProbe::CloseRequired { operation_id, next } = *route_probe {
            *route_probe = RouteProbe::Closing { operation_id, next };
            return Some(ClientResult::Close { operation_id });
        }
        None
    }
}
