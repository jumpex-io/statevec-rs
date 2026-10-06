//! Routing traces use the actual owner, codec and injected clock. Scripted peer
//! replies prove client policy, not a server or physical TCP qualification.
use super::*;
use ingress_api::stream_batch::IngressUnavailableReason;
use test_case::test_case;

fn four_peer_client(clock: &Clock) -> BatchClient {
    let peers = (1..=4)
        .map(|id| (id, SocketAddr::from(([127, 0, 0, 1], 12000 + id as u16))))
        .collect();
    construct(clock, idle_start(), peers, (RETRY, OPERATION, LIFETIME)).unwrap()
}

fn observed(mut response: Reply, term: u64, hint: Option<u64>, reason: Option<IngressUnavailableReason>) -> Reply {
    let previous = response.observation.unwrap();
    response.observation = Some(
        ReplyObservation::new(
            term,
            previous.applied_index(),
            if matches!(response.disposition, ReplyDisposition::RetryableNext) {
                term
            } else {
                previous.applied_term()
            },
            hint.and_then(NonZeroU64::new),
            reason,
        )
        .unwrap(),
    );
    response
}

#[track_caller]
pub(super) fn close_and_connect(client: &mut BatchClient, clock: &Clock, connection: u64, peer: u64) -> u64 {
    let now = clock.sample().unwrap().get();
    let close = client.drive(ClientEvent::Drive);
    assert!(
        matches!(close, ClientResult::Close { operation_id } if operation_id == connection),
        "bounded unavailability must make the real connection retire: {close:?}"
    );
    assert!(
        matches!(client.drive(ClientEvent::Drive), ClientResult::Waiting { .. }),
        "planned selection must await physical Closed"
    );
    assert!(matches!(client.drive(ClientEvent::Closed { operation_id: connection }), ClientResult::Waiting { .. }));
    waiting_until(client, now + RETRY);
    clock.set(now + RETRY - 1);
    waiting_until(client, now + RETRY);
    clock.set(now + RETRY);
    let effect = client.drive(ClientEvent::Drive);
    let ClientResult::Connect { operation_id, peer_id, address } = effect else {
        panic!("next endpoint requires a real connection effect: {effect:?}")
    };
    assert_eq!(peer_id, peer, "cyclic rotation must select the next configured peer");
    assert_eq!(address.port(), 12000 + peer as u16);
    assert!(operation_id > connection, "connection identity is never reused");
    assert!(matches!(
        client.drive(ClientEvent::Connected { operation_id, result: Ok(()) }),
        ClientResult::Waiting { .. }
    ));
    operation_id
}

#[test_case(None; "missing diagnosis")]
#[test_case(Some(IngressUnavailableReason::LeaderNotReady); "leader not ready")]
#[test_case(Some(IngressUnavailableReason::SnapshotInstalling); "snapshot installing")]
#[test_case(Some(IngressUnavailableReason::ReadyBacklog); "ready backlog")]
#[test_case(Some(IngressUnavailableReason::EngineOccupied); "engine occupied")]
#[test_case(Some(IngressUnavailableReason::HistoryScanLimited); "history scan limited")]
fn timely_unavailable_queries_rotate_and_settle_the_original(reason: Option<IngressUnavailableReason>) {
    let clock = Clock::new();
    let mut client = client(&clock);
    let (connection, submit) = committed(&mut client);
    for count in 1..=UNAVAILABLE_LIMIT {
        clock.set(INITIAL_TIME + RETRY * u64::from(count));
        let (_, query) = take_query(&mut client);
        write_return(&mut client, connection, &query);
        receive(&mut client, connection, observed(reply(&query, ReplyDisposition::Unknown), 5, None, reason));
        assert!(client.poll_event().is_none(), "temporary unavailability is not a batch result");
    }
    let successor = close_and_connect(&mut client, &clock, connection, 2);
    let (routed_connection, query) = take_query(&mut client);
    assert_eq!(routed_connection, successor);
    assert_eq!(wire(query.batch()), wire(submit.batch()));
    assert!(query.context().request_id > submit.context().request_id);
    write_return(&mut client, successor, &query);
    receive(&mut client, successor, exact(&query));
    assert!(
        matches!(client.poll_event(), Some(BatchEvent::Terminal(BatchTerminal::ExactAppliedResultUnavailable { handle, .. }))
        if handle.context == *submit.context() && handle.intent == submit.batch().commitment_v1())
    );
    assert!(client.poll_event().is_none());
}

