//! Scripted wire observations drive the production client owner. They do not
//! claim real server admission or network I/O.
use super::*;
use ingress_api::stream_batch::{self, Reply, ReplyBinding, ReplyObservation, Request};
use statevec_frame::outcome::EntryDigest;
use test_case::test_case;

#[path = "ut_batch_import.rs"]
mod import;

#[path = "ut_batch_io.rs"]
mod byte_io;

#[path = "ut_batch_driver.rs"]
mod driver;

fn entry() -> EntryBinding {
    EntryBinding {
        index: NonZeroU64::new(21).unwrap(),
        term: NonZeroU64::new(3).unwrap(),
        digest: EntryDigest::from_untrusted_wire([44; 32]),
    }
}

#[track_caller]
fn take_write(client: &mut BatchClient) -> (u64, Request) {
    let (operation_id, request) = take_frame(client);
    assert!(matches!(request, Request::Submit { .. }), "fresh work submits without a preliminary query");
    (operation_id, request)
}

#[track_caller]
fn take_query(client: &mut BatchClient) -> (u64, Request) {
    let (operation_id, request) = take_frame(client);
    assert!(matches!(request, Request::Reconcile { .. }), "known commitment must keep recovery read only");
    (operation_id, request)
}

#[track_caller]
fn take_frame(client: &mut BatchClient) -> (u64, Request) {
    let result = client.drive(ClientEvent::Drive);
    let ClientResult::Write { operation_id, request_id, bytes } = result else {
        panic!("owner must dispatch one complete original request: {result:?}; status={:?}", client.status())
    };
    let request = stream_batch::decode_request(&bytes, FRAME_LIMIT as usize, COMMAND_LIMIT as usize).unwrap();
    assert_eq!(request.context().request_id, request_id);
    assert_eq!(stream_batch::encode_request(&request, FRAME_LIMIT as usize, COMMAND_LIMIT as usize).unwrap(), bytes);
    (operation_id, request)
}

#[track_caller]
fn write_return(client: &mut BatchClient, operation_id: u64, request: &Request) {
    assert!(matches!(
        client.drive(ClientEvent::Written { operation_id, request_id: request.context().request_id, result: Ok(()) }),
        ClientResult::Waiting { .. }
    ));
}

fn reply(request: &Request, disposition: ReplyDisposition) -> Reply {
    let applied = match disposition {
        ReplyDisposition::Completed { .. } | ReplyDisposition::ExactAppliedResultUnavailable(_) => 21,
        _ => 20,
    };
    Reply {
        binding: ReplyBinding::for_request(request),
        // This is a scripted server observation, not client-created authority.
        observation: Some(
            ReplyObservation::new(
                5,
                applied,
                if matches!(disposition, ReplyDisposition::RetryableNext) { 5 } else { 3 },
                None,
                None,
            )
            .unwrap(),
        ),
        disposition,
    }
}

#[track_caller]
fn receive(client: &mut BatchClient, operation_id: u64, reply: Reply) {
    let bytes = stream_batch::encode_reply(&reply, FRAME_LIMIT as usize, COMMAND_LIMIT as usize).unwrap();
    assert!(matches!(
        client.drive(ClientEvent::Read { operation_id, result: Ok(bytes) }),
        ClientResult::Waiting { .. }
    ));
}

fn completed(request: &Request) -> Reply {
    let results = (0..request.batch().commitment_v1().count())
        .map(|offset| {
            MemberResult::Outcome(CommitOutcome {
                applied_by: 2,
                tx_seq: 100 + u64::from(offset),
                tx_id: [51; 32],
                tx_chain_hash: [52; 32],
                raft_index: entry().index.get(),
                raft_term: entry().term.get(),
                sys_status_code: 0,
                biz_status_code: if offset == 0 { 0 } else { 9 },
                emitted_event_count: 0,
                inline_events: Vec::new(),
                inline_events_truncated: false,
            })
        })
        .collect();
    reply(request, ReplyDisposition::Completed { entry: entry(), results })
}

fn exact(request: &Request) -> Reply {
    reply(
        request,
        ReplyDisposition::ExactAppliedResultUnavailable(
            AppliedStreamBatchBindingV1::new(
                request.batch().commitment_v1(),
                entry().index.get(),
                entry().term.get(),
                entry().digest,
                99 + u64::from(request.batch().commitment_v1().count()),
                Digest32::new([52; 32]),
            )
            .unwrap(),
        ),
    )
}

#[derive(Debug, Clone, Copy)]
enum CapacityReply {
    Refused,
    Exact,
    Outcomes,
    Unavailable,
}

fn capacity_reply(request: &Request, shape: CapacityReply) -> Reply {
    match shape {
        CapacityReply::Refused => {
            let mut response = reply(request, ReplyDisposition::Refused(BatchSubmitRefusal::InvalidInput));
            response.observation = None;
            response
        }
        CapacityReply::Exact => exact(request),
        CapacityReply::Outcomes | CapacityReply::Unavailable => {
            let mut response = completed(request);
            let ReplyDisposition::Completed { results, .. } = &mut response.disposition else { unreachable!() };
            for result in results {
                let MemberResult::Outcome(outcome) = result else { unreachable!() };
                match shape {
                    CapacityReply::Unavailable => {
                        *result = MemberResult::Unavailable { tx_seq: NonZeroU64::new(outcome.tx_seq).unwrap() }
                    }
                    _ => {
                        outcome.emitted_event_count = 1;
                        outcome.inline_events.push(statevec_frame::outcome::InlineResponseEvent {
                            tx_seq: outcome.tx_seq,
                            event_seq: 0,
                            event_kind: 1,
                            payload: vec![7],
                        });
                        assert_eq!(outcome.encoded_len(), OUTCOME_LIMIT as usize);
                    }
                }
            }
            response
        }
    }
}

