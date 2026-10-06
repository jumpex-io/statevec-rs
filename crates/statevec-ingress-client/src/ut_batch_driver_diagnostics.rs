//! Malformed physical replies are diagnostics after the real owner retires
//! the connection; presentation must not replace that decision with Stop.
use super::*;

#[derive(Clone, Copy)]
enum BadReply {
    Header,
    Body,
}

#[test_case::test_case(BadReply::Header; "reader_rejects_header")]
#[test_case::test_case(BadReply::Body; "owner_rejects_body")]
fn malformed_reply_does_not_fail_the_manual_drive_or_stop_recovery(fault: BadReply) {
    let clock = Clock::new();
    let mut client = client(&clock);
    let first = bytes();
    let successor = bytes();
    let (mut driver, network) = driver(&[first.clone(), successor.clone()]);
    drain(&mut driver, &mut client);
    let handle = client.try_submit(batch(1)).unwrap();
    drain(&mut driver, &mut client);
    let sent = requests(&first).pop().unwrap();
    let reply = Reply {
        binding: ReplyBinding::for_request(&sent),
        observation: None,
        disposition: ReplyDisposition::Refused(BatchSubmitRefusal::InternalFailure),
    };
    let mut malformed = stream_batch::encode_reply(&reply, FRAME_LIMIT as usize, COMMAND_LIMIT as usize).unwrap();
    match fault {
        BadReply::Header => malformed[0] = b'?',
        BadReply::Body => *malformed.last_mut().unwrap() = u8::MAX,
    }
    first.lock().unwrap().incoming.extend(malformed);
    ready(&mut driver, &mut client);
    let mut diagnostic = None;
    for _ in 0..STEPS {
        let result = driver.step(&mut client);
        assert!(
            result.is_ok(),
            "a consumed connection fault must be a diagnostic, not a failed manual drive: {result:?}"
        );
        match result.unwrap() {
            BatchDriverStep::Runnable => (),
            BatchDriverStep::Waiting => break,
            BatchDriverStep::Diagnostic { cause } => {
                assert!(diagnostic.replace(cause).is_none(), "one consumed fault has one diagnostic");
            }
        }
    }
    match fault {
        BadReply::Header => assert!(matches!(
            diagnostic,
            Some(ClientFailure::BatchRead {
                source: stream_batch::ReadFailure::Frame { source: stream_batch::FrameFailure::InvalidMagic }
            })
        )),
        BadReply::Body => assert!(matches!(
            diagnostic,
            Some(ClientFailure::BatchFrame {
                source: stream_batch::FrameFailure::UnsupportedRefusal { observed: u8::MAX }
            })
        )),
    }
    assert_eq!(first.lock().unwrap().drops, 1);
    assert!(client.poll_event().is_none(), "malformed reply is not a session Stop or terminal");
    clock.set(INITIAL_TIME + RETRY);
    drain(&mut driver, &mut client);
    let retried = requests(&successor).pop().unwrap();
    assert_eq!(retried.batch(), sent.batch(), "next endpoint receives the exact original intent");
    assert_ne!(retried.context().request_id, sent.context().request_id);
    assert_eq!(network.lock().unwrap().addresses, endpoints().iter().map(|e| e.1).collect::<Vec<_>>());
    enqueue(&successor, completed(&retried));
    ready(&mut driver, &mut client);
    drain(&mut driver, &mut client);
    assert!(
        matches!(client.poll_event(), Some(BatchEvent::Terminal(BatchTerminal::Applied { handle: actual, .. })) if actual == handle)
    );
    assert!(client.poll_event().is_none());
}

#[test]
fn readiness_wait_diagnostic_preserves_the_same_connection_and_pending_intent() {
    let clock = Clock::new();
    let mut client = client(&clock);
    let device = bytes();
    let (mut driver, network) = driver(&[device.clone()]);
    drain(&mut driver, &mut client);
    let handle = client.try_submit(batch(1)).unwrap();
    drain(&mut driver, &mut client);
    let sent = requests(&device).pop().unwrap();
    network.lock().unwrap().wait_error = Some(io::ErrorKind::Interrupted);
    assert!(matches!(driver.wait(&mut client, Duration::ZERO),
        Ok(BatchDriverStep::Diagnostic { cause: ClientFailure::BatchWait { source } })
            if source.kind() == io::ErrorKind::Interrupted));
    assert!(client.poll_event().is_none());
    assert_eq!(device.lock().unwrap().drops, 0, "wait diagnosis cannot retire a connection behind the owner");
    enqueue(&device, completed(&sent));
    ready(&mut driver, &mut client);
    drain(&mut driver, &mut client);
    assert!(
        matches!(client.poll_event(), Some(BatchEvent::Terminal(BatchTerminal::Applied { handle: actual, .. })) if actual == handle)
    );
    assert_eq!(network.lock().unwrap().addresses.len(), 1);
}