#[test_case(BatchSubmitRefusal::InternalFailure; "failed node")]
#[test_case(BatchSubmitRefusal::ShuttingDown; "draining node")]
fn query_node_failure_rotates_without_resubmitting_committed_work(reason: BatchSubmitRefusal) {
    let clock = Clock::new();
    let mut client = client(&clock);
    let (connection, submit) = committed(&mut client);
    clock.set(INITIAL_TIME + RETRY);
    let (_, query) = take_query(&mut client);
    write_return(&mut client, connection, &query);
    let mut refusal = reply(&query, ReplyDisposition::Refused(reason));
    refusal.observation = None;
    receive(&mut client, connection, refusal);
    let successor = close_and_connect(&mut client, &clock, connection, 2);
    let (_, replacement) = take_query(&mut client);
    write_return(&mut client, successor, &replacement);
    receive(&mut client, successor, exact(&replacement));
    assert!(
        matches!(client.poll_event(), Some(BatchEvent::Terminal(BatchTerminal::ExactAppliedResultUnavailable { handle, .. }))
        if handle.context == *submit.context())
    );
}

#[test]
fn draining_node_rejecting_retransmission_recovers_with_same_intent() {
    let clock = Clock::new();
    let mut client = client(&clock);
    let (connection, submit) = admitted(&mut client);
    clock.set(INITIAL_TIME + RETRY);
    let (_, retransmit) = take_write(&mut client);
    assert_eq!(wire(retransmit.batch()), wire(submit.batch()));
    assert_ne!(retransmit.context().request_id, submit.context().request_id);
    write_return(&mut client, connection, &retransmit);
    let mut refusal = reply(&retransmit, ReplyDisposition::Refused(BatchSubmitRefusal::ShuttingDown));
    refusal.observation = None; // Typed node unavailability does not need a hint.
    receive(&mut client, connection, refusal);
    assert!(client.poll_event().is_none(), "draining a retry peer cannot suspend or settle the uncertain batch");
    let successor = close_and_connect(&mut client, &clock, connection, 2);
    let (actual, recovery) = take_write(&mut client);
    assert_eq!(actual, successor);
    assert_eq!(wire(recovery.batch()), wire(submit.batch()));
    write_return(&mut client, successor, &recovery);
    receive(&mut client, successor, exact(&recovery));
    assert!(matches!(
        client.poll_event(),
        Some(BatchEvent::Terminal(BatchTerminal::ExactAppliedResultUnavailable { handle, .. }))
            if handle.context == *submit.context()
    ));
    assert!(client.poll_event().is_none());
}

#[test]
fn planned_rotation_preserves_the_query_write_until_physical_return() {
    let clock = Clock::new();
    let mut client = client(&clock);
    let (connection, first) = committed(&mut client);
    clock.set(INITIAL_TIME + RETRY);
    let (_, query) = take_query(&mut client); // Written still owed.
    receive(&mut client, connection, reply(&query, ReplyDisposition::Refused(BatchSubmitRefusal::InternalFailure)));
    for interval in 2..=5 {
        clock.set(INITIAL_TIME + RETRY * interval);
        assert!(
            matches!(client.drive(ClientEvent::Drive), ClientResult::Waiting { wake_at: Some(due) } if due.get() == INITIAL_TIME + RETRY + OPERATION),
            "planned rotation must wait for the query write return"
        );
    }
    write_return(&mut client, connection, &query);
    let successor = close_and_connect(&mut client, &clock, connection, 2);
    let (_, replacement) = take_query(&mut client);
    assert_eq!(wire(replacement.batch()), wire(first.batch()));
    write_return(&mut client, successor, &replacement);
    receive(&mut client, successor, exact(&replacement));
    assert!(matches!(
        client.poll_event(),
        Some(BatchEvent::Terminal(BatchTerminal::ExactAppliedResultUnavailable { .. }))
    ));
}

