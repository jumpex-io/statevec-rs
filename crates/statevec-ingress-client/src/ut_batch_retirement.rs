//! Connection retirement fences correlations, not intent or known history.
use super::*;
use test_case::test_case;

#[derive(Clone, Copy)]
enum Exchange {
    Connect,
    Submit,
    Reconcile,
}

#[test_case(Exchange::Connect; "connect control")]
#[test_case(Exchange::Submit; "submit")]
#[test_case(Exchange::Reconcile; "reconcile")]
fn timeout_records_consumed_time_before_physical_close(exchange: Exchange) {
    let clock = Clock::new();
    let mut client = client(&clock);
    let (connection, started) = match exchange {
        Exchange::Connect => {
            let ClientResult::Connect { operation_id, .. } = client.drive(ClientEvent::Drive) else {
                panic!("initial connection must be dispatched")
            };
            (operation_id, INITIAL_TIME)
        }
        Exchange::Submit => {
            connect(&mut client);
            client.try_submit(batch(1)).unwrap();
            let (connection, submit) = take_write(&mut client);
            write_return(&mut client, connection, &submit);
            (connection, INITIAL_TIME)
        }
        Exchange::Reconcile => {
            let (connection, _) = committed(&mut client);
            clock.set(INITIAL_TIME + RETRY);
            let (_, query) = take_query(&mut client);
            write_return(&mut client, connection, &query);
            (connection, INITIAL_TIME + RETRY)
        }
    };
    let deadline = started + OPERATION;
    clock.set(deadline);
    assert!(
        matches!(client.drive(ClientEvent::Drive), ClientResult::Close { operation_id } if operation_id == connection)
    );

    clock.set(deadline - 1);
    let result = client.drive(ClientEvent::Closed { operation_id: connection });
    assert!(
        matches!(result, ClientResult::ClockFailed {
        cause: ClientFailure::TimeRegressed { previous, observed }
    } if previous.get() == deadline && observed.get() == deadline - 1),
        "timeout must record consumed time before returning Close: {result:?}"
    );
    clock.set(deadline + RETRY);
    assert!(
        matches!(client.drive(ClientEvent::Drive), ClientResult::Waiting { wake_at: None }),
        "regressed time stops the session after releasing physical connection custody"
    );
}

#[track_caller]
fn fail_close_and_reconnect(client: &mut BatchClient, clock: &Clock, connection: u64, now: u64) -> u64 {
    assert!(matches!(
        client.drive(ClientEvent::Read {
            operation_id: connection,
            result: Err(stream_batch::ReadFailure::Io {
                source: std::io::Error::from(std::io::ErrorKind::ConnectionReset),
            }),
        }),
        ClientResult::ConnectionFault { .. }
    ));
    assert!(
        matches!(client.drive(ClientEvent::Drive), ClientResult::Close { operation_id } if operation_id == connection)
    );
    assert!(matches!(client.drive(ClientEvent::Closed { operation_id: connection }), ClientResult::Waiting { .. }));
    clock.set(now + RETRY);
    let ClientResult::Connect { operation_id, .. } = client.drive(ClientEvent::Drive) else {
        panic!("physically closed connection must permit bounded reconnect")
    };
    assert_ne!(operation_id, connection);
    assert!(matches!(
        client.drive(ClientEvent::Connected { operation_id, result: Ok(()) }),
        ClientResult::Waiting { .. }
    ));
    operation_id
}

#[test]
fn zero_term_retry_reply_cannot_authorize_resubmission_after_reconnect() {
    let clock = Clock::new();
    let mut client = client(&clock);
    connect(&mut client);
    let handle = client.try_submit(batch(1)).unwrap();
    let (connection, submit) = take_write(&mut client);
    write_return(&mut client, connection, &submit);
    receive(&mut client, connection, reply(&submit, ReplyDisposition::CommittedAwaitingApply(entry())));
    let _ = client.poll_event();
    let connection = fail_close_and_reconnect(&mut client, &clock, connection, INITIAL_TIME);
    let (_, query) = take_query(&mut client);
    write_return(&mut client, connection, &query);
    let mut observation = reply(&query, ReplyDisposition::Unknown);
    observation.observation = Some(ReplyObservation::new(0, 0, 0, None, None).unwrap());
    let mut bytes = stream_batch::encode_reply(&observation, FRAME_LIMIT as usize, COMMAND_LIMIT as usize).unwrap();
    // Independent tag edit: the valid Unknown envelope cannot certify a
    // current-term settled leader merely by becoming RetryableNext.
    *bytes.last_mut().unwrap() = 6;
    let result = client.drive(ClientEvent::Read { operation_id: connection, result: Ok(bytes) });
    assert!(
        matches!(
            result,
            ClientResult::ConnectionFault {
                cause: ClientFailure::BatchFrame { source: stream_batch::FrameFailure::InvalidReplyObservation },
                ..
            }
        ),
        "term zero cannot become retry authority on a fresh query connection: {result:?}"
    );
    assert!(client.poll_event().is_none());
    let successor = super::routing::close_and_connect(&mut client, &clock, connection, 1);
    let (_, recovery) = take_query(&mut client);
    assert_eq!(wire(recovery.batch()), wire(submit.batch()));
    write_return(&mut client, successor, &recovery);
    receive(&mut client, successor, exact(&recovery));
    assert!(matches!(
        client.poll_event(),
        Some(BatchEvent::Terminal(BatchTerminal::ExactAppliedResultUnavailable { handle: actual, .. })) if actual == handle
    ));
}

