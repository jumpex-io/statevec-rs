//! Malformed physical replies are diagnostics after the real owner retires
//! the connection; presentation must not replace that decision with Stop.
use crate::{DeploymentIdentity, ReadClientSettings};
use super::*;

#[derive(Clone, Copy)]
enum BadReply {
    Header,
    Body,
    Checksum,
    EmptyReadReply,
    ProtocolRefused,
    UnknownDisposition,
}

fn empty_reply(kind: u16) -> [u8; stream_batch::HEADER_BYTES] {
    statevec_frame::wire::WireFormat { magic: *b"SVIN", number: stream_batch::VERSION, kinds: &[3, 5] }
        .encode(kind, &[], 0).unwrap()
}

#[test_case::test_case(BadReply::Header; "reader_rejects_header")]
#[test_case::test_case(BadReply::Body; "owner_rejects_body")]
#[test_case::test_case(BadReply::Checksum; "one_changed_reply_bit_reconnects_with_the_same_intent")]
#[test_case::test_case(BadReply::EmptyReadReply; "wrong reply kind precedes its malformed body")]
#[test_case::test_case(BadReply::ProtocolRefused; "reads frozen refusal from two protocol majors later")]
#[test_case::test_case(BadReply::UnknownDisposition; "unknown disposition retains intent")]
fn malformed_reply_does_not_fail_the_manual_drive_or_stop_recovery(fault: BadReply) {
    let clock = Clock::new();
    let mut client = client(&clock);
    let first = bytes();
    let successor = bytes();
    let (mut driver, network) = driver(&[first.clone(), successor.clone()]);
    drain(&mut driver, &mut client);
    let handle = client.try_submit(batch(1)).unwrap();
    drain(&mut driver, &mut client);
    let sent_protocol = {
        let device = first.lock().unwrap();
        u16::from_le_bytes(device.outgoing[4..6].try_into().unwrap())
    };
    let sent = requests(&first).pop().unwrap();
    let reply = Reply {
        binding: ReplyBinding::for_request(&sent),
        observation: None,
        disposition: ReplyDisposition::Refused(BatchSubmitRefusal::InternalFailure),
    };
    let mut malformed = stream_batch::encode_reply(&reply, FRAME_LIMIT as usize, COMMAND_LIMIT as usize).unwrap();
    match fault {
        BadReply::Header => malformed[0] = b'?',
        BadReply::Body => {
            let refusal = malformed.len() - statevec_frame::stream_batch_protocol::HANDOVER_ADVICE_BYTES - 1;
            malformed[refusal] = 0;
            refresh_reply_checksum(&mut malformed);
        }
        BadReply::Checksum => malformed[stream_batch::HEADER_BYTES] ^= 1,
        BadReply::EmptyReadReply => {
            malformed = empty_reply(5).to_vec();
            assert_eq!(stream_batch::decode_reply(&malformed, FRAME_LIMIT as usize, COMMAND_LIMIT as usize).unwrap_err(),
                stream_batch::FrameError::Wire { source: statevec_frame::wire::WireError::UnsupportedKind { observed: 5 } });
        }
        BadReply::ProtocolRefused => {
            malformed = stream_batch::IngressRefusal::new(sent_protocol, 1, None, sent_protocol + 2, sent_protocol + 3)
                .encode()
                .to_vec();
        }
        BadReply::UnknownDisposition => {
            let disposition = malformed.len() - statevec_frame::stream_batch_protocol::HANDOVER_ADVICE_BYTES - 2;
            malformed[disposition] = 223;
            refresh_reply_checksum(&mut malformed);
        }
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
            Some(ClientError::BatchRead {
                source: stream_batch::ReadError::Frame { source: stream_batch::FrameError::Wire {
                    source: statevec_frame::wire::WireError::WrongMagic { observed }
                } }
            }) if observed == *b"?VIN"
        )),
        BadReply::Checksum => assert!(matches!(
            diagnostic,
            Some(ClientError::BatchRead {
                source: stream_batch::ReadError::Frame { source: stream_batch::FrameError::Wire {
                    source: statevec_frame::wire::WireError::Checksum { stored, computed }
                } }
            }) if stored != computed
        )),
        BadReply::EmptyReadReply => assert!(matches!(diagnostic, Some(ClientError::BatchRead {
            source: stream_batch::ReadError::Frame { source: stream_batch::FrameError::Wire {
                source: statevec_frame::wire::WireError::UnsupportedKind { observed: 5 }
            } },
        }))),
        BadReply::Body => assert!(matches!(
            diagnostic,
            Some(ClientError::BatchFrame {
                source: stream_batch::FrameError::UnsupportedRefusal { observed: 0 }
            })
        )),
        BadReply::ProtocolRefused => assert!(matches!(
            diagnostic,
            Some(ClientError::BatchFrame {
                source: stream_batch::FrameError::IngressProtocolUnsupported { refusal }
            }) if refusal.observed_protocol == sent_protocol && refusal.minimum_protocol == sent_protocol + 2
        )),
        BadReply::UnknownDisposition => assert!(matches!(
            diagnostic,
            Some(ClientError::BatchFrame {
                source: stream_batch::FrameError::UnrecognizedDisposition { observed: 223, .. }
            })
        )),
    }
    assert_eq!(first.lock().unwrap().drops, 1);
    assert!(client.poll_event().is_none(), "malformed reply is not a session Stop or terminal");
    clock.set(INITIAL_TIME + RETRY);
    drain(&mut driver, &mut client);
    let retried = requests(&successor).pop().unwrap();
    if matches!(fault, BadReply::ProtocolRefused | BadReply::UnknownDisposition) {
        assert!(matches!(retried, Request::Reconcile { .. }));
    }
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