#[test]
fn conflicting_hints_cannot_interrupt_cyclic_visits_or_replenish_their_budget() {
    let clock = Clock::new();
    let mut client = four_peer_client(&clock);
    let (mut connection, submit) = committed(&mut client);
    for count in 1..=UNAVAILABLE_LIMIT {
        clock.set(INITIAL_TIME + RETRY * u64::from(count));
        let (_, query) = take_query(&mut client);
        write_return(&mut client, connection, &query);
        receive(
            &mut client,
            connection,
            observed(reply(&query, ReplyDisposition::Unknown), 9, Some(3), Some(IngressUnavailableReason::NotLeader)),
        );
    }
    // Each owner-selected visit gets a full no-progress budget, including the
    // return to peer 1 in the next cycle. Repeated cross-hints cannot skip peers.
    for (peer, hint) in [(2, 1), (3, 2), (4, 1), (1, 4)] {
        connection = close_and_connect(&mut client, &clock, connection, peer);
        for count in 1..=UNAVAILABLE_LIMIT {
            let (_, query) = take_query(&mut client);
            assert_eq!(wire(query.batch()), wire(submit.batch()));
            write_return(&mut client, connection, &query);
            receive(
                &mut client,
                connection,
                observed(
                    reply(&query, ReplyDisposition::Unknown),
                    10 + peer,
                    Some(hint),
                    Some(IngressUnavailableReason::NotLeader),
                ),
            );
            if count < UNAVAILABLE_LIMIT {
                let retry_at = clock.sample().unwrap().get() + RETRY;
                assert!(
                    matches!(client.drive(ClientEvent::Drive), ClientResult::Waiting { wake_at: Some(due) }
                        if due.get() == retry_at),
                    "each selected endpoint visit must receive its own no-progress budget"
                );
                // Duplicate physical success cannot replenish this visit.
                assert!(matches!(
                    client.drive(ClientEvent::Connected { operation_id: connection, result: Ok(()) }),
                    ClientResult::Waiting { .. }
                ));
                clock.set(retry_at);
            }
        }
    }
    assert!(matches!(client.status(), ClientStatus::Batch { occupied: 1, .. }));
}

#[test_case(8, Some(4); "older hint")]
#[test_case(9, Some(4); "equal term conflicting hint")]
#[test_case(99, Some(99); "unknown peer at higher term")]
#[test_case(99, None; "no invented hint from high term")]
fn all_hint_terms_are_ignored_without_resetting_current_reply_progress(term: u64, hint: Option<u64>) {
    let clock = Clock::new();
    let mut client = four_peer_client(&clock);
    let (connection, _) = committed(&mut client);
    clock.set(INITIAL_TIME + RETRY);
    let (_, first) = take_query(&mut client);
    write_return(&mut client, connection, &first);
    receive(&mut client, connection, observed(reply(&first, ReplyDisposition::Unknown), 9, Some(3), None));
    for count in 2..=3 {
        clock.set(INITIAL_TIME + RETRY * count);
        let (_, query) = take_query(&mut client);
        write_return(&mut client, connection, &query);
        // Even a well-formed high-term hint on a retired operation has no
        // routing authority and cannot reset the unavailable streak.
        receive(
            &mut client,
            connection,
            observed(reply(&first, ReplyDisposition::PendingKnown(entry())), 999, Some(4), None),
        );
        receive(&mut client, connection, observed(reply(&query, ReplyDisposition::Unknown), term, hint, None));
    }
    close_and_connect(&mut client, &clock, connection, 2);
}

