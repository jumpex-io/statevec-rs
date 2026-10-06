//! Real client transitions and codec frames, with only time and peer replies
//! scripted. No alternate retry model or helper service decides client policy.
use super::*;
use test_case::test_case;

#[path = "ut_batch_retirement.rs"]
mod retirement;

#[path = "ut_batch_invariants.rs"]
mod invariants;

#[path = "ut_batch_routing.rs"]
mod routing;

#[path = "ut_batch_same_intent.rs"]
mod same_intent;

fn sent(client: &mut BatchClient) -> (u64, Request) {
    connect(client);
    client.try_submit(batch(1)).unwrap();
    let (connection, submit) = take_write(client);
    write_return(client, connection, &submit);
    (connection, submit)
}

fn admitted(client: &mut BatchClient) -> (u64, Request) {
    let (connection, submit) = sent(client);
    receive(client, connection, reply(&submit, ReplyDisposition::IngressAdmitted(entry())));
    assert!(matches!(client.poll_event(), Some(BatchEvent::Progress(BatchProgress::IngressAdmitted { .. }))));
    (connection, submit)
}

fn committed(client: &mut BatchClient) -> (u64, Request) {
    let (connection, submit) = sent(client);
    receive(client, connection, reply(&submit, ReplyDisposition::CommittedAwaitingApply(entry())));
    assert!(matches!(client.poll_event(), Some(BatchEvent::Progress(BatchProgress::CommittedAwaitingApply { .. }))));
    (connection, submit)
}

fn at_term(mut response: Reply, term: u64) -> Reply {
    let observation = response.observation.unwrap();
    let applied_term =
        if matches!(response.disposition, ReplyDisposition::RetryableNext) { term } else { observation.applied_term() };
    response.observation =
        Some(ReplyObservation::new(term, observation.applied_index(), applied_term, None, None).unwrap());
    response
}

#[track_caller]
fn waiting_until(client: &mut BatchClient, millis: u64) {
    assert!(
        matches!(client.drive(ClientEvent::Drive), ClientResult::Waiting { wake_at: Some(due) }
        if due.get() == millis),
        "owner must expose the exact next active deadline"
    );
    assert!(validate_batch_state(&client.state, None, None).is_ok());
}

#[test]
fn unanswered_query_survives_observation_intervals_and_settles_original_handle_once() {
    let clock = Clock::new();
    let mut client = client(&clock);
    let (connection, submit) = committed(&mut client);
    waiting_until(&mut client, INITIAL_TIME + RETRY);
    clock.set(INITIAL_TIME + RETRY);
    let (_, query) = take_query(&mut client);
    assert_eq!(wire(query.batch()), wire(submit.batch()));
    assert_ne!(query.context().request_id, submit.context().request_id);
    write_return(&mut client, connection, &query);
    for interval in 2..=5 {
        clock.set(INITIAL_TIME + RETRY * interval);
        assert!(
            matches!(client.drive(ClientEvent::Drive), ClientResult::Waiting { wake_at: Some(due) }
            if due.get() == INITIAL_TIME + RETRY + OPERATION),
            "observation ticks must not replace an unanswered query"
        );
    }
    receive(&mut client, connection, exact(&query));
    receive(&mut client, connection, exact(&query));
    assert!(matches!(client.status(), ClientStatus::Batch { occupied: 1, .. }));
    let Some(BatchEvent::Terminal(BatchTerminal::ExactAppliedResultUnavailable { handle, binding, .. })) =
        client.poll_event()
    else {
        panic!("delayed current query must settle despite crossing observation intervals")
    };
    assert_eq!(handle.context, *submit.context());
    assert_eq!(handle.intent, query.batch().commitment_v1());
    assert_eq!(binding.raft_index(), entry().index.get());
    assert!(client.poll_event().is_none());
    assert!(matches!(client.status(), ClientStatus::Batch { occupied: 0, .. }));
}

