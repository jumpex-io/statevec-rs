//! PoC uncertainty recovery: actual client/codec, scripted remote observations.
use super::*;
use test_case::test_case;

#[test_case(false; "retransmission")]
#[test_case(true; "import")]
fn fresh_admission_after_busy_is_progress_not_an_unavailable_peer(import: bool) {
    let clock = Clock::new();
    let mut client = client(&clock);
    connect(&mut client);
    if import {
        client.try_import(batch(1), None).unwrap();
    } else {
        client.try_submit(batch(1)).unwrap();
    }
    for attempt in 0..2 {
        let (connection, request) = take_write(&mut client);
        write_return(&mut client, connection, &request);
        receive(&mut client, connection, reply(&request, ReplyDisposition::Refused(BatchSubmitRefusal::Busy)));
        clock.set(INITIAL_TIME + (attempt + 1) * RETRY);
    }
    let (connection, request) = take_write(&mut client);
    write_return(&mut client, connection, &request);
    receive(&mut client, connection, reply(&request, ReplyDisposition::IngressAdmitted(entry())));
    assert!(matches!(client.poll_event(), Some(BatchEvent::Progress(BatchProgress::IngressAdmitted { .. }))));
    assert!(
        matches!(client.drive(ClientEvent::Drive), ClientResult::Waiting { .. }),
        "new admission must clear no-progress accounting instead of evicting a healthy peer"
    );
    receive(&mut client, connection, completed(&request));
    assert!(matches!(client.poll_event(), Some(BatchEvent::Terminal(BatchTerminal::Applied { .. }))));
}

#[test_case(ReplyDisposition::Unknown; "unknown")]
#[test_case(ReplyDisposition::IngressAdmitted(entry()); "old admission")]
#[test_case(ReplyDisposition::Refused(BatchSubmitRefusal::Busy); "old busy")]
#[test_case(ReplyDisposition::StreamGapRejected { expected_seq: 40 }; "old gap")]
fn replacement_submit_retires_weak_observations_but_preserves_exact_evidence(disposition: ReplyDisposition) {
    let clock = Clock::new();
    let mut client = client(&clock);
    let (connection, original) = admitted(&mut client);
    clock.set(INITIAL_TIME + RETRY);
    let (_, retry) = take_write(&mut client);
    assert_eq!(wire(retry.batch()), wire(original.batch()), "retry must keep the complete original intent");
    assert_ne!(retry.context().request_id, original.context().request_id, "every new exchange needs a fresh ID");
    write_return(&mut client, connection, &retry);
    let deadline = INITIAL_TIME + RETRY + OPERATION;
    receive(&mut client, connection, reply(&original, disposition));
    assert!(client.poll_event().is_none(), "old weak replies cannot settle or perturb the replacement");
    waiting_until(&mut client, deadline);
    receive(&mut client, connection, completed(&original));
    assert!(
        matches!(client.poll_event(), Some(BatchEvent::Terminal(
        BatchTerminal::Applied { handle, .. })) if handle.context == *original.context()),
        "original live route must preserve business results across Submit replacement"
    );
    receive(&mut client, connection, exact(&retry));
    assert!(client.poll_event().is_none(), "replacement reply must not settle twice");
}

#[test]
fn same_intent_retry_waits_for_physical_write_and_keeps_prior_uncertainty() {
    let clock = Clock::new();
    let mut client = client(&clock);
    connect(&mut client);
    client.try_submit(batch(1)).unwrap();
    let (connection, original) = take_write(&mut client);
    receive(&mut client, connection, reply(&original, ReplyDisposition::IngressAdmitted(entry())));
    let _ = client.poll_event();
    clock.set(INITIAL_TIME + RETRY);
    waiting_until(&mut client, INITIAL_TIME + OPERATION);
    write_return(&mut client, connection, &original);
    let (_, retry) = take_write(&mut client);
    assert_eq!(wire(retry.batch()), wire(original.batch()));
    write_return(&mut client, connection, &retry);
    receive(&mut client, connection, reply(&retry, ReplyDisposition::Refused(BatchSubmitRefusal::InvalidInput)));
    assert!(
        matches!(client.poll_event(), Some(BatchEvent::Progress(BatchProgress::Problem { .. }))),
        "fresh request IDs must not erase retransmission ambiguity"
    );
    assert!(matches!(client.status(), ClientStatus::Batch { occupied: 1, .. }));
    receive(&mut client, connection, completed(&retry));
    assert!(matches!(client.poll_event(), Some(BatchEvent::Terminal(BatchTerminal::Applied { .. }))));
}