#[test]
fn qualified_progress_resets_only_the_current_visits_streak() {
    let clock = Clock::new();
    let mut client = four_peer_client(&clock);
    let (connection, _) = admitted(&mut client);
    clock.set(INITIAL_TIME + RETRY);
    let (_, retry) = take_write(&mut client);
    write_return(&mut client, connection, &retry);
    receive(
        &mut client,
        connection,
        observed(
            reply(&retry, ReplyDisposition::Refused(BatchSubmitRefusal::InternalFailure)),
            6,
            Some(3),
            Some(IngressUnavailableReason::Failed),
        ),
    );
    let successor = close_and_connect(&mut client, &clock, connection, 2);
    let (_, retry) = take_write(&mut client);
    write_return(&mut client, successor, &retry);
    receive(&mut client, successor, reply(&retry, ReplyDisposition::CommittedAwaitingApply(entry())));
    // First confirmation of commitment resets the streak, not a repeat of an
    // already known entry. All three new Unknown replies are now
    // required. A newer hint pointing back to peer 1 cannot skip peer 3.
    for count in 1..=3 {
        clock.set(INITIAL_TIME + RETRY * (2 + count));
        let (_, query) = take_query(&mut client);
        write_return(&mut client, successor, &query);
        receive(&mut client, successor, observed(reply(&query, ReplyDisposition::Unknown), 7, Some(1), None));
    }
    close_and_connect(&mut client, &clock, successor, 3);
}

#[test]
fn stale_weak_progress_does_not_reset_the_unavailable_streak() {
    let clock = Clock::new();
    let mut client = client(&clock);
    let (connection, submit) = committed(&mut client);
    for count in 1..=3 {
        clock.set(INITIAL_TIME + RETRY * count);
        let (_, query) = take_query(&mut client);
        write_return(&mut client, connection, &query);
        receive(&mut client, connection, at_term(reply(&submit, ReplyDisposition::IngressAdmitted(entry())), 4));
        receive(&mut client, connection, reply(&query, ReplyDisposition::Unknown));
    }
    close_and_connect(&mut client, &clock, connection, 2);
}

#[test_case(BatchSubmitRefusal::Busy; "busy")]
#[test_case(BatchSubmitRefusal::NotReady; "not ready")]
fn refusals_count_exchanges_not_duplicate_frames_and_rotation_preserves_intent(reason: BatchSubmitRefusal) {
    let clock = Clock::new();
    let mut client = client(&clock);
    let connection = connect(&mut client);
    client.try_submit(batch(1)).unwrap();
    let (_, submit) = take_write(&mut client);
    write_return(&mut client, connection, &submit);
    for _ in 0..3 {
        receive(&mut client, connection, reply(&submit, ReplyDisposition::Refused(reason)));
    }
    waiting_until(&mut client, INITIAL_TIME + RETRY);
    clock.set(INITIAL_TIME + RETRY);
    let (_, retry) = take_write(&mut client);
    assert_eq!(wire(retry.batch()), wire(submit.batch()));
    assert_ne!(retry.context().request_id, submit.context().request_id);
    write_return(&mut client, connection, &retry);
    receive(&mut client, connection, reply(&retry, ReplyDisposition::Refused(reason)));
    clock.set(INITIAL_TIME + RETRY * 2);
    let (_, query) = take_write(&mut client);
    write_return(&mut client, connection, &query);
    receive(&mut client, connection, reply(&query, ReplyDisposition::Refused(reason)));
    let successor = close_and_connect(&mut client, &clock, connection, 2);
    let (_, query) = take_write(&mut client);
    assert_eq!(wire(query.batch()), wire(submit.batch()));
    write_return(&mut client, successor, &query);
    receive(&mut client, successor, exact(&query));
    assert!(matches!(
        client.poll_event(),
        Some(BatchEvent::Terminal(BatchTerminal::ExactAppliedResultUnavailable { .. }))
    ));
}

#[derive(Debug, Clone, Copy)]
enum Outstanding {
    QueryWrite,
    WriteAfterTerminal,
}