#[test]
fn original_submit_reply_on_successor_socket_cannot_settle_unsent_route() {
    let clock = Clock::new();
    let mut client = client(&clock);
    let (connection, submit) = committed(&mut client);
    let successor = fail_close_and_reconnect(&mut client, &clock, connection, INITIAL_TIME);
    let (_, query) = take_query(&mut client);
    write_return(&mut client, successor, &query);

    receive(&mut client, successor, completed(&submit));
    assert!(
        client.poll_event().is_none(),
        "an original Submit ID cannot migrate its reply route to a successor socket"
    );
    assert!(matches!(client.status(), ClientStatus::Batch { occupied: 1, .. }));
    receive(&mut client, successor, exact(&query));
    assert!(matches!(
        client.poll_event(),
        Some(BatchEvent::Terminal(BatchTerminal::ExactAppliedResultUnavailable { .. }))
    ));
}

#[test]
fn physical_retirement_preserves_the_consumed_suspension_diagnostic() {
    let clock = Clock::new();
    let mut client = client(&clock);
    let (connection, submit) = sent(&mut client);
    receive(&mut client, connection, reply(&submit, ReplyDisposition::Conflict));
    let Some(BatchEvent::Progress(BatchProgress::Problem { cause, .. })) = client.poll_event() else {
        panic!("conflict must suspend with its typed cause")
    };
    fail_close_and_reconnect(&mut client, &clock, connection, INITIAL_TIME);
    waiting_until(&mut client, INITIAL_TIME + LIFETIME);
    assert!(client.poll_event().is_none(), "retirement cannot reissue a consumed problem");
    assert!(matches!(client.drive(ClientEvent::Stop), ClientResult::Close { .. }));
    let Some(BatchEvent::Terminal(BatchTerminal::OutcomeUnknown { problem: Some(retained), .. })) = client.poll_event()
    else {
        panic!("socket retirement must preserve the first suspension diagnostic")
    };
    assert!(Arc::ptr_eq(&cause, &retained), "notification and terminal share only the immutable diagnostic");
    assert!(matches!(*retained, ClientFailure::BatchConflict));
}

#[test_case(false; "fresh_original_never_dispatched")]
#[test_case(true; "imported_original_already_uncertain")]
fn connection_loss_before_dispatch_does_not_reclassify_send_history(imported: bool) {
    let clock = Clock::new();
    let mut client = client(&clock);
    let connection = connect(&mut client);
    let handle = if imported { client.try_import(batch(1), None) } else { client.try_submit(batch(1)) }.unwrap();
    let successor = fail_close_and_reconnect(&mut client, &clock, connection, INITIAL_TIME);
    let (actual, submit) = take_write(&mut client);
    assert_eq!(actual, successor);
    assert_eq!(wire(submit.batch()), wire(&batch(1)));
    assert_eq!(
        submit.context().request_id == handle.context.request_id,
        !imported,
        "first wire identity follows transmission history, not connection history"
    );
    write_return(&mut client, successor, &submit);
    receive(&mut client, successor, reply(&submit, ReplyDisposition::Refused(BatchSubmitRefusal::InvalidInput)));
    if imported {
        assert!(matches!(client.poll_event(), Some(BatchEvent::Progress(BatchProgress::Problem { .. }))));
        assert!(matches!(client.status(), ClientStatus::Batch { occupied: 1, .. }));
        let _ = client.drive(ClientEvent::Stop);
        assert!(
            matches!(client.poll_event(), Some(BatchEvent::Terminal(BatchTerminal::OutcomeUnknown { batch: original, .. })) if wire(&original) == wire(&batch(1)))
        );
    } else {
        assert!(
            matches!(client.poll_event(), Some(BatchEvent::Terminal(BatchTerminal::NotAdmitted { batch: original, .. })) if wire(&original) == wire(&batch(1))),
            "connection retirement alone cannot erase first-send non-admission proof"
        );
        assert!(client.try_submit(batch(1)).is_ok(), "refusal cannot advance the fixed stream cursor");
    }
}
