//! Explicit caller reconstruction, through the real owner and wire codec.
use super::*;
use test_case::test_case;

#[test_case(false; "unknown commitment")]
#[test_case(true; "known commitment")]
fn new_session_import_preserves_original_uncertainty_and_commitment(known: bool) {
    let clock = Clock::new();
    let original = wire(&batch(1));
    let mut old = client(&clock);
    connect(&mut old);
    old.try_submit(batch(1)).unwrap();
    let (connection, submit) = take_write(&mut old);
    write_return(&mut old, connection, &submit);
    if known {
        receive(&mut old, connection, reply(&submit, ReplyDisposition::CommittedAwaitingApply(entry())));
    }
    drop(old);

    clock.set(INITIAL_TIME + LIFETIME * 2);
    let sample = clock.clone();
    // New session identity; no old slot, transport or request correlation survives.
    let mut recovered = BatchClient::bounded(
        (Digest32::new([11; 32]), Digest32::new([22; 32]), Digest32::new([33; 32])),
        NonZeroU128::new(10).unwrap(),
        idle_start(),
        endpoints(),
        COMMAND_LIMIT,
        FRAME_LIMIT,
        OUTCOME_LIMIT,
        RETRY,
        OPERATION,
        LIFETIME,
        3,
        move || sample.sample(),
    )
    .unwrap();
    connect(&mut recovered);
    let input = ClientStreamBatch::decode_v1(&original, FRAME_LIMIT as usize, COMMAND_LIMIT as usize).unwrap();
    let committed = known.then(entry);
    let handle = recovered.try_import(input, committed).unwrap();
    let ClientState::Batch { slot, .. } = &recovered.state;
    assert!(
        matches!(slot, Some(BatchSlot::Pending { submit_transmissions: 2, committed: saved, .. })
        if *saved == committed),
        "import must preserve uncertainty and known commitment before any transmission"
    );
    assert_eq!(handle.context.session_incarnation.get(), 10);
    assert!(matches!(recovered.try_submit(batch(1)).unwrap_err().reason, ClientFailure::Full));
    let (connection, query) = take_frame(&mut recovered);
    assert_eq!(matches!(query, Request::Reconcile { .. }), known);
    assert_eq!(wire(query.batch()), original);
    write_return(&mut recovered, connection, &query);
    receive(&mut recovered, connection, exact(&query));
    assert!(matches!(recovered.status(), ClientStatus::Batch { occupied: 1, .. }));
    assert!(matches!(recovered.poll_event(), Some(BatchEvent::Terminal(
        BatchTerminal::ExactAppliedResultUnavailable { handle: actual, .. })) if actual == handle));
    let mut next = idle_start();
    next.client_seq += 1;
    recovered.try_submit(batch_at(next, 1)).unwrap();
}

#[test]
fn import_refusal_returns_full_input_and_evidence_without_reservation() {
    let clock = Clock::new();
    let mut client = client(&clock);
    let original = wire(&batch(1));
    let refused = client.try_import(batch(1), Some(entry())).unwrap_err();
    assert!(matches!(refused.reason, ClientFailure::NotConnected));
    assert_eq!(refused.committed, Some(entry()));
    assert_eq!(wire(&refused.batch), original);
    connect(&mut client);
    for stream in 1..=1 {
        client.try_import(batch(stream), None).unwrap();
    }
    let original = wire(&batch(1));
    let refused = client.try_import(batch(1), Some(entry())).unwrap_err();
    assert!(matches!(refused.reason, ClientFailure::Full));
    assert_eq!(refused.committed, Some(entry()));
    assert_eq!(wire(&refused.batch), original);
    assert!(matches!(client.status(), ClientStatus::Batch { occupied: 1, .. }));
    assert!(validate_batch_state(&client.state, None, None).is_ok());
}

#[statevec_domain_roles::semantic_evidence(transition = IngressClientOwner::try_import, kind = OwnerPath)]
#[test]
fn imported_input_refusal_cannot_prove_not_admitted_and_stop_returns_evidence() {
    let clock = Clock::new();
    let mut client = client(&clock);
    connect(&mut client);
    let handle = client.try_import(batch(1), Some(entry())).unwrap();
    let (connection, query) = take_query(&mut client);
    write_return(&mut client, connection, &query);
    receive(&mut client, connection, reply(&query, ReplyDisposition::Refused(BatchSubmitRefusal::InvalidInput)));
    assert!(
        matches!(client.poll_event(), Some(BatchEvent::Progress(BatchProgress::Problem { .. }))),
        "imported input must not become NotAdmitted from a new session's refusal"
    );
    assert!(matches!(client.drive(ClientEvent::Stop), ClientResult::Close { .. }));
    assert!(matches!(client.poll_event(), Some(BatchEvent::Terminal(BatchTerminal::OutcomeUnknown {
        handle: actual, committed: Some(saved), batch: original, ..
    })) if actual == handle && saved == entry() && wire(&original) == wire(&batch(1))));
}