#[test_case(Outstanding::QueryWrite; "query reply received before written")]
#[test_case(Outstanding::WriteAfterTerminal; "terminal consumed before partial write return")]
fn planned_switch_still_obeys_the_original_exchange_deadline(outstanding: Outstanding) {
    let clock = Clock::new();
    let mut client = client(&clock);
    let (connection, submit) = committed(&mut client);
    clock.set(INITIAL_TIME + RETRY);
    let (_, query) = take_query(&mut client);
    receive(&mut client, connection, reply(&query, ReplyDisposition::Refused(BatchSubmitRefusal::InternalFailure)));
    if matches!(outstanding, Outstanding::WriteAfterTerminal) {
        receive(&mut client, connection, completed(&submit));
        assert!(matches!(client.poll_event(), Some(BatchEvent::Terminal(BatchTerminal::Applied { .. }))));
    }
    let due = INITIAL_TIME + RETRY + OPERATION;
    clock.set(due - 1);
    let waiting = client.drive(ClientEvent::Drive);
    assert!(
        matches!(waiting, ClientResult::Waiting { wake_at: Some(wake) } if wake.get() == due),
        "planned switch must retain accepted exchanges until their original deadline: {waiting:?}"
    );
    clock.set(due);
    close_and_connect(&mut client, &clock, connection, 2);
    assert!(validate_batch_state(&client.state, None, None).is_ok());
}

#[test]
fn routing_terms_never_authorize_retries_or_filter_exact_old_term_history() {
    let clock = Clock::new();
    let mut client = client(&clock);
    let (connection, submit) = committed(&mut client);
    for count in 1..=3 {
        clock.set(INITIAL_TIME + RETRY * count);
        let (_, query) = take_query(&mut client);
        write_return(&mut client, connection, &query);
        receive(&mut client, connection, observed(reply(&query, ReplyDisposition::Unknown), 999, Some(2), None));
    }
    let successor = close_and_connect(&mut client, &clock, connection, 2);
    let (_, query) = take_query(&mut client);
    write_return(&mut client, successor, &query);
    receive(&mut client, successor, at_term(exact(&query), 6));
    assert!(
        matches!(client.poll_event(), Some(BatchEvent::Terminal(BatchTerminal::ExactAppliedResultUnavailable { handle, .. })) if handle.context == *submit.context()),
        "routing terms must not filter exact older-term history"
    );
}

#[test_case(1; "one configured peer")]
#[test_case(32; "maximum configured endpoints")]
fn failed_connections_visit_each_peer_once_with_positive_retry_intervals(count: u64) {
    let clock = Clock::new();
    let peers = (1..=count)
        .map(|id| (id, SocketAddr::from(([127, 0, 0, 1], 12000 + id as u16))))
        .collect();
    let mut client = construct(&clock, idle_start(), peers, (RETRY, OPERATION, LIFETIME)).unwrap();
    for visit in 0..count + 2 {
        let now = INITIAL_TIME + RETRY * visit;
        clock.set(now);
        let effect = client.drive(ClientEvent::Drive);
        let ClientResult::Connect { operation_id, peer_id, .. } = effect else {
            panic!("scheduled Connect: {effect:?}")
        };
        assert_eq!(peer_id, visit % count + 1);
        assert!(matches!(client.drive(ClientEvent::Connected {
            operation_id, result: Err(std::io::ErrorKind::ConnectionRefused.into()),
        }), ClientResult::ConnectionFailed { next_wake: Some(due), .. } if due.get() == now + RETRY));
        waiting_until(&mut client, now + RETRY);
        clock.set(now + RETRY - 1);
        waiting_until(&mut client, now + RETRY);
    }
}

#[derive(Debug, Clone, Copy)]
enum SwitchExit {
    ReadFailure,
    WriteFailure,
    Stop,
    Lifetime,
}

