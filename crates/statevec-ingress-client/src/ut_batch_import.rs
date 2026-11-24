//! Explicit caller reconstruction, through the real owner and wire codec.
use crate::{BatchClientSettings, DeploymentIdentity};
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
        BatchClientSettings::new(
            DeploymentIdentity::new(Digest32::new([11; 32]), Digest32::new([22; 32]), Digest32::new([33; 32])),
            NonZeroU128::new(10).unwrap(),
            idle_start(),
            endpoints(),
            COMMAND_LIMIT,
            FRAME_LIMIT,
            OUTCOME_LIMIT,
            3,
        )
        .with_retry(RETRY)
        .with_operation_millis(OPERATION)
        .with_lifetime_millis(LIFETIME),
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
    assert!(matches!(recovered.try_submit(batch(1)).unwrap_err().reason, ClientError::Full));
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

#[test_case(None; "unknown commit resends the original")]
#[test_case(Some(entry()); "known commit reconciles the original")]
fn smaller_sender_recovers_all_1024_results(committed: Option<EntryBinding>) {
    const MEMBERS: usize = 1024;
    const FRAME: usize = 2 << 20;
    let mut client = construct_limits(
        (Digest32::new([11; 32]), Digest32::new([22; 32]), Digest32::new([33; 32])),
        16, FRAME as u32, OUTCOME_LIMIT, 3,
    ).unwrap();
    let connection = connect(&mut client);
    let (_, commands) = batch(1).into_parts();
    let original = ClientStreamBatch::try_new(idle_start(), vec![commands[0].clone(); MEMBERS], FRAME, MEMBERS).unwrap();
    let encoded = original.encode_v1(FRAME, MEMBERS).unwrap();
    let refused = client.try_submit(original).unwrap_err();
    assert!(matches!(refused.reason, ClientError::TooLarge {
        source: ClientStreamBatchError::InvalidCount { observed: MEMBERS, limit: 16 }
    }));
    let handle = client.try_import(refused.batch, committed)
        .unwrap_or_else(|refusal| panic!("the local fresh cap must allow recovery: {:?}", refusal.reason));
    let ClientResult::Write { bytes, .. } = client.drive(ClientEvent::Drive) else { panic!("missing recovery write") };
    let request = stream_batch::decode_request(&bytes, FRAME, MEMBERS).unwrap();
    assert_eq!(matches!(request, Request::Reconcile { .. }), committed.is_some());
    assert_eq!(request.batch().encode_v1(FRAME, MEMBERS).unwrap(), encoded);
    write_return(&mut client, connection, &request);

    let bytes = stream_batch::encode_reply(&completed(&request), FRAME, MEMBERS).unwrap();
    let mut reader = client.bind_batch_stream(connection, std::io::Cursor::new(bytes)).unwrap();
    let event = loop {
        match reader.poll() {
            std::task::Poll::Ready(Some(event)) => break event,
            std::task::Poll::Ready(None) => (),
            std::task::Poll::Pending => panic!("the complete reply is available"),
        }
    };
    assert!(matches!(client.drive(event), ClientResult::Waiting { .. }));
    assert!(matches!(client.poll_event(), Some(BatchEvent::Terminal(BatchTerminal::Applied {
        handle: actual, entry: applied, results, ..
    })) if actual == handle && applied == entry() && results.len() == MEMBERS));
    assert!(client.poll_event().is_none());
}

#[test]
fn import_refuses_a_batch_whose_worst_reply_exceeds_the_frame_before_reservation() {
    let mut client = construct_limits(
        (Digest32::new([11; 32]), Digest32::new([22; 32]), Digest32::new([33; 32])),
        1, FRAME_LIMIT, OUTCOME_LIMIT, 3,
    ).unwrap();
    connect(&mut client);
    let (_, commands) = batch(1).into_parts();
    let original = ClientStreamBatch::try_new(idle_start(), vec![commands[0].clone(); 32], FRAME_LIMIT as usize, 32).unwrap();
    let refusal = client.try_import(original, Some(entry())).unwrap_err();
    assert!(matches!(refusal.reason, ClientError::TooLarge { .. }));
    assert_eq!(refusal.committed, Some(entry()));
    assert_eq!(refusal.batch.commands().len(), 32);
    assert_eq!(client.try_submit(batch(1)).unwrap().context.request_id.get(), 1);
}

#[test]
fn import_refusal_returns_full_input_and_evidence_without_reservation() {
    let clock = Clock::new();
    let mut client = client(&clock);
    let original = wire(&batch(1));
    let refused = client.try_import(batch(1), Some(entry())).unwrap_err();
    assert!(matches!(refused.reason, ClientError::NotConnected));
    assert_eq!(refused.committed, Some(entry()));
    assert_eq!(wire(&refused.batch), original);
    connect(&mut client);
    for stream in 1..=1 {
        client.try_import(batch(stream), None).unwrap();
    }
    let original = wire(&batch(1));
    let refused = client.try_import(batch(1), Some(entry())).unwrap_err();
    assert!(matches!(refused.reason, ClientError::Full));
    assert_eq!(refused.committed, Some(entry()));
    assert_eq!(wire(&refused.batch), original);
    assert!(matches!(client.status(), ClientStatus::Batch { occupied: 1, .. }));
    assert!(validate_batch_state(&client.state, None, None).is_ok());
}

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