#[test_case(CapacityReply::Refused; "typed refusal")]
#[test_case(CapacityReply::Exact; "exact applied binding")]
#[test_case(CapacityReply::Outcomes; "maximum count with inline outcomes")]
#[test_case(CapacityReply::Unavailable; "maximum count unavailable")]
fn checked_frame_budget_receives_each_terminal_shape(shape: CapacityReply) {
    let mut client = construct_limits(
        (Digest32::new([11; 32]), Digest32::new([22; 32]), Digest32::new([33; 32])),
        COMMAND_LIMIT,
        REPLY_FRAME_BOUND,
        OUTCOME_LIMIT,
        3,
    )
    .unwrap();
    connect(&mut client);
    let handle = client.try_submit(batch_at(idle_start(), COMMAND_LIMIT as usize)).unwrap();
    let (connection, request) = take_write(&mut client);
    write_return(&mut client, connection, &request);
    let response = capacity_reply(&request, shape);
    let bytes = stream_batch::encode_reply(&response, REPLY_FRAME_BOUND as usize, COMMAND_LIMIT as usize).unwrap();
    assert!(matches!(
        client.drive(ClientEvent::Read { operation_id: connection, result: Ok(bytes) }),
        ClientResult::Waiting { .. }
    ));
    assert!(matches!(client.status(), ClientStatus::Batch { occupied: 1, .. }));
    match (shape, client.poll_event()) {
        (
            CapacityReply::Refused,
            Some(BatchEvent::Terminal(BatchTerminal::NotAdmitted {
                handle: returned,
                batch: original,
                cause: ClientFailure::ServerRefused { reason: BatchSubmitRefusal::InvalidInput },
            })),
        ) => {
            assert_eq!(returned, handle);
            assert_eq!(wire(&original), wire(request.batch()));
        }
        (
            CapacityReply::Exact,
            Some(BatchEvent::Terminal(BatchTerminal::ExactAppliedResultUnavailable {
                handle: returned, binding, ..
            })),
        ) => {
            assert_eq!(returned, handle);
            let ReplyDisposition::ExactAppliedResultUnavailable(expected) = response.disposition else {
                unreachable!()
            };
            assert_eq!(binding, expected);
        }
        (
            CapacityReply::Outcomes | CapacityReply::Unavailable,
            Some(BatchEvent::Terminal(BatchTerminal::Applied { handle: returned, entry, results, .. })),
        ) => {
            assert_eq!(returned, handle);
            assert_eq!(ReplyDisposition::Completed { entry, results }, response.disposition);
        }
        other => panic!("legal reply must reach its typed terminal: {other:?}"),
    }
    assert!(matches!(client.status(), ClientStatus::Batch { occupied: 0, .. }));
    assert!(client.poll_event().is_none());
}

#[test]
fn first_submit_and_ordered_terminal_keep_capacity_and_advance_only_on_consumption() {
    let clock = Clock::new();
    let mut client = client(&clock);
    connect(&mut client);
    let original = batch_at(idle_start(), 2);
    let bytes = wire(&original);
    let handle = client.try_submit(original).unwrap();
    assert!(
        matches!(client.status(), ClientStatus::Batch { next_deadline: Some(now), .. } if now.get() == INITIAL_TIME)
    );
    let (connection, request) = take_write(&mut client);
    assert_eq!(wire(request.batch()), bytes);
    assert_eq!(*request.context(), handle.context);
    write_return(&mut client, connection, &request);
    receive(&mut client, connection, reply(&request, ReplyDisposition::IngressAdmitted(entry())));
    receive(&mut client, connection, completed(&request));
    receive(&mut client, connection, completed(&request));
    receive(&mut client, connection, reply(&request, ReplyDisposition::IngressAdmitted(entry())));
    assert!(matches!(client.try_submit(batch(1)).unwrap_err().reason, ClientFailure::Full));
    assert!(matches!(client.status(), ClientStatus::Batch { occupied: 1, .. }));

    let Some(BatchEvent::Terminal(BatchTerminal::Applied { handle: returned, entry: applied, results, .. })) =
        client.poll_event()
    else {
        panic!("terminal must supersede progress without releasing its slot")
    };
    assert_eq!((returned, applied), (handle, entry()));
    assert_eq!(results.iter().map(MemberResult::tx_seq).collect::<Vec<_>>(), [100, 101]);
    assert!(matches!(&results[1], MemberResult::Outcome(outcome) if outcome.biz_status_code == 9));
    assert!(client.poll_event().is_none(), "duplicate terminal cannot be delivered twice");
    assert!(matches!(client.status(), ClientStatus::Batch { occupied: 0, .. }));
    let mut next = idle_start();
    next.client_seq += 2;
    client.try_submit(batch_at(next, 1)).unwrap();
    receive(&mut client, connection, completed(&request));
    assert!(client.poll_event().is_none(), "retired original ID cannot complete its successor");
}

#[test]
fn api_distinguishes_progress_from_terminal_without_releasing_or_resampling() {
    let clock = Clock::new();
    let mut client = client(&clock);
    connect(&mut client);
    clock.set(INITIAL_TIME + RETRY);
    let handle = client.try_submit(batch(1)).unwrap();
    let (connection, request) = take_write(&mut client);
    write_return(&mut client, connection, &request);
    assert_eq!(handle, ReplyBinding::for_request(&request));
    // The public handle is a shared correlation value, not slot authority.
    let mut caller_copy = handle;
    caller_copy.context.request_id = NonZeroU64::new(999).unwrap();
    caller_copy.intent = batch(2).commitment_v1();
    assert_ne!(caller_copy, handle);

    receive(&mut client, connection, reply(&request, ReplyDisposition::IngressAdmitted(entry())));
    *clock.0.lock().unwrap() = Err(ClockReadFailure::OutOfRange);
    let event = client.poll_event().expect("admission progress");
    assert_eq!(event.handle(), handle);
    let BatchEvent::Progress(progress) = event else { panic!("admission is not terminal") };
    assert!(matches!(progress, BatchProgress::IngressAdmitted { handle: observed, .. } if observed == handle));
    assert!(matches!(client.status(), ClientStatus::Batch { occupied: 1, .. }));

    clock.set(INITIAL_TIME + RETRY);
    receive(&mut client, connection, reply(&request, ReplyDisposition::CommittedAwaitingApply(entry())));
    let BatchEvent::Progress(progress) = client.poll_event().unwrap() else { panic!("commit is not terminal") };
    assert!(matches!(progress, BatchProgress::CommittedAwaitingApply { handle: observed, .. } if observed == handle));
    assert!(matches!(client.status(), ClientStatus::Batch { occupied: 1, .. }));

    receive(&mut client, connection, completed(&request));
    assert!(matches!(client.status(), ClientStatus::Batch { occupied: 1, .. }));
    *clock.0.lock().unwrap() = Err(ClockReadFailure::OutOfRange);
    let event = client.poll_event().expect("latched exact terminal");
    assert_eq!(event.handle(), handle);
    let BatchEvent::Terminal(terminal) = event else { panic!("exact application settles the slot") };
    assert!(matches!(terminal, BatchTerminal::Applied { handle: observed, entry: applied, results, .. }
        if observed == handle && applied == entry() && results.len() == 1));
    assert!(matches!(client.status(), ClientStatus::Batch { occupied: 0, .. }));
    assert!(client.poll_event().is_none(), "terminal consumption releases capacity only once");
}