#[test_case::test_case(None; "fresh submit")]
#[test_case::test_case(Some(entry()); "reconcile with a known commitment")]
fn an_unassigned_refusal_suspends_without_resending(committed: Option<EntryBinding>) {
    let clock = Clock::new();
    let mut client = client(&clock);
    let device = bytes();
    let successor = bytes();
    let (mut driver, network) = driver(&[device.clone(), successor.clone()]);
    drain(&mut driver, &mut client);
    let original = batch(1);
    let intent = original.commitment_v1();
    let handle = if committed.is_some() {
        client.try_import(original, committed).unwrap()
    } else {
        client.try_submit(original).unwrap()
    };
    drain(&mut driver, &mut client);
    let sent = requests(&device).pop().unwrap();
    assert_eq!(matches!(sent, Request::Reconcile { .. }), committed.is_some());

    let refused = Reply {
        binding: ReplyBinding::for_request(&sent),
        observation: None,
        disposition: ReplyDisposition::Refused(BatchSubmitRefusal::InternalFailure),
    };
    let mut frame = stream_batch::encode_reply(&refused, FRAME_LIMIT as usize, COMMAND_LIMIT as usize).unwrap();
    let reason = frame.len() - statevec_frame::stream_batch_protocol::HANDOVER_ADVICE_BYTES - 1;
    frame[reason] = u8::MAX;
    refresh_reply_checksum(&mut frame);
    device.lock().unwrap().incoming.extend(frame);
    ready(&mut driver, &mut client);
    drain(&mut driver, &mut client);

    let problem = client.poll_event();
    assert!(
        matches!(problem, Some(BatchEvent::Progress(BatchProgress::Problem { handle: observed, cause }))
            if observed == handle && matches!(*cause, ClientError::ServerRefused {
                reason: BatchSubmitRefusal::Unrecognized { observed: u8::MAX },
            })),
        "the correlated refusal must suspend this batch"
    );
    clock.set(INITIAL_TIME + OPERATION + RETRY);
    drain(&mut driver, &mut client);
    assert_eq!(device.lock().unwrap().drops, 0);
    assert_eq!(network.lock().unwrap().addresses.len(), 1);
    assert!(requests(&device).is_empty() && requests(&successor).is_empty());

    driver.stop(&mut client).unwrap();
    let stopped = client.poll_event();
    assert!(
        matches!(stopped, Some(BatchEvent::Terminal(BatchTerminal::OutcomeUnknown {
            handle: observed, batch, committed: known, cause: ClientError::Stopped, ..
        })) if observed == handle && batch.commitment_v1() == intent && known == committed),
        "Stop returns the same intent and any known commitment"
    );
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
        Ok(BatchDriverStep::Diagnostic { cause: ClientError::BatchWait { source } })
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

#[track_caller]
fn assert_partial_timeout(observed: BatchDriverStep, expected: statevec_frame::wire::WireError) {
    let BatchDriverStep::Diagnostic { cause: ClientError::BatchRead {
        source: stream_batch::ReadError::FrameRead { frame, source },
    } } = observed else { panic!("expected a partial reply at its owner deadline: {observed:?}") };
    assert_eq!(frame, stream_batch::FrameError::Wire { source: expected });
    assert_eq!(source.kind(), io::ErrorKind::TimedOut);
}

#[derive(Clone, Copy)]
enum WriteCompletion {
    BeforeDeadline,
    AtDeadline,
}

#[test_case::test_case(WriteCompletion::BeforeDeadline; "write receipt still permits first-send non-admission")]
#[test_case::test_case(WriteCompletion::AtDeadline; "deadline on the final write observes the partial reply")]
fn a_completed_write_and_partial_reply_arrive_on_the_same_driver_turn(completion: WriteCompletion) {
    let clock = Clock::new();
    let mut client = client(&clock);
    let device = bytes();
    let (mut driver, _) = driver(&[device.clone()]);
    drain(&mut driver, &mut client);
    let first = client.try_submit(batch(1)).unwrap();
    drain(&mut driver, &mut client);
    let original = requests(&device).pop().unwrap();
    let encoded = stream_batch::encode_reply(&completed(&original), FRAME_LIMIT as usize, COMMAND_LIMIT as usize).unwrap();
    // The peer sent its reply twice, with only seven bytes of the duplicate available.
    device.lock().unwrap().incoming.extend(encoded.iter().chain(&encoded[..7]));
    ready(&mut driver, &mut client);
    drain(&mut driver, &mut client);
    assert!(matches!(client.poll_event(), Some(BatchEvent::Terminal(BatchTerminal::Applied { handle, .. })) if handle == first));

    let next_start = InputRef { client_seq: idle_start().client_seq + 1, ..idle_start() };
    let second = client.try_submit(batch_at(next_start, 1)).unwrap();
    assert!(matches!(driver.step(&mut client).unwrap(), BatchDriverStep::Runnable));
    let deadline = INITIAL_TIME + OPERATION;
    let write_time = match completion {
        WriteCompletion::BeforeDeadline => deadline - 1,
        WriteCompletion::AtDeadline => deadline,
    };
    device.lock().unwrap().clock_after_write = Some((clock.clone(), write_time));
    let observed = driver.step(&mut client).unwrap();
    let sent = requests(&device).pop().expect("the second write reached the peer on this turn");
    assert_eq!(sent.batch().commitment_v1(), second.intent);
    match completion {
        WriteCompletion::BeforeDeadline => {
            assert!(matches!(observed, BatchDriverStep::Runnable));
            device.lock().unwrap().incoming.extend(&encoded[7..]);
            enqueue(&device, reply(&sent, ReplyDisposition::Refused(BatchSubmitRefusal::TooLarge)));
            ready(&mut driver, &mut client);
            drain(&mut driver, &mut client);
            assert!(matches!(client.poll_event(), Some(BatchEvent::Terminal(BatchTerminal::NotAdmitted { handle, .. })) if handle == second));
        }
        WriteCompletion::AtDeadline => {
            assert_partial_timeout(observed, statevec_frame::wire::WireError::IncompleteHeader { received: 7 });
            assert!(matches!(driver.step(&mut client).unwrap(), BatchDriverStep::Runnable));
            assert_eq!(device.lock().unwrap().drops, 1);
        }
    }
    assert!(client.poll_event().is_none());
}

#[test]
fn batch_step_reports_the_partial_header_at_its_reply_deadline_and_recovers_the_original_handle() {
    let clock = Clock::new();
    let mut client = client(&clock);
    let old = bytes();
    let next = bytes();
    let (mut driver, _) = driver(&[old.clone(), next.clone()]);
    drain(&mut driver, &mut client);
    let handle = client.try_submit(batch(1)).unwrap();
    drain(&mut driver, &mut client);
    let sent = requests(&old).pop().unwrap();
    let reply = stream_batch::encode_reply(&completed(&sent), FRAME_LIMIT as usize, COMMAND_LIMIT as usize).unwrap();
    old.lock().unwrap().incoming.extend(&reply[..7]);
    ready(&mut driver, &mut client);
    drain(&mut driver, &mut client);
    assert_eq!(old.lock().unwrap().drops, 0);
    clock.set(INITIAL_TIME + OPERATION);

    assert_partial_timeout(driver.step(&mut client).unwrap(), statevec_frame::wire::WireError::IncompleteHeader { received: 7 });
    assert!(client.poll_event().is_none());
    assert!(matches!(driver.step(&mut client).unwrap(), BatchDriverStep::Runnable));
    assert_eq!(old.lock().unwrap().drops, 1);
    drain(&mut driver, &mut client);
    clock.set(INITIAL_TIME + OPERATION + RETRY);
    drain(&mut driver, &mut client);
    let retry = requests(&next).pop().unwrap();
    assert_eq!(retry.batch(), sent.batch());
    assert_ne!(retry.context().request_id, sent.context().request_id);
    enqueue(&next, completed(&retry));
    ready(&mut driver, &mut client);
    drain(&mut driver, &mut client);
    assert!(matches!(client.poll_event(), Some(BatchEvent::Terminal(BatchTerminal::Applied { handle: actual, .. })) if actual == handle));
    assert!(client.poll_event().is_none());
}

fn direct_read_client(clock: &Clock) -> crate::IngressReadOwner {
    let sampled = clock.clone();
    crate::IngressReadOwner::bounded(
        ReadClientSettings::new(
            DeploymentIdentity::new(Digest32::new([11; 32]), Digest32::new([22; 32]), Digest32::new([33; 32])),
            NonZeroU128::new(9).unwrap(),
            endpoints(),
            FRAME_LIMIT,
            crate::ReadSession::Independent,
        )
        .with_retry_millis(RETRY)
        .with_operation_millis(OPERATION),
        move || sampled.sample(),
    ).unwrap()
}

#[test]
fn read_prepare_wait_reports_the_partial_body_at_its_reply_deadline_and_retries_the_same_query() {
    use crate::{ReadEvent, ReadKey, ReadQuery};
    use statevec_frame::read_protocol::{CLIENT_READ_LIMITS, ReadObservation, ReadRole};
    let clock = Clock::new();
    let mut client = direct_read_client(&clock);
    let old = bytes();
    let next = bytes();
    let (mut driver, _) = driver(&[old.clone(), next.clone()]);
    let handle = client.try_read(ReadQuery { keys: vec![ReadKey::SysId { kind: 3, sys_id: 7 }], min_applied_tx_seq: Some(5) }, LIFETIME).unwrap();
    drain(&mut driver, &mut client);
    let request = |device: &Arc<Mutex<Device>>| stream_batch::decode_read_request(&device.lock().unwrap().outgoing, FRAME_LIMIT as usize, CLIENT_READ_LIMITS).unwrap();
    let answer = |request: &stream_batch::ReadRequest| stream_batch::encode_read_reply(
        &request.context, Some(ReadObservation::new(2, 7, 2, 7, None, ReadRole::Follower).unwrap()),
        stream_batch::ReadReplyBody::Answered { applied_tx_seq: 7, tx_chain_hash: Digest32::ZERO, records: &[None] },
        FRAME_LIMIT as usize,
    ).unwrap();
    let sent = request(&old);
    let reply = answer(&sent);
    old.lock().unwrap().incoming.extend(&reply[..stream_batch::HEADER_BYTES + 3]);
    ready(&mut driver, &mut client);
    drain(&mut driver, &mut client);
    clock.set(INITIAL_TIME + OPERATION);

    let (observed, wait) = driver.prepare_wait(&mut client).unwrap();
    assert_partial_timeout(observed, statevec_frame::wire::WireError::IncompleteBody { received: 3, declared: reply.len() - stream_batch::HEADER_BYTES });
    assert_eq!(wait, Some(Duration::ZERO));
    assert!(client.poll_event().is_none());
    assert!(matches!(driver.step(&mut client).unwrap(), BatchDriverStep::Runnable));
    assert_eq!(old.lock().unwrap().drops, 1);
    drain(&mut driver, &mut client);
    clock.set(INITIAL_TIME + OPERATION + RETRY);
    drain(&mut driver, &mut client);
    let retry = request(&next);
    assert_eq!((&retry.keys, retry.min_applied_tx_seq), (&sent.keys, sent.min_applied_tx_seq));
    assert_ne!(retry.context.request_id, sent.context.request_id);
    next.lock().unwrap().incoming.extend(answer(&retry));
    ready(&mut driver, &mut client);
    drain(&mut driver, &mut client);
    assert!(matches!(client.poll_event(), Some(ReadEvent::Answered { handle: actual, .. }) if actual == handle));
    assert!(client.poll_event().is_none());
}

#[test]
fn the_direct_read_connection_and_exact_decoder_refuse_an_empty_batch_reply_at_its_kind() {
    let mut client = direct_read_client(&Clock::new());
    let old = bytes();
    let (mut driver, _) = driver(&[old.clone()]);
    let _ = client.try_read(crate::ReadQuery {
        keys: vec![crate::ReadKey::SysId { kind: 3, sys_id: 7 }], min_applied_tx_seq: None,
    }, LIFETIME).unwrap();
    drain(&mut driver, &mut client);
    let frame = empty_reply(3);
    let expected = stream_batch::FrameError::Wire { source: statevec_frame::wire::WireError::UnsupportedKind { observed: 3 } };
    assert_eq!(stream_batch::decode_read_reply(&frame, FRAME_LIMIT as usize, crate::CLIENT_READ_LIMITS).unwrap_err(), expected);
    old.lock().unwrap().incoming.extend(frame);
    ready(&mut driver, &mut client);
    let BatchDriverStep::Diagnostic { cause: ClientError::BatchRead {
        source: stream_batch::ReadError::Frame { source },
    } } = driver.step(&mut client).unwrap() else { panic!("expected the connection's reply-kind refusal") };
    assert_eq!(source, expected);
    assert!(client.poll_event().is_none());
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
            Ok(BatchDriverStep::Diagnostic { cause: ClientError::BatchWait { source } })
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
        handle: actual, batch: original, cause: ClientError::Stopped, ..
    })) if actual == handle && wire(&original) == wire(sent.batch())));
    assert!(client.poll_event().is_none());
}