#[test]
fn late_commit_changes_scheduled_retry_to_read_only_without_query_permission() {
    let clock = Clock::new();
    let mut client = client(&clock);
    let (connection, original) = admitted(&mut client);
    receive(&mut client, connection, reply(&original, ReplyDisposition::CommittedAwaitingApply(entry())));
    assert!(matches!(client.poll_event(), Some(BatchEvent::Progress(BatchProgress::CommittedAwaitingApply { .. }))));
    clock.set(INITIAL_TIME + RETRY);
    let (_, query) = take_query(&mut client);
    write_return(&mut client, connection, &query);
    receive(&mut client, connection, completed(&original));
    assert!(
        matches!(client.poll_event(), Some(BatchEvent::Terminal(BatchTerminal::Applied { .. }))),
        "retained Submit exact completion must remain usable during Reconcile"
    );
    receive(&mut client, connection, exact(&query));
    assert!(client.poll_event().is_none(), "late query cannot settle twice");
}

#[test_case(2; "isolated older leader")]
#[test_case(3; "same term contradiction")]
#[test_case(4; "newer term contradiction")]
fn absence_cannot_erase_known_commitment(term: u64) {
    let clock = Clock::new();
    let mut client = client(&clock);
    let (connection, original) = committed(&mut client);
    clock.set(INITIAL_TIME + RETRY);
    let (_, query) = take_query(&mut client);
    write_return(&mut client, connection, &query);
    receive(&mut client, connection, at_term(reply(&query, ReplyDisposition::RetryableNext), term));
    if term < entry().term.get() {
        assert!(client.poll_event().is_none(), "an older isolated leader cannot refute a newer committed fact");
        let successor = routing::close_and_connect(&mut client, &clock, connection, 2);
        let (_, query) = take_query(&mut client);
        write_return(&mut client, successor, &query);
        receive(&mut client, successor, exact(&query));
        assert!(matches!(
            client.poll_event(),
            Some(BatchEvent::Terminal(BatchTerminal::ExactAppliedResultUnavailable { .. }))
        ));
    } else {
        assert!(matches!(client.poll_event(), Some(BatchEvent::Progress(BatchProgress::Problem { cause, .. }))
            if matches!(*cause, ClientFailure::CommittedHistoryMissing { known } if known == entry())));
        assert!(matches!(client.drive(ClientEvent::Stop), ClientResult::Close { .. }));
        assert!(matches!(client.poll_event(), Some(BatchEvent::Terminal(BatchTerminal::OutcomeUnknown {
            batch, committed: Some(known), ..
        })) if wire(&batch) == wire(original.batch()) && known == entry()));
    }
}

#[test]
fn repeated_pending_rotates_and_retries_never_accumulate_correlation_history() {
    let clock = Clock::new();
    let mut client = client(&clock);
    let (mut connection, original) = admitted(&mut client);
    let mut latest = original.context().request_id;
    for turn in 1..=12 {
        clock.set(clock.sample().unwrap().get() + RETRY);
        let (_, request) = take_write(&mut client);
        assert!(request.context().request_id > latest);
        latest = request.context().request_id;
        assert_eq!(wire(request.batch()), wire(original.batch()));
        write_return(&mut client, connection, &request);
        receive(&mut client, connection, reply(&request, ReplyDisposition::PendingKnown(entry())));
        let _ = client.poll_event();
        if turn % UNAVAILABLE_LIMIT == 0 {
            // Repeated pending cannot pin any visit indefinitely. Each new
            // selected endpoint gets the same bounded observation allowance.
            let peer = if (turn / UNAVAILABLE_LIMIT) % 2 == 1 { 2 } else { 1 };
            connection = routing::close_and_connect(&mut client, &clock, connection, peer);
        }
        assert!(validate_batch_state(&client.state, None, None).is_ok());
        let ClientState::Batch { slot, .. } = &client.state;
        assert_eq!(slot.iter().count(), 1);
        let Some(BatchSlot::Pending { submit_route, attempt, .. }) = slot else { unreachable!() };
        assert!(attempt.query().is_none());
        assert!(
            submit_route.is_none_or(|(op, first, last)| op == connection && first <= last && last == latest),
            "dispatched Submit interval stays bounded to the current connection and latest exchange"
        );
    }
    let (_, recovery) = take_write(&mut client);
    write_return(&mut client, connection, &recovery);
    receive(&mut client, connection, exact(&recovery));
    assert!(matches!(
        client.poll_event(),
        Some(BatchEvent::Terminal(BatchTerminal::ExactAppliedResultUnavailable { .. }))
    ));
}