#[test]
fn sole_slot_never_overlaps_a_write_or_releases_an_unconsumed_terminal() {
    let clock = Clock::new();
    let mut client = client(&clock);
    connect(&mut client);
    for stream in 1..=1 {
        client.try_submit(batch(stream)).unwrap();
    }
    let mut requests = Vec::new();
    for stream in 1..=1 {
        let (connection, request) = take_write(&mut client);
        assert_eq!(request.batch().first_input().stream_id, stream);
        assert!(
            matches!(client.drive(ClientEvent::Drive), ClientResult::Waiting { .. }),
            "one exact write remains in physical custody"
        );
        write_return(&mut client, connection, &request);
        requests.push((connection, request));
    }
    for (connection, request) in requests.iter().rev() {
        receive(&mut client, *connection, completed(request));
    }
    assert!(matches!(client.try_submit(batch(1)).unwrap_err().reason, ClientFailure::Full));
    for _ in 0..1 {
        assert!(matches!(client.poll_event(), Some(BatchEvent::Terminal(BatchTerminal::Applied { .. }))));
    }
    assert!(client.poll_event().is_none());
}

#[test]
fn committed_knowledge_survives_late_admission_newer_refusal_and_stop() {
    let clock = Clock::new();
    let mut client = client(&clock);
    connect(&mut client);
    let handle = client.try_submit(batch(1)).unwrap();
    let (connection, request) = take_write(&mut client);
    write_return(&mut client, connection, &request);
    receive(&mut client, connection, reply(&request, ReplyDisposition::CommittedAwaitingApply(entry())));
    receive(&mut client, connection, reply(&request, ReplyDisposition::IngressAdmitted(entry())));
    let mut refusal = reply(&request, ReplyDisposition::Refused(BatchSubmitRefusal::InternalFailure));
    refusal.observation = Some(ReplyObservation::new(50, 21, 3, None, None).unwrap());
    receive(&mut client, connection, refusal);
    assert!(
        matches!(client.poll_event(), Some(BatchEvent::Progress(BatchProgress::CommittedAwaitingApply { entry: observed, .. })) if observed == entry())
    );
    assert!(client.poll_event().is_none());
    assert!(matches!(client.drive(ClientEvent::Stop), ClientResult::Close { .. }));
    let Some(BatchEvent::Terminal(BatchTerminal::OutcomeUnknown {
        handle: returned, batch: original, committed, ..
    })) = client.poll_event()
    else {
        panic!("stop returns unresolved intent")
    };
    assert_eq!(returned, handle);
    assert_eq!(wire(&original), wire(request.batch()));
    assert_eq!(committed, Some(entry()), "late weak replies cannot erase confirmed commitment");
}

#[test]
fn old_term_exact_reply_settles_despite_a_newer_refusal_and_keeps_unknown_results_honest() {
    let clock = Clock::new();
    let mut client = client(&clock);
    connect(&mut client);
    client.try_submit(batch(1)).unwrap();
    let (connection, request) = take_write(&mut client);
    write_return(&mut client, connection, &request);
    receive(&mut client, connection, reply(&request, ReplyDisposition::IngressAdmitted(entry())));
    let mut refusal = reply(&request, ReplyDisposition::Refused(BatchSubmitRefusal::NotReady));
    refusal.observation = Some(ReplyObservation::new(50, 21, 3, None, None).unwrap());
    receive(&mut client, connection, refusal);
    let mut applied = exact(&request);
    applied.observation = Some(ReplyObservation::new(3, 21, 3, None, None).unwrap());
    receive(&mut client, connection, applied);
    assert!(
        matches!(client.poll_event(), Some(BatchEvent::Terminal(BatchTerminal::ExactAppliedResultUnavailable { binding, .. }))
        if binding.intent() == request.batch().commitment_v1() && binding.raft_index() == 21)
    );
    let mut next = idle_start();
    next.client_seq += 1;
    client.try_submit(batch_at(next, 1)).unwrap();
}

#[test]
fn weak_coverage_suspends_without_settling_and_its_problem_survives_total_expiry() {
    let clock = Clock::new();
    let mut client = client(&clock);
    connect(&mut client);
    client.try_submit(batch(1)).unwrap();
    let (connection, request) = take_write(&mut client);
    write_return(&mut client, connection, &request);
    receive(
        &mut client,
        connection,
        reply(&request, ReplyDisposition::ProcessedResultUnavailable { next_sequence: 80 }),
    );
    assert!(
        matches!(client.status(), ClientStatus::Batch { occupied: 1, .. }),
        "range-only coverage must retain live intent custody"
    );
    assert!(matches!(client.drive(ClientEvent::Drive), ClientResult::Waiting { .. }));
    clock.set(INITIAL_TIME + LIFETIME);
    assert!(matches!(client.drive(ClientEvent::Drive), ClientResult::Close { .. }));
    let Some(BatchEvent::Terminal(BatchTerminal::OutcomeUnknown {
        batch: original,
        committed: None,
        cause: ClientFailure::RequestLifetimeExpired,
        problem: Some(problem),
        ..
    })) = client.poll_event()
    else {
        panic!("weak coverage is neither applied nor non-admission")
    };
    assert!(matches!(*problem, ClientFailure::InsufficientAppliedEvidence { next_sequence: 80 }));
    assert_eq!(wire(&original), wire(request.batch()));
}