#[derive(Clone, Copy)]
enum WaitBudget {
    HostCeiling,
    OwnerDeadline,
    PartlySpent,
    FullySpent,
    Nonblocking,
}

#[test_case::test_case(WaitBudget::HostCeiling; "host_ceiling")]
#[test_case::test_case(WaitBudget::OwnerDeadline; "owner_deadline")]
#[test_case::test_case(WaitBudget::PartlySpent; "deduct_elapsed_wait")]
#[test_case::test_case(WaitBudget::FullySpent; "exhausted_budget")]
#[test_case::test_case(WaitBudget::Nonblocking; "zero_budget")]
fn persistent_wait_errors_pause_only_within_the_remaining_budget(cut: WaitBudget) {
    // Real owner/driver, with only the physical wait failing. Parking is
    // observed directly, so OS scheduling cannot make an unpadded wait pass.
    let (host_ceiling, until_due, wait_budget, elapsed_floor) = match cut {
        WaitBudget::HostCeiling => (40, OPERATION, 40, 0),
        WaitBudget::OwnerDeadline => (OPERATION, 10, 10, 0),
        WaitBudget::PartlySpent => (20, OPERATION, 20, 5),
        WaitBudget::FullySpent => (1, OPERATION, 1, 1),
        WaitBudget::Nonblocking => (0, OPERATION, 0, 0),
    };
    let clock = Clock::new();
    let mut client = client(&clock);
    let device = bytes();
    let (mut driver, network) = driver(&[device.clone()]);
    drain(&mut driver, &mut client);
    let handle = client.try_submit(batch(1)).unwrap();
    drain(&mut driver, &mut client);
    let sent = requests(&device).pop().unwrap();
    clock.set(INITIAL_TIME + OPERATION - until_due);

    for attempt in 1..=3 {
        assert!(matches!(driver.step(&mut client), Ok(BatchDriverStep::Waiting)));
        network.lock().unwrap().wait_error = Some(io::ErrorKind::AlreadyExists);
        network.lock().unwrap().wait_error_delay = Duration::from_millis(elapsed_floor);
        assert!(matches!(driver.wait(&mut client, Duration::from_millis(host_ceiling)),
            Ok(BatchDriverStep::Diagnostic { cause: ClientFailure::BatchWait { source } })
                if source.kind() == io::ErrorKind::AlreadyExists));
        let network = network.lock().unwrap();
        assert_eq!(network.waits.last().unwrap().2, Some(Duration::from_millis(wait_budget)));
        assert_eq!(network.failed_wait_pauses.len(), attempt, "each failed wait must park, not spin");
        let remaining = *network.failed_wait_pauses.last().unwrap();
        assert_eq!(
            remaining,
            Duration::from_millis(wait_budget - elapsed_floor),
            "failed wait must deduct exactly the device's elapsed time from its existing budget"
        );
        assert!(client.poll_event().is_none(), "wait diagnostics cannot terminate the owner's intent");
        assert_eq!(device.lock().unwrap().drops, 0);
    }

    driver.stop(&mut client).unwrap();
    drain(&mut driver, &mut client);
    assert!(driver.is_quiescent());
    assert_eq!(device.lock().unwrap().drops, 1);
    assert_eq!(network.lock().unwrap().addresses.len(), 1);
    assert!(matches!(client.poll_event(), Some(BatchEvent::Terminal(BatchTerminal::OutcomeUnknown {
        handle: actual, batch: original, cause: ClientFailure::Stopped, ..
    })) if actual == handle && wire(&original) == wire(sent.batch())));
    assert!(client.poll_event().is_none());
}