#[test_case(BatchSubmitRefusal::Busy; "busy")]
#[test_case(BatchSubmitRefusal::NotReady; "not ready")]
fn sole_transmission_refusal_retries_only_after_positive_interval(reason: BatchSubmitRefusal) {
    let clock = Clock::new();
    let mut client = client(&clock);
    connect(&mut client);
    client.try_submit(batch(1)).unwrap();
    let (connection, submit) = take_write(&mut client);
    write_return(&mut client, connection, &submit);
    receive(&mut client, connection, reply(&submit, ReplyDisposition::Refused(reason)));
    waiting_until(&mut client, INITIAL_TIME + RETRY);
    clock.set(INITIAL_TIME + RETRY - 1);
    waiting_until(&mut client, INITIAL_TIME + RETRY);
    clock.set(INITIAL_TIME + RETRY);
    let (_, retried) = take_write(&mut client);
    assert_eq!(wire(retried.batch()), wire(submit.batch()));
    assert_ne!(retried.context().request_id, submit.context().request_id);
    write_return(&mut client, connection, &retried);
    receive(&mut client, connection, reply(&retried, ReplyDisposition::Refused(reason)));
    clock.set(INITIAL_TIME + RETRY * 2);
    take_write(&mut client);
}

#[test_case(BatchSubmitRefusal::InternalFailure; "fatal query node")]
#[test_case(BatchSubmitRefusal::ShuttingDown; "draining query node")]
#[test_case(BatchSubmitRefusal::NotReady; "temporarily unavailable query node")]
fn unavailable_query_does_not_suspend_or_authorize_submit(reason: BatchSubmitRefusal) {
    let clock = Clock::new();
    let mut client = client(&clock);
    let (connection, _) = committed(&mut client);
    clock.set(INITIAL_TIME + RETRY);
    let (_, query) = take_query(&mut client);
    write_return(&mut client, connection, &query);
    let mut refused = reply(&query, ReplyDisposition::Refused(reason));
    refused.observation = None;
    receive(&mut client, connection, refused);
    assert!(client.poll_event().is_none());
    let connection = match reason {
        BatchSubmitRefusal::InternalFailure | BatchSubmitRefusal::ShuttingDown => {
            routing::close_and_connect(&mut client, &clock, connection, 2)
        }
        _ => {
            clock.set(INITIAL_TIME + RETRY * 2);
            connection
        }
    };
    let (_, next) = take_query(&mut client);
    write_return(&mut client, connection, &next);
    receive(&mut client, connection, exact(&next));
    assert!(matches!(
        client.poll_event(),
        Some(BatchEvent::Terminal(BatchTerminal::ExactAppliedResultUnavailable { .. }))
    ));
}

#[test_case(false; "wrong operation disposition")]
#[test_case(true; "wrong current query profile")]
fn current_query_joins_kind_and_full_context(corrupt_profile: bool) {
    let clock = Clock::new();
    let mut client = client(&clock);
    let (connection, submit) = committed(&mut client);
    clock.set(INITIAL_TIME + RETRY);
    let (_, query) = take_query(&mut client);
    write_return(&mut client, connection, &query);
    let mut response =
        if corrupt_profile { exact(&query) } else { reply(&query, ReplyDisposition::IngressAdmitted(entry())) };
    if corrupt_profile {
        response.binding.context.execution_profile = Digest32::new([92; 32]);
    }
    receive(&mut client, connection, response);
    let Some(BatchEvent::Progress(BatchProgress::Problem { cause, .. })) = client.poll_event() else {
        panic!("current query corruption must surface without settling intent")
    };
    let ClientFailure::BatchReply { source } = cause.as_ref() else {
        panic!("expected typed reply failure: {cause:?}")
    };
    assert_eq!(
        *source,
        if corrupt_profile {
            ReplyRequestFailure::ContextMismatch
        } else {
            ReplyRequestFailure::UnexpectedReconcileDisposition
        }
    );
    receive(&mut client, connection, completed(&submit));
    assert!(matches!(client.poll_event(), Some(BatchEvent::Terminal(BatchTerminal::Applied { .. }))));
}