#[test]
fn consuming_a_weak_coverage_problem_does_not_release_the_original_or_allow_new_input() {
    let clock = Clock::new();
    let mut client = client(&clock);
    connect(&mut client);
    client.try_submit(batch(1)).unwrap();
    let (connection, request) = take_write(&mut client);
    write_return(&mut client, connection, &request);
    receive(
        &mut client,
        connection,
        reply(&request, ReplyDisposition::ProcessedResultUnavailable { next_sequence: 80 }),
    );
    assert!(
        matches!(
            client.poll_event(),
            Some(BatchEvent::Progress(BatchProgress::Problem { cause, .. })) if matches!(*cause, ClientFailure::InsufficientAppliedEvidence { next_sequence: 80 })
        ),
        "range-only evidence must report a problem, never a terminal"
    );
    assert!(matches!(client.status(), ClientStatus::Batch { occupied: 1, .. }));
    assert!(matches!(client.try_submit(batch(1)).unwrap_err().reason, ClientFailure::Full));
    receive(&mut client, connection, completed(&request));
    assert!(matches!(client.poll_event(), Some(BatchEvent::Terminal(BatchTerminal::Applied { .. }))));
}

#[test_case(CapacityReply::Outcomes, false; "completed_before_problem_consumption")]
#[test_case(CapacityReply::Exact, false; "exact_before_problem_consumption")]
#[test_case(CapacityReply::Outcomes, true; "completed_after_problem_consumption")]
#[test_case(CapacityReply::Exact, true; "exact_after_problem_consumption")]
fn contradictory_committed_entry_retains_original_knowledge_and_can_accept_its_exact_terminal(
    terminal: CapacityReply,
    consume_problem: bool,
) {
    let clock = Clock::new();
    let mut client = client(&clock);
    connect(&mut client);
    client.try_submit(batch(1)).unwrap();
    let (connection, request) = take_write(&mut client);
    write_return(&mut client, connection, &request);
    receive(&mut client, connection, reply(&request, ReplyDisposition::CommittedAwaitingApply(entry())));
    let different = EntryBinding { digest: EntryDigest::from_untrusted_wire([77; 32]), ..entry() };
    receive(&mut client, connection, reply(&request, ReplyDisposition::CommittedAwaitingApply(different)));
    let notified = if consume_problem {
        let Some(BatchEvent::Progress(BatchProgress::Problem { cause, .. })) = client.poll_event() else {
            panic!("a second committed identity must not replace the first")
        };
        Some(cause)
    } else {
        None
    };
    // A later problem cannot overwrite the first, regardless of notification.
    let third = EntryBinding { digest: EntryDigest::from_untrusted_wire([88; 32]), ..entry() };
    receive(&mut client, connection, reply(&request, ReplyDisposition::CommittedAwaitingApply(third)));
    receive(&mut client, connection, capacity_reply(&request, terminal));
    let problem = match client.poll_event() {
        Some(BatchEvent::Terminal(BatchTerminal::Applied { entry: known, problem, .. })) => {
            assert_eq!(known, entry());
            problem
        }
        Some(BatchEvent::Terminal(BatchTerminal::ExactAppliedResultUnavailable { binding, problem, .. })) => {
            assert_eq!(binding.entry_digest(), entry().digest);
            problem
        }
        other => panic!("exact evidence must settle the suspended original: {other:?}"),
    };
    let problem = problem.expect("exact settlement must retain the first problem even before notification");
    assert!(matches!(*problem, ClientFailure::ConflictingCommittedEntry { known, observed }
        if known == entry() && observed == different));
    if let Some(notified) = notified {
        assert!(Arc::ptr_eq(&problem, &notified), "terminal keeps the originally notified typed cause");
    }
    assert!(client.poll_event().is_none(), "terminal consumes the sole slot once");
}