#[test]
fn retained_weak_and_commit_replies_do_not_refresh_a_current_query_or_reset_routing() {
    let clock = Clock::new();
    let mut client = client(&clock);
    let (connection, original) = committed(&mut client);
    clock.set(INITIAL_TIME + RETRY);
    let (_, query) = take_query(&mut client);
    write_return(&mut client, connection, &query);
    receive(&mut client, connection, reply(&query, ReplyDisposition::Unknown));
    clock.set(INITIAL_TIME + RETRY * 2);
    let (_, query) = take_query(&mut client);
    write_return(&mut client, connection, &query);
    for disposition in [
        ReplyDisposition::Unknown,
        ReplyDisposition::IngressAdmitted(entry()),
        ReplyDisposition::CommittedAwaitingApply(entry()),
    ] {
        receive(&mut client, connection, reply(&original, disposition));
        waiting_until(&mut client, INITIAL_TIME + RETRY * 2 + OPERATION);
        assert!(
            matches!(&client.state, ClientState::Batch { unavailable_replies: 1, .. }),
            "late retained replies cannot reset another exchange's routing accounting"
        );
    }
    receive(&mut client, connection, exact(&query));
    assert!(matches!(
        client.poll_event(),
        Some(BatchEvent::Terminal(BatchTerminal::ExactAppliedResultUnavailable { .. }))
    ));
}

#[test]
fn older_commit_adds_knowledge_without_ending_the_current_submit_wait() {
    let clock = Clock::new();
    let mut client = client(&clock);
    let (connection, original) = admitted(&mut client);
    clock.set(INITIAL_TIME + RETRY);
    let (_, retry) = take_write(&mut client);
    write_return(&mut client, connection, &retry);
    receive(&mut client, connection, reply(&retry, ReplyDisposition::PendingKnown(entry())));
    let _ = client.poll_event();
    clock.set(INITIAL_TIME + RETRY * 2);
    let (_, current) = take_write(&mut client);
    write_return(&mut client, connection, &current);

    receive(&mut client, connection, reply(&original, ReplyDisposition::CommittedAwaitingApply(entry())));
    assert!(
        matches!(client.poll_event(), Some(BatchEvent::Progress(BatchProgress::CommittedAwaitingApply { .. }))),
        "old dispatched Submit must still establish commitment"
    );
    assert!(
        matches!(&client.state, ClientState::Batch { unavailable_replies: 1, .. }),
        "old committed evidence cannot reset current no-progress accounting"
    );
    waiting_until(&mut client, INITIAL_TIME + RETRY * 2 + OPERATION);
    receive(&mut client, connection, reply(&current, ReplyDisposition::PendingKnown(entry())));
    assert!(client.poll_event().is_none(), "pending observation cannot undo commitment");
    clock.set(INITIAL_TIME + RETRY * 3);
    let (_, query) = take_query(&mut client);
    write_return(&mut client, connection, &query);
    receive(&mut client, connection, exact(&original));
    assert!(
        matches!(client.poll_event(), Some(BatchEvent::Terminal(BatchTerminal::ExactAppliedResultUnavailable { .. }))),
        "old dispatched exact evidence also settles during Reconcile"
    );
}

#[test]
fn old_submit_completion_does_not_release_replacement_write_or_migrate_to_next_batch() {
    let clock = Clock::new();
    let mut client = client(&clock);
    let (connection, original) = admitted(&mut client);
    clock.set(INITIAL_TIME + RETRY);
    let (_, retry) = take_write(&mut client); // its Written observation remains outstanding
    receive(&mut client, connection, completed(&original));
    assert!(
        matches!(client.poll_event(), Some(BatchEvent::Terminal(BatchTerminal::Applied { .. }))),
        "old strong reply must settle without consuming current Write custody"
    );
    let next_handle = client
        .try_submit(batch_at(InputRef { client_seq: 42, ..idle_start() }, 1))
        .unwrap();
    waiting_until(&mut client, INITIAL_TIME + RETRY + OPERATION);
    write_return(&mut client, connection, &retry);
    let (_, next) = take_write(&mut client);
    write_return(&mut client, connection, &next);
    // Even with the new batch's exact intent, an ID issued for the previous
    // batch is not one of this batch's actual transmissions.
    let mut foreign = completed(&next);
    foreign.binding.context.request_id = original.context().request_id;
    receive(&mut client, connection, foreign);
    assert!(client.poll_event().is_none(), "previous batch IDs cannot supply current batch evidence");
    receive(&mut client, connection, completed(&next));
    assert!(
        matches!(client.poll_event(), Some(BatchEvent::Terminal(BatchTerminal::Applied { handle, .. })) if handle == next_handle)
    );
}