#[test]
fn suspension_during_query_preserves_its_exact_completion_and_physical_deadline() {
    let clock = Clock::new();
    let mut client = client(&clock);
    let (connection, submit) = committed(&mut client);
    clock.set(INITIAL_TIME + RETRY);
    let (_, query) = take_query(&mut client);
    write_return(&mut client, connection, &query);
    receive(
        &mut client,
        connection,
        reply(
            &submit,
            ReplyDisposition::CommittedAwaitingApply(EntryBinding {
                digest: EntryDigest::from_untrusted_wire([99; 32]),
                ..entry()
            }),
        ),
    );
    assert!(
        matches!(client.poll_event(), Some(BatchEvent::Progress(BatchProgress::Problem { cause, .. })) if matches!(*cause, ClientFailure::ConflictingCommittedEntry { .. }))
    );
    clock.set(INITIAL_TIME + RETRY * 3);
    waiting_until(&mut client, INITIAL_TIME + RETRY + OPERATION);
    receive(&mut client, connection, exact(&query));
    assert!(matches!(
        client.poll_event(),
        Some(BatchEvent::Terminal(BatchTerminal::ExactAppliedResultUnavailable { .. }))
    ));
}

#[test]
fn query_timeout_waits_for_close_then_uses_fresh_id_and_original_intent() {
    let clock = Clock::new();
    let mut client = client(&clock);
    let (connection, submit) = committed(&mut client);
    clock.set(INITIAL_TIME + RETRY);
    let (_, old_query) = take_query(&mut client);
    write_return(&mut client, connection, &old_query);
    clock.set(INITIAL_TIME + RETRY + OPERATION);
    assert!(
        matches!(client.drive(ClientEvent::Drive), ClientResult::Close { operation_id } if operation_id == connection)
    );
    receive(&mut client, connection, exact(&old_query));
    assert!(client.poll_event().is_none());
    clock.set(INITIAL_TIME + RETRY * 2 + OPERATION);
    waiting_until(&mut client, INITIAL_TIME + LIFETIME);
    assert!(matches!(client.drive(ClientEvent::Closed { operation_id: connection }), ClientResult::Waiting { .. }));
    let ClientResult::Connect { operation_id: next_connection, peer_id: 2, .. } = client.drive(ClientEvent::Drive)
    else {
        panic!("only physical retirement permits reconnect")
    };
    assert_ne!(next_connection, connection);
    assert!(matches!(
        client.drive(ClientEvent::Connected { operation_id: next_connection, result: Ok(()) }),
        ClientResult::Waiting { .. }
    ));
    let (_, next_query) = take_query(&mut client);
    assert_ne!(next_query.context().request_id, old_query.context().request_id);
    assert_eq!(wire(next_query.batch()), wire(submit.batch()));
    write_return(&mut client, next_connection, &next_query);
    receive(&mut client, connection, exact(&old_query));
    receive(&mut client, next_connection, exact(&old_query));
    assert!(client.poll_event().is_none(), "old query stays retired even on the successor connection");
    receive(&mut client, next_connection, exact(&next_query));
    assert!(
        matches!(client.poll_event(), Some(BatchEvent::Terminal(BatchTerminal::ExactAppliedResultUnavailable { handle, .. })) if handle.context == *submit.context())
    );
}

#[test]
fn waiting_slot_wakes_and_queries_without_other_business_events() {
    let clock = Clock::new();
    let mut client = client(&clock);
    let mut connection = connect(&mut client);
    for stream in 1..=1 {
        client.try_submit(batch(stream)).unwrap();
    }
    for stream in 1..=1 {
        let (_, submit) = take_write(&mut client);
        assert_eq!(submit.batch().first_input().stream_id, stream);
        write_return(&mut client, connection, &submit);
        receive(&mut client, connection, reply(&submit, ReplyDisposition::CommittedAwaitingApply(entry())));
    }
    let mut ids = Vec::new();
    let mut unavailable = 0;
    let mut peer = 1;
    for _ in 1..=3 {
        clock.set(clock.sample().unwrap().get() + RETRY);
        for stream in 1..=1 {
            let (_, query) = take_query(&mut client);
            assert_eq!(query.batch().first_input().stream_id, stream);
            ids.push(query.context().request_id);
            write_return(&mut client, connection, &query);
            // Repeated commitment is not apply progress. Slot scheduling must
            // remain fair across the resulting bounded endpoint rotations.
            receive(&mut client, connection, reply(&query, ReplyDisposition::CommittedAwaitingApply(entry())));
            unavailable += 1;
            if unavailable >= UNAVAILABLE_LIMIT {
                peer = if peer == 1 { 2 } else { 1 };
                connection = routing::close_and_connect(&mut client, &clock, connection, peer);
                unavailable = 0;
            }
        }
    }
    assert!(ids.windows(2).all(|ids| ids[0] < ids[1]));
    assert!(matches!(client.status(), ClientStatus::Batch { occupied: 1, .. }));
}