#[test_case(BatchSubmitRefusal::TooLarge; "too_large")]
#[test_case(BatchSubmitRefusal::InvalidInput; "invalid_input")]
#[test_case(BatchSubmitRefusal::Unauthorized; "unauthorized")]
#[test_case(BatchSubmitRefusal::ShuttingDown; "shutting_down")]
#[test_case(BatchSubmitRefusal::IncompatibleContext; "incompatible_context")]
fn sole_send_permanent_refusal_returns_original_without_advancing_stream(reason: BatchSubmitRefusal) {
    let clock = Clock::new();
    let mut client = client(&clock);
    connect(&mut client);
    client.try_submit(batch(1)).unwrap();
    let (connection, request) = take_write(&mut client);
    write_return(&mut client, connection, &request);
    let mut refused = reply(&request, ReplyDisposition::Refused(reason));
    if reason == BatchSubmitRefusal::IncompatibleContext {
        refused.observation = None;
    }
    receive(&mut client, connection, refused);
    assert!(matches!(client.try_submit(batch(1)).unwrap_err().reason, ClientFailure::Full));
    let Some(BatchEvent::Terminal(BatchTerminal::NotAdmitted {
        batch: original,
        cause: ClientFailure::ServerRefused { reason: returned },
        ..
    })) = client.poll_event()
    else {
        panic!("sole transmission was expressly refused")
    };
    assert_eq!(returned, reason);
    assert_eq!(wire(&original), wire(request.batch()));
    client.try_submit(batch(1)).unwrap();
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AdmissionHistory {
    NoReply,
    Admitted,
}

#[test_case(AdmissionHistory::NoReply; "sole_gap")]
#[test_case(AdmissionHistory::Admitted; "gap_after_admission")]
fn gap_only_proves_non_admission_without_an_earlier_qualified_admission(history: AdmissionHistory) {
    let clock = Clock::new();
    let mut client = client(&clock);
    connect(&mut client);
    client.try_submit(batch(1)).unwrap();
    let (connection, request) = take_write(&mut client);
    write_return(&mut client, connection, &request);
    if history == AdmissionHistory::Admitted {
        receive(&mut client, connection, reply(&request, ReplyDisposition::IngressAdmitted(entry())));
    }
    let request = if history == AdmissionHistory::Admitted {
        let _ = client.poll_event();
        clock.set(INITIAL_TIME + RETRY);
        let (_, retry) = take_write(&mut client);
        write_return(&mut client, connection, &retry);
        retry
    } else {
        request
    };
    receive(&mut client, connection, reply(&request, ReplyDisposition::StreamGapRejected { expected_seq: 40 }));
    let event = client.poll_event();
    if history == AdmissionHistory::Admitted {
        assert!(matches!(
            event,
            Some(BatchEvent::Progress(BatchProgress::Problem { cause, .. })) if matches!(*cause, ClientFailure::StreamGap { expected_sequence: 40 })
        ));
        assert!(matches!(client.try_submit(batch(1)).unwrap_err().reason, ClientFailure::Full));
    } else {
        assert!(matches!(
            event,
            Some(BatchEvent::Terminal(BatchTerminal::NotAdmitted {
                cause: ClientFailure::StreamGap { expected_sequence: 40 },
                ..
            }))
        ));
        client.try_submit(batch(1)).unwrap();
    }
}

#[test_case(None; "unknown failure without observation")]
#[test_case(Some(ReplyObservation::new(5, 20, 3, None, None).unwrap()); "execution failure without node failure")]
fn internal_failure_suspends_but_cannot_prevent_late_exact_completion(observation: Option<ReplyObservation>) {
    let clock = Clock::new();
    let mut client = client(&clock);
    connect(&mut client);
    client.try_submit(batch(1)).unwrap();
    let (connection, request) = take_write(&mut client);
    write_return(&mut client, connection, &request);
    let mut failure = reply(&request, ReplyDisposition::Refused(BatchSubmitRefusal::InternalFailure));
    failure.observation = observation;
    receive(&mut client, connection, failure);
    assert!(matches!(
        client.poll_event(),
        Some(BatchEvent::Progress(BatchProgress::Problem { cause, .. })) if matches!(*cause, ClientFailure::ServerRefused { reason: BatchSubmitRefusal::InternalFailure })
    ));
    clock.set(INITIAL_TIME + RETRY);
    assert!(
        matches!(client.drive(ClientEvent::Drive), ClientResult::Waiting { wake_at: Some(due) }
        if due.get() == INITIAL_TIME + LIFETIME),
        "execution failure must not become an automatic retry"
    );
    receive(&mut client, connection, completed(&request));
    assert!(matches!(client.poll_event(), Some(BatchEvent::Terminal(BatchTerminal::Applied { .. }))));
}

#[test]
fn old_session_is_ignored_but_wrong_intent_on_current_submit_is_a_typed_problem() {
    let clock = Clock::new();
    let mut client = client(&clock);
    connect(&mut client);
    client.try_submit(batch(1)).unwrap();
    let (connection, request) = take_write(&mut client);
    write_return(&mut client, connection, &request);
    let mut old_session = completed(&request);
    old_session.binding.context.session_incarnation = NonZeroU128::new(8).unwrap();
    receive(&mut client, connection, old_session);
    assert!(client.poll_event().is_none());
    let mut unknown_operation = completed(&request);
    unknown_operation.binding.context.request_id = NonZeroU64::new(request.context().request_id.get() + 1).unwrap();
    receive(&mut client, connection, unknown_operation);
    assert!(client.poll_event().is_none(), "a request never issued by this session is not current");
    let mut foreign_intent = reply(&request, ReplyDisposition::Conflict);
    foreign_intent.binding.intent = batch(2).commitment_v1();
    receive(&mut client, connection, foreign_intent);
    assert!(matches!(
        client.poll_event(),
        Some(BatchEvent::Progress(BatchProgress::Problem { cause, .. })) if matches!(*cause, ClientFailure::BatchReply { source: ReplyRequestFailure::IntentMismatch })
    ));
    receive(&mut client, connection, completed(&request));
    assert!(matches!(client.poll_event(), Some(BatchEvent::Terminal(BatchTerminal::Applied { .. }))));
}

#[derive(Debug, Clone, Copy)]
enum ContextComponent {
    Cluster,
    Genesis,
    ExecutionProfile,
}

#[test_case(ContextComponent::Cluster; "cluster")]
#[test_case(ContextComponent::Genesis; "genesis")]
#[test_case(ContextComponent::ExecutionProfile; "execution_profile")]
fn current_operation_context_mismatch_reports_problem_without_releasing_intent(component: ContextComponent) {
    let clock = Clock::new();
    let mut client = client(&clock);
    connect(&mut client);
    let handle = client.try_submit(batch(1)).unwrap();
    let (connection, request) = take_write(&mut client);
    write_return(&mut client, connection, &request);
    let mut incompatible = reply(&request, ReplyDisposition::Refused(BatchSubmitRefusal::InvalidInput));
    let context = &mut incompatible.binding.context;
    match component {
        ContextComponent::Cluster => context.cluster_identity = Digest32::new([77; 32]),
        ContextComponent::Genesis => context.genesis_identity = Digest32::new([77; 32]),
        ContextComponent::ExecutionProfile => context.execution_profile = Digest32::new([77; 32]),
    }

    receive(&mut client, connection, incompatible);
    assert!(
        matches!(client.poll_event(), Some(BatchEvent::Progress(BatchProgress::Problem {
            handle: returned,
            cause,
        })) if returned == handle && matches!(*cause, ClientFailure::BatchReply { source: ReplyRequestFailure::ContextMismatch })),
        "current operation context corruption must report a typed problem"
    );
    assert!(matches!(client.status(), ClientStatus::Batch { occupied: 1, .. }));
    assert!(matches!(client.drive(ClientEvent::Drive), ClientResult::Waiting { .. }));
    assert!(client.poll_event().is_none(), "problem notification is consumed once, not the batch");
    assert!(matches!(client.drive(ClientEvent::Stop), ClientResult::Close { .. }));
    let Some(BatchEvent::Terminal(BatchTerminal::OutcomeUnknown {
        handle: returned,
        batch: original,
        committed: None,
        ..
    })) = client.poll_event()
    else {
        panic!("protocol failure must retain the original intent, not claim non-admission")
    };
    assert_eq!(returned, handle);
    assert_eq!(wire(&original), wire(request.batch()));
}

#[test]
fn query_only_reply_cannot_authorize_resubmit_or_settle_a_submit() {
    let clock = Clock::new();
    let mut client = client(&clock);
    connect(&mut client);
    client.try_submit(batch(1)).unwrap();
    let (connection, request) = take_write(&mut client);
    write_return(&mut client, connection, &request);
    receive(&mut client, connection, reply(&request, ReplyDisposition::RetryableNext));
    assert!(matches!(
        client.poll_event(),
        Some(BatchEvent::Progress(BatchProgress::Problem { cause, .. })) if matches!(*cause, ClientFailure::BatchReply { source: ReplyRequestFailure::UnexpectedSubmitDisposition })
    ));
    assert!(matches!(client.drive(ClientEvent::Drive), ClientResult::Waiting { .. }));
    assert!(matches!(client.status(), ClientStatus::Batch { occupied: 1, .. }));
}

#[test]
fn terminal_before_write_return_does_not_authorize_an_overlapping_write() {
    let clock = Clock::new();
    let mut client = client(&clock);
    connect(&mut client);
    client.try_submit(batch(1)).unwrap();
    let (connection, request) = take_write(&mut client);
    receive(&mut client, connection, completed(&request));
    assert!(matches!(client.poll_event(), Some(BatchEvent::Terminal(BatchTerminal::Applied { .. }))));
    client
        .try_submit(batch_at(InputRef { client_seq: 42, ..idle_start() }, 1))
        .unwrap();
    assert!(
        matches!(client.drive(ClientEvent::Drive), ClientResult::Waiting { .. }),
        "terminal consumption must not clear physical write custody"
    );
    let wrong_id = NonZeroU64::new(request.context().request_id.get() + 1).unwrap();
    assert!(matches!(
        client.drive(ClientEvent::Written { operation_id: connection, request_id: wrong_id, result: Ok(()) }),
        ClientResult::Waiting { .. }
    ));
    assert!(matches!(client.drive(ClientEvent::Drive), ClientResult::Waiting { .. }));
    write_return(&mut client, connection, &request);
    let (_, next) = take_write(&mut client);
    assert_eq!(next.batch().first_input().client_seq, 42);
}

#[test]
fn exchange_expiry_waits_for_close_then_retries_the_original_without_a_query() {
    let clock = Clock::new();
    let mut client = client(&clock);
    connect(&mut client);
    client.try_submit(batch(1)).unwrap();
    let (connection, request) = take_write(&mut client);
    write_return(&mut client, connection, &request);
    clock.set(INITIAL_TIME + RETRY * 3);
    assert!(
        matches!(client.drive(ClientEvent::Drive), ClientResult::Waiting { wake_at: Some(due) }
        if due.get() == INITIAL_TIME + OPERATION),
        "observation intervals cannot replace an unanswered exchange"
    );
    clock.set(INITIAL_TIME + OPERATION);
    assert!(
        matches!(client.drive(ClientEvent::Drive), ClientResult::Close { operation_id } if operation_id == connection)
    );
    receive(&mut client, connection, completed(&request));
    assert!(client.poll_event().is_none(), "retirement fences the old socket's reply");
    clock.set(INITIAL_TIME + OPERATION + RETRY);
    assert!(matches!(client.drive(ClientEvent::Drive), ClientResult::Waiting { .. }), "physical close is still owed");
    assert!(matches!(client.drive(ClientEvent::Closed { operation_id: connection }), ClientResult::Waiting { .. }));
    let ClientResult::Connect { operation_id: successor, peer_id: 2, .. } = client.drive(ClientEvent::Drive) else {
        panic!("bounded next endpoint")
    };
    assert_ne!(successor, connection);
    assert!(matches!(
        client.drive(ClientEvent::Connected { operation_id: successor, result: Ok(()) }),
        ClientResult::Waiting { .. }
    ));
    let mut stale = completed(&request);
    stale.binding.context.execution_profile = Digest32::new([77; 32]);
    receive(&mut client, connection, stale);
    assert!(client.poll_event().is_none(), "old connection input cannot become a current context failure");
    let (query_connection, query) = take_write(&mut client);
    assert_eq!(query_connection, successor);
    assert_ne!(query.context().request_id, request.context().request_id);
    assert_eq!(wire(query.batch()), wire(request.batch()));
    assert!(matches!(client.drive(ClientEvent::Stop), ClientResult::Close { .. }));
    let Some(BatchEvent::Terminal(BatchTerminal::OutcomeUnknown { batch: original, .. })) = client.poll_event() else {
        panic!("retained original")
    };
    assert_eq!(wire(&original), wire(request.batch()));
}

#[path = "ut_batch_reconcile.rs"]
mod reconcile;

#[test]
fn write_return_is_consumed_even_when_the_bound_clock_fails() {
    let clock = Clock::new();
    let mut client = client(&clock);
    connect(&mut client);
    client.try_submit(batch(1)).unwrap();
    let (connection, request) = take_write(&mut client);
    receive(&mut client, connection, completed(&request));
    *clock.0.lock().unwrap() = Err(ClockReadFailure::OutOfRange);
    assert!(matches!(
        client.drive(ClientEvent::Written {
            operation_id: connection,
            request_id: request.context().request_id,
            result: Ok(())
        }),
        ClientResult::ClockFailed { .. }
    ));
    assert!(
        matches!(
            &client.state,
            ClientState::Batch { connection: BatchConnection::CloseRequired { retry: None, .. }, .. }
        ),
        "clock failure cannot retain a returned write buffer"
    );
    assert!(
        matches!(client.poll_event(), Some(BatchEvent::Terminal(BatchTerminal::Applied { .. }))),
        "latched terminal survives clock failure"
    );
    assert!(
        matches!(client.drive(ClientEvent::Drive), ClientResult::Close { operation_id } if operation_id == connection)
    );
    let _ = client.drive(ClientEvent::Closed { operation_id: connection });
    clock.set(INITIAL_TIME + 1);
    assert!(matches!(client.drive(ClientEvent::Drive), ClientResult::Waiting { wake_at: None }));
}

#[test]
fn malformed_current_frame_retires_the_connection_without_settling_any_intent() {
    let clock = Clock::new();
    let mut client = client(&clock);
    connect(&mut client);
    client.try_submit(batch(1)).unwrap();
    let (connection, _) = take_write(&mut client);
    assert!(matches!(
        client.drive(ClientEvent::Read { operation_id: connection, result: Ok(vec![0; 16]) }),
        ClientResult::ConnectionFault {
            cause: ClientFailure::BatchFrame { source: stream_batch::FrameFailure::InvalidMagic },
            ..
        }
    ));
    assert!(
        matches!(client.drive(ClientEvent::Drive), ClientResult::Close { operation_id } if operation_id == connection)
    );
    assert!(client.poll_event().is_none());
    assert!(matches!(client.status(), ClientStatus::Batch { occupied: 1, .. }));
}

#[derive(Debug, Clone, Copy)]
enum FailedIo {
    Read,
    Write,
}

#[test_case(FailedIo::Read; "read_failed")]
#[test_case(FailedIo::Write; "write_failed")]
fn io_failure_with_unreadable_clock_stops_and_keeps_close_debt(failure: FailedIo) {
    let clock = Clock::new();
    let mut client = client(&clock);
    connect(&mut client);
    client.try_submit(batch(1)).unwrap();
    let (connection, request) = take_write(&mut client);
    *clock.0.lock().unwrap() = Err(ClockReadFailure::OutOfRange);
    let source = std::io::Error::from(std::io::ErrorKind::ConnectionReset);
    let event = match failure {
        FailedIo::Read => {
            ClientEvent::Read { operation_id: connection, result: Err(stream_batch::ReadFailure::Io { source }) }
        }
        FailedIo::Write => ClientEvent::Written {
            operation_id: connection,
            request_id: request.context().request_id,
            result: Err(source),
        },
    };
    let ClientResult::ConnectionFault { operation_id, cause } = client.drive(event) else {
        panic!("consumed I/O failure retains its typed cause")
    };
    assert_eq!(operation_id, connection);
    match (failure, cause) {
        (FailedIo::Read, ClientFailure::BatchRead { source: stream_batch::ReadFailure::Io { source } })
        | (FailedIo::Write, ClientFailure::BatchWrite { source }) => {
            assert_eq!(source.kind(), std::io::ErrorKind::ConnectionReset)
        }
        other => panic!("wrong I/O cause: {other:?}"),
    }
    assert!(
        matches!(client.drive(ClientEvent::Drive), ClientResult::Close { operation_id } if operation_id == connection)
    );
    assert!(matches!(client.drive(ClientEvent::Closed { operation_id: connection }), ClientResult::ClockFailed { .. }));
    clock.set(INITIAL_TIME + 1);
    assert!(matches!(client.drive(ClientEvent::Drive), ClientResult::Waiting { wake_at: None }));
    assert!(matches!(
        client.poll_event(),
        Some(BatchEvent::Terminal(BatchTerminal::OutcomeUnknown { cause: ClientFailure::InvalidTime { .. }, .. }))
    ));
    assert!(matches!(client.status(), ClientStatus::Batch { stopped: true, occupied: 0, .. }));
}

#[test]
fn write_completion_at_its_deadline_retires_socket_without_changing_an_already_consumed_terminal() {
    let clock = Clock::new();
    let mut client = client(&clock);
    connect(&mut client);
    client.try_submit(batch(1)).unwrap();
    let (connection, request) = take_write(&mut client);
    receive(&mut client, connection, completed(&request));
    assert!(matches!(client.poll_event(), Some(BatchEvent::Terminal(BatchTerminal::Applied { .. }))));
    clock.set(INITIAL_TIME + OPERATION);
    assert!(matches!(client.drive(ClientEvent::Written {
        operation_id: connection, request_id: request.context().request_id, result: Ok(()),
    }), ClientResult::Close { operation_id } if operation_id == connection));
    assert!(client.poll_event().is_none());
    assert!(matches!(client.status(), ClientStatus::Batch { occupied: 0, .. }));
}

#[test]
fn one_client_expiry_preserves_independent_latched_terminal_and_committed_custody() {
    let clock = Clock::new();
    let mut expired = client(&clock);
    let connection = connect(&mut expired);
    expired.try_submit(batch(1)).unwrap();
    let (_, first) = take_write(&mut expired);
    write_return(&mut expired, connection, &first);
    receive(&mut expired, connection, reply(&first, ReplyDisposition::IngressAdmitted(entry())));
    clock.set(INITIAL_TIME + LIFETIME / 2);
    let mut completed_client =
        construct(&clock, InputRef { stream_id: 2, ..idle_start() }, endpoints(), (RETRY, OPERATION, LIFETIME))
            .unwrap();
    let connection2 = connect(&mut completed_client);
    let terminal = completed_client.try_submit(batch(2)).unwrap();
    let (_, second) = take_write(&mut completed_client);
    write_return(&mut completed_client, connection2, &second);
    receive(&mut completed_client, connection2, completed(&second));
    let mut committed_client =
        construct(&clock, InputRef { stream_id: 3, ..idle_start() }, endpoints(), (RETRY, OPERATION, LIFETIME))
            .unwrap();
    let connection3 = connect(&mut committed_client);
    let committed = committed_client.try_submit(batch(3)).unwrap();
    let (_, third) = take_write(&mut committed_client);
    write_return(&mut committed_client, connection3, &third);
    receive(&mut committed_client, connection3, reply(&third, ReplyDisposition::CommittedAwaitingApply(entry())));
    clock.set(INITIAL_TIME + LIFETIME);
    assert!(matches!(expired.drive(ClientEvent::Drive), ClientResult::Close { .. }));
    assert!(matches!(
        expired.poll_event(),
        Some(BatchEvent::Terminal(BatchTerminal::OutcomeUnknown { committed: None, .. }))
    ));
    assert!(
        matches!(completed_client.poll_event(), Some(BatchEvent::Terminal(BatchTerminal::Applied { handle, .. })) if handle == terminal)
    );
    let (_, query) = take_query(&mut committed_client);
    write_return(&mut committed_client, connection3, &query);
    receive(&mut committed_client, connection3, exact(&query));
    assert!(
        matches!(committed_client.poll_event(), Some(BatchEvent::Terminal(BatchTerminal::ExactAppliedResultUnavailable { handle, .. })) if handle == committed)
    );
}

#[test_case(false, false; "drive_read_failure_with_known_commitment")]
#[test_case(false, true; "drive_regression_with_known_commitment")]
#[test_case(true, false; "wait_read_failure_after_latched_terminal")]
#[test_case(true, true; "wait_regression_after_latched_terminal")]
fn drive_clock_failure_terminates_without_discarding_known_facts(terminal: bool, regressed: bool) {
    let clock = Clock::new();
    let mut client = client(&clock);
    connect(&mut client);
    client.try_submit(batch(1)).unwrap();
    let (connection, request) = take_write(&mut client);
    write_return(&mut client, connection, &request);
    receive(&mut client, connection, reply(&request, ReplyDisposition::CommittedAwaitingApply(entry())));
    let _ = client.poll_event();
    if terminal {
        receive(&mut client, connection, completed(&request));
    }
    *clock.0.lock().unwrap() =
        if regressed { Ok(MonotonicMillis::new(INITIAL_TIME - 1)) } else { Err(ClockReadFailure::OutOfRange) };
    assert!(matches!(
        client.drive(ClientEvent::Drive),
        ClientResult::ClockFailed { cause: ClientFailure::InvalidTime { .. } | ClientFailure::TimeRegressed { .. } }
    ));
    assert!(
        matches!(client.status(), ClientStatus::Batch { stopped: true, occupied: 1, .. }),
        "drive clock failure must stop with custody retained"
    );
    match client.poll_event().unwrap() {
        BatchEvent::Terminal(BatchTerminal::Applied { entry: actual, .. }) if terminal => assert_eq!(actual, entry()),
        BatchEvent::Terminal(BatchTerminal::OutcomeUnknown { batch: original, committed, cause, .. }) if !terminal => {
            assert_eq!(wire(&original), wire(&batch(1)));
            assert_eq!(committed, Some(entry()));
            assert!(matches!(cause, ClientFailure::InvalidTime { .. } | ClientFailure::TimeRegressed { .. }));
        }
        other => panic!("latched facts must survive: {other:?}"),
    }
    assert!(
        matches!(client.drive(ClientEvent::Drive), ClientResult::Close { operation_id } if operation_id == connection)
    );
    let _ = client.drive(ClientEvent::Closed { operation_id: connection });
    clock.set(INITIAL_TIME + RETRY);
    assert!(matches!(client.drive(ClientEvent::Drive), ClientResult::Waiting { wake_at: None }));
    assert!(client.poll_event().is_none());
}

#[test_case(false; "submit_operation_deadline")]
#[test_case(true; "observation_retry_deadline")]
fn runtime_deadline_overflow_stops_original_without_an_extra_exchange(retry: bool) {
    let clock = Clock::new();
    clock.set(u64::MAX - LIFETIME);
    let mut client = client(&clock);
    let connection = connect(&mut client);
    client.try_import(batch(1), None).unwrap();
    if retry {
        clock.set(u64::MAX - OPERATION);
        let (_, request) = take_write(&mut client);
        write_return(&mut client, connection, &request);
        clock.set(u64::MAX - 1);
        let response = reply(&request, ReplyDisposition::Refused(BatchSubmitRefusal::Busy));
        let bytes = stream_batch::encode_reply(&response, FRAME_LIMIT as usize, COMMAND_LIMIT as usize).unwrap();
        assert!(matches!(
            client.drive(ClientEvent::Read { operation_id: connection, result: Ok(bytes) }),
            ClientResult::Close { .. }
        ));
    } else {
        clock.set(u64::MAX - 1);
        assert!(matches!(client.drive(ClientEvent::Drive), ClientResult::ClockFailed { .. }));
    }
    assert!(
        matches!(client.status(), ClientStatus::Batch { stopped: true, occupied: 1, .. }),
        "deadline overflow terminates, never suspends an original"
    );
    assert!(matches!(
        client.poll_event(),
        Some(BatchEvent::Terminal(BatchTerminal::OutcomeUnknown { cause: ClientFailure::InvalidTime { .. }, .. }))
    ));
}

#[derive(Clone, Copy)]
enum RetryOverflowCut {
    ReadFailure,
    FailedConnect,
}

#[test_case(RetryOverflowCut::ReadFailure; "read_failure_requires_close")]
#[test_case(RetryOverflowCut::FailedConnect; "failed_connect_already_disposed")]
fn retry_deadline_overflow_returns_original_and_commitment_before_lifetime(cut: RetryOverflowCut) {
    let clock = Clock::new();
    clock.set(u64::MAX - LIFETIME);
    let mut client = client(&clock);
    let connection = connect(&mut client);
    let handle = client.try_import(batch(1), Some(entry())).unwrap();
    if matches!(cut, RetryOverflowCut::FailedConnect) {
        client.drive(ClientEvent::Read {
            operation_id: connection,
            result: Err(stream_batch::ReadFailure::Io { source: std::io::ErrorKind::ConnectionReset.into() }),
        });
        assert!(matches!(client.drive(ClientEvent::Drive), ClientResult::Close { .. }));
        client.drive(ClientEvent::Closed { operation_id: connection });
        clock.set(u64::MAX - OPERATION);
        let ClientResult::Connect { operation_id, .. } = client.drive(ClientEvent::Drive) else {
            panic!("retry starts while its operation deadline still fits")
        };
        clock.set(u64::MAX - 1); // Lifetime is MAX; only the next retry addition overflows.
        assert!(matches!(client.drive(ClientEvent::Connected {
            operation_id, result: Err(std::io::ErrorKind::ConnectionRefused.into()),
        }), ClientResult::ConnectionFailed { source, next_wake: None, .. }
            if source.kind() == std::io::ErrorKind::ConnectionRefused));
    } else {
        clock.set(u64::MAX - 1);
        assert!(matches!(
            client.drive(ClientEvent::Read {
                operation_id: connection,
                result: Err(stream_batch::ReadFailure::Io { source: std::io::ErrorKind::ConnectionReset.into() }),
            }),
            ClientResult::ConnectionFault { .. }
        ));
    }

    assert!(
        matches!(client.poll_event(), Some(BatchEvent::Terminal(BatchTerminal::OutcomeUnknown {
        handle: actual, batch: original, committed: Some(known),
        cause: ClientFailure::InvalidTime { source: ClockReadFailure::OutOfRange }, ..
    })) if actual == handle && wire(&original) == wire(&batch(1)) && known == entry()),
        "retry overflow must terminate original custody with its known commitment"
    );
    if matches!(cut, RetryOverflowCut::ReadFailure) {
        assert!(
            matches!(client.drive(ClientEvent::Drive), ClientResult::Close { operation_id } if operation_id == connection)
        );
        client.drive(ClientEvent::Closed { operation_id: connection });
    }
    assert!(matches!(client.status(), ClientStatus::Batch { stopped: true, occupied: 0, next_deadline: None, .. }));
    assert!(matches!(client.drive(ClientEvent::Drive), ClientResult::Waiting { wake_at: None }));
    assert!(client.poll_event().is_none(), "termination is consumed exactly once");
}