#[derive(Clone, Copy)]
enum UnsentId {
    ReservedImport,
    Future,
}

#[test_case(UnsentId::ReservedImport; "reserved_import_id")]
#[test_case(UnsentId::Future; "future_id")]
fn only_actually_dispatched_submit_ids_can_supply_strong_evidence(id: UnsentId) {
    let clock = Clock::new();
    let mut client = client(&clock);
    let connection = connect(&mut client);
    let handle = client.try_import(batch(1), None).unwrap();
    let (_, request) = take_write(&mut client);
    write_return(&mut client, connection, &request);
    let mut unissued = completed(&request);
    unissued.binding.context.request_id = match id {
        UnsentId::ReservedImport => handle.context.request_id,
        UnsentId::Future => NonZeroU64::new(request.context().request_id.get() + 1).unwrap(),
    };
    receive(&mut client, connection, unissued);
    assert!(client.poll_event().is_none(), "never-dispatched Submit ID cannot settle the batch");
    waiting_until(&mut client, INITIAL_TIME + OPERATION);
    receive(&mut client, connection, completed(&request));
    assert!(matches!(client.poll_event(), Some(BatchEvent::Terminal(BatchTerminal::Applied { .. }))));
}

#[test]
fn retired_query_id_does_not_enter_the_retained_submit_interval() {
    let clock = Clock::new();
    let mut client = client(&clock);
    let (connection, original) = committed(&mut client);
    clock.set(INITIAL_TIME + RETRY);
    let (_, old_query) = take_query(&mut client);
    write_return(&mut client, connection, &old_query);
    receive(&mut client, connection, reply(&old_query, ReplyDisposition::Unknown));
    clock.set(INITIAL_TIME + RETRY * 2);
    let (_, current) = take_query(&mut client);
    write_return(&mut client, connection, &current);
    receive(&mut client, connection, completed(&old_query));
    assert!(client.poll_event().is_none(), "retired query ID cannot become retained Submit evidence");
    waiting_until(&mut client, INITIAL_TIME + RETRY * 2 + OPERATION);
    receive(&mut client, connection, completed(&original));
    assert!(matches!(client.poll_event(), Some(BatchEvent::Terminal(BatchTerminal::Applied { .. }))));
}

#[derive(Clone, Copy)]
enum InvalidStrongReply {
    Context,
    Intent,
    ConflictingCommitment,
}

#[test_case(InvalidStrongReply::Context; "context_mismatch")]
#[test_case(InvalidStrongReply::Intent; "intent_mismatch")]
#[test_case(InvalidStrongReply::ConflictingCommitment; "committed_binding_mismatch")]
fn historical_strong_reply_still_requires_complete_semantic_validation(invalid: InvalidStrongReply) {
    let clock = Clock::new();
    let mut client = client(&clock);
    let (connection, original) = admitted(&mut client);
    clock.set(INITIAL_TIME + RETRY);
    let (_, current) = take_write(&mut client);
    write_return(&mut client, connection, &current);
    let mut response = completed(&original);
    let expected = match invalid {
        InvalidStrongReply::Context => {
            response.binding.context.execution_profile = Digest32::new([99; 32]);
            ClientFailure::BatchReply { source: ReplyRequestFailure::ContextMismatch }
        }
        InvalidStrongReply::Intent => {
            response.binding.intent = batch(2).commitment_v1();
            ClientFailure::BatchReply { source: ReplyRequestFailure::IntentMismatch }
        }
        InvalidStrongReply::ConflictingCommitment => {
            let known = EntryBinding { digest: EntryDigest::from_untrusted_wire([99; 32]), ..entry() };
            receive(&mut client, connection, reply(&current, ReplyDisposition::CommittedAwaitingApply(known)));
            let _ = client.poll_event();
            ClientFailure::ConflictingCommittedEntry { known, observed: entry() }
        }
    };
    receive(&mut client, connection, response);
    let Some(BatchEvent::Progress(BatchProgress::Problem { cause, .. })) = client.poll_event() else {
        panic!("historical strong evidence must pass full semantic validation")
    };
    // ClientFailure carries I/O causes and is not PartialEq. Match typed causes,
    // never their rendered diagnostics.
    assert!(match (&*cause, expected) {
        (ClientFailure::BatchReply { source }, ClientFailure::BatchReply { source: expected }) => *source == expected,
        (
            ClientFailure::ConflictingCommittedEntry { known, observed },
            ClientFailure::ConflictingCommittedEntry { known: k, observed: o },
        ) => *known == k && *observed == o,
        _ => false,
    });
    assert!(matches!(client.status(), ClientStatus::Batch { occupied: 1, .. }));
}