#[test]
fn pending_problem_does_not_hide_later_committed_progress() {
    let clock = Clock::new();
    let mut client = client(&clock);
    let (connection, submit) = sent(&mut client);
    receive(&mut client, connection, reply(&submit, ReplyDisposition::Conflict));
    receive(&mut client, connection, reply(&submit, ReplyDisposition::CommittedAwaitingApply(entry())));

    assert!(matches!(client.poll_event(), Some(BatchEvent::Progress(BatchProgress::Problem { .. }))));
    assert!(
        matches!(client.poll_event(), Some(BatchEvent::Progress(BatchProgress::CommittedAwaitingApply { entry: known, .. })) if known == entry()),
        "consuming a problem must expose independently confirmed committed progress"
    );
    assert!(client.poll_event().is_none());
    assert!(matches!(client.status(), ClientStatus::Batch { occupied: 1, .. }));
    receive(&mut client, connection, completed(&submit));
    assert!(matches!(client.poll_event(), Some(BatchEvent::Terminal(BatchTerminal::Applied { .. }))));
}

#[test]
fn consumed_problem_survives_later_diagnostics_commit_and_stop() {
    let clock = Clock::new();
    let mut client = client(&clock);
    let (connection, submit) = sent(&mut client);
    receive(&mut client, connection, reply(&submit, ReplyDisposition::Conflict));
    assert!(matches!(client.poll_event(), Some(BatchEvent::Progress(BatchProgress::Problem { .. }))));
    // A repeated failure cannot replace the first diagnostic or restart work.
    receive(&mut client, connection, reply(&submit, ReplyDisposition::Refused(BatchSubmitRefusal::InternalFailure)));
    receive(&mut client, connection, reply(&submit, ReplyDisposition::CommittedAwaitingApply(entry())));
    assert!(matches!(client.drive(ClientEvent::Stop), ClientResult::Close { .. }));

    let Some(BatchEvent::Terminal(BatchTerminal::OutcomeUnknown { batch: original, committed, problem, .. })) =
        client.poll_event()
    else {
        panic!("stopping suspended work must return the original uncertain intent")
    };
    assert!(
        matches!(problem.as_deref(), Some(ClientFailure::BatchConflict)),
        "consuming a problem must not erase or replace the first diagnostic at Stop: {problem:?}"
    );
    assert_eq!(committed, Some(entry()));
    assert_eq!(wire(&original), wire(submit.batch()));
    assert!(client.poll_event().is_none());
}

#[test]
fn query_reply_at_deadline_cannot_settle_the_original() {
    let clock = Clock::new();
    let mut client = client(&clock);
    let (connection, _) = committed(&mut client);
    clock.set(INITIAL_TIME + RETRY);
    let (_, query) = take_query(&mut client);
    write_return(&mut client, connection, &query);
    clock.set(INITIAL_TIME + RETRY + OPERATION);
    let bytes = stream_batch::encode_reply(&exact(&query), FRAME_LIMIT as usize, COMMAND_LIMIT as usize).unwrap();
    let effect = client.drive(ClientEvent::Read { operation_id: connection, result: Ok(bytes) });
    assert!(client.poll_event().is_none(), "query deadline must win over a queued exact reply");
    assert!(matches!(effect, ClientResult::Close { .. }));
    assert!(matches!(client.status(), ClientStatus::Batch { occupied: 1, .. }));
}
