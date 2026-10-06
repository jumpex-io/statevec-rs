// Same owner and state: these routing facts cannot mint slot retry authority.
impl IngressClientOwner {
    fn observe_batch_route(&mut self, unavailable: u32) {
        let ClientState::Batch { unavailable_replies, unavailable_reply_limit, connection, .. } = &mut self.state;
        *unavailable_replies = unavailable;
        if unavailable >= *unavailable_reply_limit {
            if let BatchConnection::Connected { operation_id, peer, write } = *connection {
                *connection = BatchConnection::Switching { operation_id, peer, write };
            }
        }
    }

    // Strict cyclic selection uses only the current endpoint, never a hint. It is
    // sealed into CloseRequired/Disconnected before the driver sees an effect.
    fn next_batch_peer(&self) -> usize {
        let ClientState::Batch { endpoints, connection, .. } = &self.state;
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