#[test_case(SwitchExit::ReadFailure; "read failure during planned drain")]
#[test_case(SwitchExit::WriteFailure; "write failure during planned drain")]
#[test_case(SwitchExit::Stop; "explicit stop during planned drain")]
#[test_case(SwitchExit::Lifetime; "total lifetime during planned drain")]
fn switching_keeps_retirement_and_session_termination_obligations(exit: SwitchExit) {
    let clock = Clock::new();
    let mut client = client(&clock);
    let (connection, original) = committed(&mut client);
    // Start the current query close to the original total deadline; its own
    // operation deadline extends beyond it, and must not refresh that lifetime.
    clock.set(INITIAL_TIME + LIFETIME - RETRY * 2);
    let (_, query) = take_query(&mut client);
    receive(&mut client, connection, reply(&query, ReplyDisposition::Refused(BatchSubmitRefusal::InternalFailure)));
    waiting_until(&mut client, INITIAL_TIME + LIFETIME);
    let close = match exit {
        SwitchExit::ReadFailure | SwitchExit::WriteFailure => {
            let event = match exit {
                SwitchExit::ReadFailure => ClientEvent::Read {
                    operation_id: connection,
                    result: Err(stream_batch::ReadFailure::Io { source: std::io::ErrorKind::ConnectionReset.into() }),
                },
                _ => ClientEvent::Written {
                    operation_id: connection,
                    request_id: query.context().request_id,
                    result: Err(std::io::ErrorKind::BrokenPipe.into()),
                },
            };
            assert!(matches!(client.drive(event), ClientResult::ConnectionFault { .. }));
            client.drive(ClientEvent::Drive)
        }
        SwitchExit::Stop => client.drive(ClientEvent::Stop),
        SwitchExit::Lifetime => {
            clock.set(INITIAL_TIME + LIFETIME);
            client.drive(ClientEvent::Drive)
        }
    };
    assert!(matches!(close, ClientResult::Close { operation_id } if operation_id == connection));
    assert!(
        matches!(
            client.drive(ClientEvent::Written {
                operation_id: connection,
                request_id: query.context().request_id,
                result: Ok(()),
            }),
            ClientResult::Waiting { .. }
        ),
        "late buffer return cannot issue another Close"
    );
    assert!(matches!(client.drive(ClientEvent::Closed { operation_id: connection }), ClientResult::Waiting { .. }));
    match exit {
        SwitchExit::ReadFailure | SwitchExit::WriteFailure => {
            clock.set(INITIAL_TIME + LIFETIME - RETRY);
            let ClientResult::Connect { operation_id, peer_id: 2, .. } = client.drive(ClientEvent::Drive) else {
                panic!("failed planned drain still schedules a successor")
            };
            assert!(matches!(
                client.drive(ClientEvent::Connected { operation_id, result: Ok(()) }),
                ClientResult::Waiting { .. }
            ));
            let (_, replacement) = take_query(&mut client);
            assert_eq!(wire(replacement.batch()), wire(original.batch()));
            write_return(&mut client, operation_id, &replacement);
            receive(&mut client, operation_id, exact(&replacement));
            assert!(matches!(
                client.poll_event(),
                Some(BatchEvent::Terminal(BatchTerminal::ExactAppliedResultUnavailable { .. }))
            ));
        }
        SwitchExit::Stop | SwitchExit::Lifetime => {
            for expected_stream in 1..=1 {
                let Some(BatchEvent::Terminal(BatchTerminal::OutcomeUnknown { batch, cause, .. })) =
                    client.poll_event()
                else {
                    panic!("session stop retains each original once")
                };
                assert_eq!(batch.first_input().stream_id, expected_stream);
                assert!(matches!(
                    (exit, cause),
                    (SwitchExit::Stop, ClientFailure::Stopped)
                        | (SwitchExit::Lifetime, ClientFailure::RequestLifetimeExpired)
                ));
            }
            assert!(client.poll_event().is_none());
            assert!(matches!(client.status(), ClientStatus::Batch { occupied: 0, stopped: true, .. }));
        }
    }
    assert!(validate_batch_state(&client.state, None, None).is_ok());
}
