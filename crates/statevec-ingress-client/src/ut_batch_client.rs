//! Deterministic traces through the actual owner; the double supplies only time
//! and physical connection results. No fabricated terminal or alternate owner.
use super::*;
use statevec_frame::EngineCommandPayload;
use statevec_frame::outcome::CommitOutcome;
use std::sync::{Arc, Mutex};

#[path = "ut_batch_reply.rs"]
mod replies;

const FRAME_LIMIT: u32 = 4096;
const COMMAND_LIMIT: u32 = 16;
const OUTCOME_LIMIT: u32 = 128;
const REPLY_FRAME_BOUND: u32 = (statevec_frame::stream_batch_protocol::REPLY_BASE_BYTES
    + statevec_frame::stream_batch_protocol::ENTRY_BINDING_BYTES) as u32
    + COMMAND_LIMIT * (OUTCOME_LIMIT + 1);
const RETRY: u64 = 10;
const OPERATION: u64 = 100;
const LIFETIME: u64 = 1000;
const INITIAL_TIME: u64 = 500;
const UNAVAILABLE_LIMIT: u32 = 3;

#[derive(Clone)]
struct Clock(Arc<Mutex<Result<MonotonicMillis, ClockReadFailure>>>);
impl Clock {
    fn new() -> Self {
        Self(Arc::new(Mutex::new(Ok(MonotonicMillis::new(INITIAL_TIME)))))
    }
    fn sample(&self) -> Result<MonotonicMillis, ClockReadFailure> {
        match self.0.lock() {
            Ok(now) => *now,
            // The wrapper unwind tests poison this physical callback on
            // purpose. Resume the fault without obscuring the typed oracle.
            Err(_) => std::panic::resume_unwind(Box::new("injected clock callback failure")),
        }
    }
    fn set(&self, now: u64) {
        *self.0.lock().unwrap() = Ok(MonotonicMillis::new(now));
    }
}

fn idle_start() -> InputRef {
    InputRef { client_id: 7, stream_id: 1, client_seq: 41, request_id: None }
}
fn endpoints() -> Vec<(u64, SocketAddr)> {
    vec![(1, "127.0.0.1:12001".parse().unwrap()), (2, "127.0.0.1:12002".parse().unwrap())]
}
fn construct(
    clock: &Clock,
    starts: InputRef,
    endpoints: Vec<(u64, SocketAddr)>,
    budgets: (u64, u64, u64),
) -> Result<BatchClient, ClientFailure> {
    let clock = clock.clone();
    BatchClient::bounded(
        (Digest32::new([11; 32]), Digest32::new([22; 32]), Digest32::new([33; 32])),
        NonZeroU128::new(9).unwrap(),
        starts,
        endpoints,
        COMMAND_LIMIT,
        FRAME_LIMIT,
        OUTCOME_LIMIT,
        budgets.0,
        budgets.1,
        budgets.2,
        UNAVAILABLE_LIMIT,
        move || clock.sample(),
    )
}
fn client(clock: &Clock) -> BatchClient {
    construct(clock, idle_start(), endpoints(), (RETRY, OPERATION, LIFETIME)).unwrap()
}
fn connect(client: &mut BatchClient) -> u64 {
    let ClientResult::Connect { operation_id, peer_id: 1, address } = client.drive(ClientEvent::Drive) else {
        panic!("initial connection effect")
    };
    assert_eq!(address, endpoints()[0].1);
    assert!(matches!(
        client.drive(ClientEvent::Connected { operation_id, result: Ok(()) }),
        ClientResult::Waiting { .. }
    ));
    operation_id
}
fn batch(stream: u32) -> ClientStreamBatch {
    let mut first = idle_start();
    first.stream_id = stream;
    batch_at(first, 1)
}
fn batch_at(first: InputRef, count: usize) -> ClientStreamBatch {
    let commands = (0..count)
        .map(|offset| {
            // Opaque application input. Sequence-dependent bytes expose any
            // accidental substitution without linking a business runtime.
            EngineCommandPayload {
                payload_codec: statevec_frame::PayloadCodec { codec_id: "runtime_binary_v0".into(), codec_version: 0 },
                payload_bytes: (first.client_seq + offset as u64).to_le_bytes().to_vec().into(),
            }
        })
        .collect();
    ClientStreamBatch::try_new(first, commands, FRAME_LIMIT as usize, COMMAND_LIMIT as usize).unwrap()
}
fn wire(batch: &ClientStreamBatch) -> Vec<u8> {
    batch.encode_v1(FRAME_LIMIT as usize, COMMAND_LIMIT as usize).unwrap()
}

#[derive(Clone, Copy, Debug)]
enum InputCustody {
    Submit,
    Import,
}

#[derive(Clone, Copy, Debug)]
enum SpareAllocation {
    Commands,
    Codec,
}

#[test_case::test_case(InputCustody::Submit, SpareAllocation::Commands; "submit_command_capacity")]
#[test_case::test_case(InputCustody::Import, SpareAllocation::Commands; "import_command_capacity")]
#[test_case::test_case(InputCustody::Submit, SpareAllocation::Codec; "submit_codec_capacity")]
#[test_case::test_case(InputCustody::Import, SpareAllocation::Codec; "import_codec_capacity")]
fn accepted_input_bounds_backing_allocations_without_changing_intent(custody: InputCustody, spare: SpareAllocation) {
    let (first, mut commands) = batch(1).into_parts();
    match spare {
        SpareAllocation::Commands => commands.reserve_exact(65_536),
        SpareAllocation::Codec => commands[0].payload_codec.codec_id.to_mut().reserve_exact(65_536),
    }
    let payload = Arc::clone(&commands[0].payload_bytes);
    let original = ClientStreamBatch::try_new(first, commands, FRAME_LIMIT as usize, COMMAND_LIMIT as usize).unwrap();
    let expected = wire(&original);
    let commitment = original.commitment_v1();
    let clock = Clock::new();
    let mut client = client(&clock);
    connect(&mut client);
    match custody {
        InputCustody::Submit => client.try_submit(original),
        InputCustody::Import => client.try_import(original, None),
    }
    .unwrap();
    assert!(matches!(client.drive(ClientEvent::Stop), ClientResult::Close { .. }));
    let Some(BatchEvent::Terminal(BatchTerminal::OutcomeUnknown { batch: returned, .. })) = client.poll_event() else {
        panic!("stop must return accepted original input")
    };
    assert_eq!(wire(&returned), expected);
    assert_eq!(returned.commitment_v1(), commitment);
    let (_, commands) = returned.into_parts();
    assert!(Arc::ptr_eq(&commands[0].payload_bytes, &payload), "normalization must not copy payload bytes");
    assert_eq!(commands.capacity(), commands.len(), "accepted commands must not retain spare allocation");
    for command in commands {
        // The only accepted identity is canonicalized to its borrowed constant.
        assert_eq!(command.payload_codec.retained_id_bytes(), 0, "accepted codec must not retain an allocation");
    }
}

#[test]
fn construction_bounds_endpoint_backing_allocation() {
    let mut peers = endpoints();
    peers.reserve_exact(65_536);
    let client = construct(&Clock::new(), idle_start(), peers, (RETRY, OPERATION, LIFETIME)).unwrap();
    let ClientState::Batch { endpoints: retained, .. } = &client.state;
    assert_eq!(retained, &endpoints());
    assert_eq!(retained.capacity(), retained.len(), "configured endpoints must not retain spare allocation");
}

#[test]
fn sole_slot_retains_intact_input_until_explicit_stop_and_terminal_consumption() {
    let clock = Clock::new();
    let mut client = client(&clock);
    let operation_id = connect(&mut client);
    let original = wire(&batch(1));
    let expected = client.try_submit(batch(1)).unwrap();
    let refused = client.try_submit(batch(1)).unwrap_err();
    assert!(matches!(refused.reason, ClientFailure::Full));
    assert_eq!(wire(&refused.batch), original);
    assert!(client.poll_event().is_none(), "local acceptance is not a result");
    assert!(matches!(client.drive(ClientEvent::Stop), ClientResult::Close { operation_id: id } if id == operation_id));
    assert!(
        matches!(client.status(), ClientStatus::Batch { occupied: 1, stopped: true, .. }),
        "terminal retains the sole reservation until consumption"
    );
    let Some(BatchEvent::Terminal(BatchTerminal::OutcomeUnknown { handle, batch: returned, committed, cause, .. })) =
        client.poll_event()
    else {
        panic!("expected retained original")
    };
    assert_eq!(handle, expected);
    assert_eq!(wire(&returned), original);
    assert_eq!(committed, None);
    assert!(matches!(cause, ClientFailure::Stopped));
    assert!(matches!(client.status(), ClientStatus::Batch { occupied: 0, .. }));
    assert!(client.poll_event().is_none());
    assert!(
        matches!(client.drive(ClientEvent::Stop), ClientResult::Waiting { .. }),
        "stop must not issue a second close"
    );
    assert!(matches!(client.try_submit(batch(1)).unwrap_err().reason, ClientFailure::Stopped));
}

#[test]
fn fixed_stream_refusals_preserve_bytes_and_do_not_consume_request_ids() {
    let clock = Clock::new();
    let mut client = client(&clock);
    connect(&mut client);
    for foreign in [batch(2), batch_at(InputRef { client_id: 8, ..idle_start() }, 1)] {
        let original = wire(&foreign);
        let refused = client.try_submit(foreign).unwrap_err();
        assert!(matches!(refused.reason, ClientFailure::InvalidInput));
        assert_eq!(wire(&refused.batch), original);
    }
    let first = client.try_submit(batch(1)).unwrap();
    assert_eq!(first.context.request_id.get(), 1);
    let refused = client.try_submit(batch(1)).unwrap_err();
    assert!(matches!(refused.reason, ClientFailure::Full));
    assert_eq!(refused.batch.commitment_v1(), first.intent);
    assert!(matches!(client.status(), ClientStatus::Batch { occupied: 1, .. }));
}

#[test]
fn idle_clock_sample_starts_lifetime_and_expiry_isolated_to_its_client() {
    let clock = Clock::new();
    let mut first_client = client(&clock);
    connect(&mut first_client);
    let after_idle = INITIAL_TIME + LIFETIME * 10;
    clock.set(after_idle);
    let first = first_client.try_submit(batch(1)).unwrap();
    assert!(
        matches!(&first_client.state, ClientState::Batch { slot, .. }
        if matches!(slot, Some(BatchSlot::Pending { deadline, .. }) if deadline.get() == after_idle + LIFETIME)),
        "acceptance deadline uses this call's clock sample"
    );
    clock.set(after_idle + LIFETIME - 1);
    let mut independent =
        construct(&clock, InputRef { stream_id: 2, ..idle_start() }, endpoints(), (RETRY, OPERATION, LIFETIME))
            .unwrap();
    connect(&mut independent);
    independent.try_submit(batch(2)).unwrap();
    clock.set(after_idle + LIFETIME);
    assert!(matches!(first_client.drive(ClientEvent::Drive), ClientResult::Close { .. }));
    assert!(matches!(first_client.status(), ClientStatus::Batch { occupied: 1, stopped: true, .. }));
    let Some(BatchEvent::Terminal(BatchTerminal::OutcomeUnknown { handle, cause, .. })) = first_client.poll_event()
    else {
        panic!("expected retained original")
    };
    assert_eq!(handle, first);
    assert!(matches!(cause, ClientFailure::RequestLifetimeExpired));
    assert!(first_client.poll_event().is_none());
    assert!(
        matches!(independent.drive(ClientEvent::Drive), ClientResult::Write { .. }),
        "independent client continues"
    );
    assert!(matches!(independent.status(), ClientStatus::Batch { occupied: 1, stopped: false, .. }));
}

#[test]
fn acceptance_time_expiry_keeps_unissued_close_runnable_until_physical_retirement() {
    let clock = Clock::new();
    let mut client = client(&clock);
    let operation_id = connect(&mut client);
    let handle = client.try_submit(batch(1)).unwrap();
    clock.set(INITIAL_TIME + LIFETIME);
    let refusal = client.try_submit(batch(2)).unwrap_err();
    assert!(matches!(refusal.reason, ClientFailure::Stopped));
    assert_eq!(wire(&refusal.batch), wire(&batch(2)));
    assert!(
        matches!(client.status(), ClientStatus::Batch {
        occupied: 1, stopped: true, next_deadline: Some(due), ..
    } if due == clock.sample().unwrap()),
        "unissued close must remain immediately runnable"
    );

    let BatchEvent::Terminal(BatchTerminal::OutcomeUnknown { handle: returned, batch: original, cause, .. }) =
        client.poll_event().unwrap()
    else {
        panic!("expected retained original")
    };
    assert_eq!(returned, handle);
    assert_eq!(wire(&original), wire(&batch(1)));
    assert!(matches!(cause, ClientFailure::RequestLifetimeExpired));
    assert!(
        matches!(client.status(), ClientStatus::Batch {
        occupied: 0, next_deadline: Some(due), ..
    } if due == clock.sample().unwrap()),
        "consuming intent does not retire the socket"
    );
    assert!(matches!(client.drive(ClientEvent::Drive),
        ClientResult::Close { operation_id: id } if id == operation_id));
    assert!(matches!(client.drive(ClientEvent::Stop), ClientResult::Waiting { wake_at: None }));
    assert!(matches!(client.drive(ClientEvent::Closed { operation_id }), ClientResult::Waiting { wake_at: None }));
    assert!(client.poll_event().is_none());
}

#[test]
fn acceptance_time_failure_stops_existing_custody_and_returns_the_refused_input() {
    for sample in [
        Err(ClockReadFailure::OutOfRange),
        Ok(MonotonicMillis::new(INITIAL_TIME - 1)),
        Ok(MonotonicMillis::new(u64::MAX)),
    ] {
        let clock = Clock::new();
        let mut client = client(&clock);
        let operation_id = connect(&mut client);
        let handle = client
            .try_import(
                batch(1),
                Some(EntryBinding {
                    index: NonZeroU64::new(7).unwrap(),
                    term: NonZeroU64::new(2).unwrap(),
                    digest: statevec_frame::outcome::EntryDigest::from_untrusted_wire([9; 32]),
                }),
            )
            .unwrap();
        *clock.0.lock().unwrap() = sample;
        let refusal = client.try_submit(batch(2)).unwrap_err();
        assert!(matches!(refusal.reason, ClientFailure::InvalidTime { .. } | ClientFailure::TimeRegressed { .. }));
        assert_eq!(wire(&refusal.batch), wire(&batch(2)), "refused input stays intact");
        let Some(BatchEvent::Terminal(BatchTerminal::OutcomeUnknown {
            handle: actual,
            batch: original,
            committed,
            cause,
            ..
        })) = client.poll_event()
        else {
            panic!("time failure returns original custody")
        };
        assert_eq!(actual, handle);
        assert_eq!(wire(&original), wire(&batch(1)));
        assert_eq!(
            committed,
            Some(EntryBinding {
                index: NonZeroU64::new(7).unwrap(),
                term: NonZeroU64::new(2).unwrap(),
                digest: statevec_frame::outcome::EntryDigest::from_untrusted_wire([9; 32])
            })
        );
        assert!(matches!(cause, ClientFailure::InvalidTime { .. } | ClientFailure::TimeRegressed { .. }));
        assert!(
            matches!(client.drive(ClientEvent::Drive), ClientResult::Close { operation_id: same } if same == operation_id)
        );
        let _ = client.drive(ClientEvent::Closed { operation_id });
        clock.set(u64::MAX);
        assert!(
            matches!(client.drive(ClientEvent::Drive), ClientResult::Waiting { wake_at: None }),
            "restoring time cannot revive a stopped session"
        );
        assert!(client.poll_event().is_none());
    }
}

#[test]
fn disconnected_and_unregistered_input_cannot_reserve_a_slot() {
    let clock = Clock::new();
    let mut client = client(&clock);
    let original = wire(&batch(1));
    let refusal = client.try_submit(batch(1)).unwrap_err();
    assert!(matches!(refusal.reason, ClientFailure::NotConnected));
    assert_eq!(wire(&refusal.batch), original);
    connect(&mut client);
    assert!(matches!(client.try_submit(batch(99)).unwrap_err().reason, ClientFailure::InvalidInput));
    assert_eq!(client.try_submit(batch(1)).unwrap().context.request_id.get(), 1);
}

#[test]
fn connect_timeout_waits_for_physical_close_and_old_observations_cannot_install_successor() {
    let clock = Clock::new();
    let mut client = client(&clock);
    let ClientResult::Connect { operation_id: old, .. } = client.drive(ClientEvent::Drive) else { panic!("connect") };
    clock.set(INITIAL_TIME + OPERATION);
    assert!(matches!(client.drive(ClientEvent::Drive), ClientResult::Close { operation_id } if operation_id == old));
    assert!(matches!(
        client.drive(ClientEvent::Connected { operation_id: old, result: Ok(()) }),
        ClientResult::Waiting { wake_at: None }
    ));
    clock.set(INITIAL_TIME + OPERATION + RETRY);
    assert!(matches!(client.drive(ClientEvent::Drive), ClientResult::Waiting { .. }));
    assert!(matches!(client.try_submit(batch(1)).unwrap_err().reason, ClientFailure::NotConnected));
    assert!(matches!(client.drive(ClientEvent::Closed { operation_id: old }), ClientResult::Waiting { .. }));
    let ClientResult::Connect { operation_id: current, peer_id: 2, .. } = client.drive(ClientEvent::Drive) else {
        panic!("next endpoint only after close")
    };
    assert_ne!(old, current);
    assert!(matches!(
        client.drive(ClientEvent::Connected { operation_id: old, result: Ok(()) }),
        ClientResult::Waiting { .. }
    ));
    assert!(matches!(client.try_submit(batch(1)).unwrap_err().reason, ClientFailure::NotConnected));
    assert!(matches!(
        client.drive(ClientEvent::Connected { operation_id: current, result: Ok(()) }),
        ClientResult::Waiting { .. }
    ));
    assert!(matches!(client.drive(ClientEvent::Closed { operation_id: old }), ClientResult::Waiting { .. }));
    client.try_submit(batch(1)).unwrap();
}

#[test_case::test_case(BATCH_CLIENT_RETRY_MIN_MS, BATCH_CLIENT_OPERATION_MIN_MS, BATCH_CLIENT_LIFETIME_MIN_MS; "minimum")]
#[test_case::test_case(BATCH_CLIENT_RETRY_MAX_MS, BATCH_CLIENT_OPERATION_MAX_MS, BATCH_CLIENT_LIFETIME_MAX_MS; "maximum")]
#[test_case::test_case(BATCH_CLIENT_RETRY_DEFAULT_MS, BATCH_CLIENT_OPERATION_DEFAULT_MS, BATCH_CLIENT_LIFETIME_DEFAULT_MS; "defaults")]
fn time_budget_boundaries_are_accepted(retry: u64, operation: u64, lifetime: u64) {
    assert!(construct(&Clock::new(), idle_start(), endpoints(), (retry, operation, lifetime),).is_ok());
}

#[test_case::test_case(0, OPERATION, LIFETIME; "retry zero")]
#[test_case::test_case(BATCH_CLIENT_RETRY_MAX_MS + 1, OPERATION, LIFETIME; "retry above cap")]
#[test_case::test_case(RETRY, 0, LIFETIME; "operation zero")]
#[test_case::test_case(RETRY, BATCH_CLIENT_OPERATION_MAX_MS + 1, BATCH_CLIENT_LIFETIME_MAX_MS; "operation above cap")]
#[test_case::test_case(RETRY, OPERATION, 0; "lifetime zero")]
#[test_case::test_case(RETRY, OPERATION, BATCH_CLIENT_LIFETIME_MAX_MS + 1; "lifetime above cap")]
#[test_case::test_case(RETRY, LIFETIME + 1, LIFETIME; "operation exceeds lifetime")]
#[test_case::test_case(LIFETIME, OPERATION, LIFETIME; "retry consumes lifetime")]
fn invalid_time_configuration_is_refused(retry: u64, operation: u64, lifetime: u64) {
    assert!(matches!(
        construct(&Clock::new(), idle_start(), endpoints(), (retry, operation, lifetime),),
        Err(ClientFailure::InvalidLimit)
    ));
}

#[test]
fn constructor_checks_streams_endpoints_and_unrepresentable_initial_deadline() {
    let clock = Clock::new();
    assert!(matches!(
        construct(&clock, idle_start(), vec![], (RETRY, OPERATION, LIFETIME),),
        Err(ClientFailure::InvalidEndpoints)
    ));
    let duplicate_peers = vec![endpoints()[0], endpoints()[0]];
    assert!(matches!(
        construct(&clock, idle_start(), duplicate_peers, (RETRY, OPERATION, LIFETIME),),
        Err(ClientFailure::InvalidEndpoints)
    ));
    clock.set(u64::MAX);
    assert!(matches!(
        construct(&clock, idle_start(), endpoints(), (RETRY, OPERATION, LIFETIME),),
        Err(ClientFailure::InvalidTime { source: ClockReadFailure::OutOfRange })
    ));
}

#[test]
fn exhausted_stream_is_not_wrapped_or_silently_resequenced() {
    let clock = Clock::new();
    let mut starts = idle_start();
    starts.client_seq = u64::MAX;
    let mut client = construct(&clock, starts, endpoints(), (RETRY, OPERATION, LIFETIME)).unwrap();
    connect(&mut client);
    let refusal = client.try_submit(batch(1)).unwrap_err();
    assert!(matches!(refusal.reason, ClientFailure::SequenceExhausted));
    assert_eq!(wire(&refusal.batch), wire(&batch(1)));
    assert!(matches!(client.status(), ClientStatus::Batch { occupied: 0, .. }));
}

#[test_case::test_case(1; "initial")]
#[test_case::test_case(u64::MAX - 1; "last_available")]
#[test_case::test_case(u64::MAX; "exhausted")]
fn local_correlation_exhaustion_is_not_stream_sequence_exhaustion(current: u64) {
    // These are the actual scalar guards used at publication, not a fabricated
    // owner with MAX historical requests or connections.
    let request = adapter::next_batch_request_id(NonZeroU64::new(current).unwrap());
    let connection = adapter::next_batch_connection_id(current);
    if current == u64::MAX {
        assert!(matches!(request, Err(ClientFailure::RequestIdExhausted)));
        assert!(matches!(connection, Err(ClientFailure::ConnectionIdExhausted)));
    } else {
        assert_eq!(request.unwrap().get(), current + 1);
        assert_eq!(connection.unwrap(), current + 1);
    }
}

#[derive(Clone, Copy)]
enum InvalidReservation {
    OccupiedSlot,
    ForeignSession,
    ForeignStream,
    WrongSequence,
    ReusedRequest,
    UnadvancedCounter,
}

#[test_case::test_case(InvalidReservation::OccupiedSlot; "occupied_slot")]
#[test_case::test_case(InvalidReservation::ForeignSession; "foreign_session")]
#[test_case::test_case(InvalidReservation::ForeignStream; "foreign_stream")]
#[test_case::test_case(InvalidReservation::WrongSequence; "wrong_sequence")]
#[test_case::test_case(InvalidReservation::ReusedRequest; "reused_request")]
#[test_case::test_case(InvalidReservation::UnadvancedCounter; "unadvanced_counter")]
fn slot_candidate_guard_returns_original_without_publishing(case: InvalidReservation) {
    let clock = Clock::new();
    let mut client = client(&clock);
    let operation_id = connect(&mut client);
    let resident = matches!(case, InvalidReservation::OccupiedSlot).then(|| client.try_submit(batch(1)).unwrap());
    let ClientState::Batch { context, .. } = &client.state;
    let mut context = *context;
    let mut next = adapter::next_batch_request_id(context.request_id).unwrap();
    let mut input = idle_start();
    match case {
        InvalidReservation::OccupiedSlot => (),
        InvalidReservation::ForeignSession => context.session_incarnation = NonZeroU128::MIN,
        InvalidReservation::ForeignStream => input.stream_id += 1,
        InvalidReservation::WrongSequence => input.client_seq += 1,
        InvalidReservation::ReusedRequest => context.request_id = next,
        InvalidReservation::UnadvancedCounter => next = context.request_id,
    }
    let original = batch_at(input, 1);
    let bytes = wire(&original);
    // Challenge the actual pre-publication boundary with a borrowed candidate,
    // not fabricated installed state or completion evidence.
    let candidate = BatchSlot::Pending {
        handle: BatchHandle { context, intent: original.commitment_v1() },
        batch: original,
        deadline: MonotonicMillis::new(INITIAL_TIME + LIFETIME),
        attempt: BatchAttempt::Eligible { at: clock.sample().unwrap() },
        submit_route: None,
        submit_transmissions: 0,
        committed: None,
        notice: None,
    };
    let result = client.reserve_batch_slot(candidate, next);
    assert!(
        matches!(&result, Err((ClientFailure::BatchReservationInvariant, _))),
        "invalid candidate must be refused before publishing slot custody"
    );
    let (_, BatchSlot::Pending { batch: returned, .. }) = result.unwrap_err() else { panic!("original candidate") };
    assert_eq!(wire(&returned), bytes);
    assert!(validate_batch_state(&client.state, None, None).is_ok());
    let expected = resident.unwrap_or_else(|| client.try_submit(batch(1)).unwrap());
    assert_eq!(expected.context.request_id.get(), 1, "candidate refusal cannot consume an ID");
    assert!(matches!(client.drive(ClientEvent::Stop), ClientResult::Close { operation_id: id } if id == operation_id));
    let Some(BatchEvent::Terminal(BatchTerminal::OutcomeUnknown { handle, .. })) = client.poll_event() else {
        panic!("custody")
    };
    assert_eq!(handle, expected, "candidate refusal must retain earlier custody");
    assert!(client.poll_event().is_none());
}

#[test]
fn connect_candidate_guard_cannot_replace_a_live_socket() {
    let clock = Clock::new();
    let mut client = client(&clock);
    let operation_id = connect(&mut client);
    client.try_submit(batch(1)).unwrap();

    // The internal publication boundary cannot mint a second effect while the
    // first socket is owned, even if a future scheduling caller omits its guard.
    let result = client.begin_batch_connect(1, clock.sample().unwrap());
    assert!(
        matches!(result, Err(ClientFailure::BatchConnectionInvariant)),
        "connection candidate cannot replace outstanding physical custody"
    );
    assert!(validate_batch_state(&client.state, None, None).is_ok());
    assert!(matches!(client.drive(ClientEvent::Stop), ClientResult::Close { operation_id: id } if id == operation_id));
    assert!(matches!(client.drive(ClientEvent::Closed { operation_id }), ClientResult::Waiting { wake_at: None }));
    assert!(validate_batch_state(&client.state, None, None).is_ok());
    assert!(matches!(
        client.poll_event(),
        Some(BatchEvent::Terminal(BatchTerminal::OutcomeUnknown { cause: ClientFailure::Stopped, .. }))
    ));
    assert!(client.poll_event().is_none());
}

#[test]
fn complete_frame_and_command_limits_are_checked_before_admission() {
    let clock = Clock::new();
    // Keep the request exact-fit cut above the necessary reply reservation.
    let exact = FRAME_LIMIT as usize;
    let exact_request = |stream| {
        let (first, mut commands) = batch(stream).into_parts();
        let envelope = ingress_api::stream_batch::HEADER_BYTES + ingress_api::stream_batch::CONTEXT_BYTES;
        let padding = exact - wire(&batch(stream)).len() - envelope;
        let mut bytes = commands[0].payload_bytes.to_vec();
        bytes.resize(bytes.len() + padding, 0);
        commands[0].payload_bytes = bytes.into();
        ClientStreamBatch::try_new(first, commands, FRAME_LIMIT as usize, COMMAND_LIMIT as usize).unwrap()
    };
    let mut client = BatchClient::bounded(
        (Digest32::new([11; 32]), Digest32::new([22; 32]), Digest32::new([33; 32])),
        NonZeroU128::MIN,
        idle_start(),
        endpoints(),
        1,
        exact as u32,
        OUTCOME_LIMIT,
        RETRY,
        OPERATION,
        LIFETIME,
        3,
        {
            let clock = clock.clone();
            move || clock.sample()
        },
    )
    .unwrap();
    connect(&mut client);
    let (first, mut commands) = batch(1).into_parts();
    commands.push(commands[0].clone());
    let oversized = ClientStreamBatch::try_new(first, commands, FRAME_LIMIT as usize, COMMAND_LIMIT as usize).unwrap();
    let original = wire(&oversized);
    let refusal = client.try_submit(oversized).unwrap_err();
    assert!(matches!(
        refusal.reason,
        ClientFailure::TooLarge { source: ClientStreamBatchFailure::InvalidCount { observed: 2, limit: 1 } }
    ));
    assert_eq!(wire(&refusal.batch), original);
    let (first, mut commands) = exact_request(1).into_parts();
    let mut bytes = commands[0].payload_bytes.to_vec();
    bytes.push(0);
    commands[0].payload_bytes = bytes.into();
    let oversized = ClientStreamBatch::try_new(first, commands, FRAME_LIMIT as usize, COMMAND_LIMIT as usize).unwrap();
    let original = wire(&oversized);
    let refusal = client.try_submit(oversized).unwrap_err();
    assert!(
        matches!(refusal.reason, ClientFailure::TooLarge { source: ClientStreamBatchFailure::EncodedBytesExceeded { observed, limit } } if observed == limit + 1)
    );
    assert_eq!(wire(&refusal.batch), original);
    assert_eq!(
        client.try_submit(exact_request(1)).unwrap().context.request_id.get(),
        1,
        "exact fit and no ID spent on refusal"
    );
}

#[test]
fn failed_connect_preserves_typed_cause_and_spaces_next_attempt() {
    let clock = Clock::new();
    let mut client = client(&clock);
    let ClientResult::Connect { operation_id, .. } = client.drive(ClientEvent::Drive) else { panic!("connect") };
    let result = client
        .drive(ClientEvent::Connected { operation_id, result: Err(std::io::ErrorKind::ConnectionRefused.into()) });
    assert!(
        !matches!(&result, ClientResult::DriveRejected { .. }),
        "a consumed connection failure must not be returned for replay"
    );
    assert!(
        matches!(result, ClientResult::ConnectionFailed {
        operation_id: returned, source, next_wake: Some(due),
    } if returned == operation_id && source.kind() == std::io::ErrorKind::ConnectionRefused
        && due.get() == INITIAL_TIME + RETRY),
        "consumed failure carries its retry wake"
    );
    assert!(
        matches!(client.drive(ClientEvent::Connected {
        operation_id, result: Err(std::io::ErrorKind::ConnectionRefused.into()),
    }), ClientResult::Waiting { wake_at: Some(t) } if t.get() == INITIAL_TIME + RETRY),
        "duplicate failure cannot rotate or reschedule the next endpoint"
    );
    assert!(
        matches!(client.drive(ClientEvent::Drive), ClientResult::Waiting { wake_at: Some(t) } if t.get() == INITIAL_TIME + RETRY)
    );
    clock.set(INITIAL_TIME + RETRY);
    assert!(
        matches!(client.drive(ClientEvent::Drive), ClientResult::Connect { peer_id: 2, operation_id: next, .. } if next != operation_id)
    );
}

#[test]
fn queued_close_is_consumed_after_a_later_refused_submission() {
    let clock = Clock::new();
    let mut client = client(&clock);
    let operation_id = connect(&mut client);
    assert!(matches!(client.drive(ClientEvent::Stop), ClientResult::Close { .. }));
    let closed = ClientEvent::Closed { operation_id };
    clock.set(INITIAL_TIME + 5);
    assert!(matches!(client.try_submit(batch(1)).unwrap_err().reason, ClientFailure::Stopped));

    let result = client.drive(closed);
    assert!(
        matches!(result, ClientResult::Waiting { wake_at: None }),
        "a queued physical close must retire despite intervening admission: {result:?}"
    );
    assert!(matches!(client.state, ClientState::Batch { connection: BatchConnection::Stopped, .. }));
}

#[test_case::test_case(Err(ClockReadFailure::OutOfRange); "clock read fails")]
#[test_case::test_case(Ok(MonotonicMillis::new(INITIAL_TIME - 1)); "clock regresses")]
fn physical_close_is_consumed_even_when_the_clock_cannot_be_read(sample: Result<MonotonicMillis, ClockReadFailure>) {
    let clock = Clock::new();
    let mut client = client(&clock);
    let operation_id = connect(&mut client);
    let handle = client.try_submit(batch(1)).unwrap();
    assert!(matches!(client.drive(ClientEvent::Stop), ClientResult::Close { .. }));
    *clock.0.lock().unwrap() = sample;

    let result = client.drive(ClientEvent::Closed { operation_id });
    assert!(
        matches!(client.state, ClientState::Batch { connection: BatchConnection::Stopped, .. }),
        "clock failure must not retain physically retired connection custody"
    );
    assert!(matches!(
        (sample, result),
        (
            Err(ClockReadFailure::OutOfRange),
            ClientResult::ClockFailed { cause: ClientFailure::InvalidTime { source: ClockReadFailure::OutOfRange } }
        ) | (Ok(_), ClientResult::ClockFailed { cause: ClientFailure::TimeRegressed { .. } })
    ));
    let BatchEvent::Terminal(BatchTerminal::OutcomeUnknown { handle: returned, cause, .. }) =
        client.poll_event().unwrap()
    else {
        panic!("expected retained original")
    };
    assert_eq!(returned, handle);
    assert!(matches!(cause, ClientFailure::Stopped));
    assert!(client.poll_event().is_none());
}

#[test_case::test_case(Err(ClockReadFailure::OutOfRange); "clock read fails")]
#[test_case::test_case(Ok(MonotonicMillis::new(INITIAL_TIME - 1)); "clock regresses")]
#[test_case::test_case(Ok(MonotonicMillis::new(u64::MAX)); "retry deadline overflows")]
fn failed_connect_with_invalid_time_consumes_retirement_and_stops(sample: Result<MonotonicMillis, ClockReadFailure>) {
    let clock = Clock::new();
    let mut client = client(&clock);
    let ClientResult::Connect { operation_id, .. } = client.drive(ClientEvent::Drive) else { panic!("connect") };
    *clock.0.lock().unwrap() = sample;
    let result = client
        .drive(ClientEvent::Connected { operation_id, result: Err(std::io::ErrorKind::ConnectionRefused.into()) });
    assert!(matches!(result, ClientResult::ConnectionFailed { operation_id: actual, source, next_wake: None }
        if actual == operation_id && source.kind() == std::io::ErrorKind::ConnectionRefused));
    assert!(
        matches!(client.state, ClientState::Batch { connection: BatchConnection::Stopped, .. }),
        "clock failure cannot retain physically retired connection custody"
    );
    clock.set(u64::MAX);
    assert!(
        matches!(client.drive(ClientEvent::Drive), ClientResult::Waiting { wake_at: None }),
        "clock recovery cannot revive the session"
    );
    assert!(client.poll_event().is_none());
}

#[test]
fn queued_connect_samples_only_at_processing_time() {
    let clock = Clock::new();
    let mut client = client(&clock);
    let ClientResult::Connect { operation_id, .. } = client.drive(ClientEvent::Drive) else { panic!("connect") };
    let connected = ClientEvent::Connected { operation_id, result: Ok(()) };
    clock.set(INITIAL_TIME + OPERATION);
    assert!(
        matches!(client.drive(connected), ClientResult::Close { operation_id: same } if same == operation_id),
        "a queued success must use handling time, not its earlier creation time"
    );
}

#[test]
fn public_adoption_samples_only_at_processing_time() {
    let clock = Clock::new();
    let mut client = client(&clock);
    connect(&mut client);
    let original = batch(1);
    clock.set(INITIAL_TIME + OPERATION + LIFETIME);
    client.try_submit(original).unwrap();
    assert!(
        matches!(&client.state, ClientState::Batch { slot, .. }
        if matches!(slot, Some(BatchSlot::Pending { deadline, .. })
            if deadline.get() == INITIAL_TIME + OPERATION + LIFETIME * 2)),
        "public adoption must sample the bound clock when accepting the original"
    );
}

#[test]
fn clock_failure_cannot_park_an_already_required_close() {
    let clock = Clock::new();
    let mut client = client(&clock);
    let operation_id = connect(&mut client);
    client.try_submit(batch(1)).unwrap();
    clock.set(INITIAL_TIME + LIFETIME);
    assert!(matches!(client.try_submit(batch(2)).unwrap_err().reason, ClientFailure::Stopped));
    *clock.0.lock().unwrap() = Err(ClockReadFailure::OutOfRange);
    assert!(
        matches!(client.drive(ClientEvent::Drive), ClientResult::Close { operation_id: same } if same == operation_id)
    );
    assert!(matches!(client.drive(ClientEvent::Closed { operation_id }), ClientResult::ClockFailed { .. }));
    assert!(matches!(client.state, ClientState::Batch { connection: BatchConnection::Stopped, .. }));
}

#[test]
fn failed_connect_after_stop_returns_resources_without_requiring_a_second_close() {
    let clock = Clock::new();
    let mut client = client(&clock);
    let ClientResult::Connect { operation_id, .. } = client.drive(ClientEvent::Drive) else { panic!("connect") };
    assert!(matches!(client.drive(ClientEvent::Stop), ClientResult::Close { .. }));
    *clock.0.lock().unwrap() = Err(ClockReadFailure::OutOfRange);
    assert!(matches!(
        client.drive(ClientEvent::Connected { operation_id, result: Err(std::io::ErrorKind::Interrupted.into()) }),
        ClientResult::ConnectionFailed { operation_id: same, source, next_wake: None }
            if same == operation_id && source.kind() == std::io::ErrorKind::Interrupted
    ));
    assert!(matches!(client.state, ClientState::Batch { connection: BatchConnection::Stopped, .. }));
    assert!(matches!(client.drive(ClientEvent::Stop), ClientResult::Waiting { wake_at: None }));
}

#[test]
fn connect_deadline_overflow_stops_without_issuing_a_socket() {
    let clock = Clock::new();
    let mut client = client(&clock);
    clock.set(u64::MAX);
    assert!(matches!(
        client.drive(ClientEvent::Drive),
        ClientResult::ClockFailed { cause: ClientFailure::InvalidTime { source: ClockReadFailure::OutOfRange } }
    ));
    assert!(matches!(
        client.status(),
        ClientStatus::Batch { stopped: true, connected: false, next_deadline: None, .. }
    ));
    assert!(matches!(client.drive(ClientEvent::Drive), ClientResult::Waiting { wake_at: None }));
}

#[test]
fn stop_while_connecting_closes_once_and_never_accepts_late_success() {
    let clock = Clock::new();
    let mut client = client(&clock);
    let ClientResult::Connect { operation_id, .. } = client.drive(ClientEvent::Drive) else { panic!("connect") };
    assert!(
        matches!(client.drive(ClientEvent::Stop), ClientResult::Close { operation_id: same } if same == operation_id)
    );
    assert!(matches!(
        client.drive(ClientEvent::Connected { operation_id, result: Ok(()) }),
        ClientResult::Waiting { wake_at: None }
    ));
    assert!(matches!(client.drive(ClientEvent::Closed { operation_id }), ClientResult::Waiting { wake_at: None }));
    assert!(matches!(client.drive(ClientEvent::Stop), ClientResult::Waiting { wake_at: None }));
    assert!(matches!(client.try_submit(batch(1)).unwrap_err().reason, ClientFailure::Stopped));
}

#[test]
fn wrong_start_does_not_rewrite_intent_or_consume_a_sequence() {
    let clock = Clock::new();
    let mut client = client(&clock);
    connect(&mut client);
    let (mut first, commands) = batch(1).into_parts();
    first.client_seq += 1;
    let wrong = ClientStreamBatch::try_new(first, commands, FRAME_LIMIT as usize, COMMAND_LIMIT as usize).unwrap();
    let original = wire(&wrong);
    let refusal = client.try_submit(wrong).unwrap_err();
    assert!(matches!(refusal.reason, ClientFailure::BatchSequence { expected: 41, observed: 42 }));
    assert_eq!(wire(&refusal.batch), original);
    assert_eq!(client.try_submit(batch(1)).unwrap().context.request_id.get(), 1);
}

#[test_case::test_case(1; "singleton")]
#[test_case::test_case(2; "two members")]
#[test_case::test_case(COMMAND_LIMIT as usize; "member limit")]
fn reservation_matches_the_real_request_codec_and_survives_terminal_latching(count: usize) {
    let clock = Clock::new();
    let mut client = client(&clock);
    connect(&mut client);
    let original = batch_at(idle_start(), count);
    let charged = adapter::batch_frame_bytes(&original, COMMAND_LIMIT, FRAME_LIMIT).unwrap();
    let handle = client.try_submit(original).unwrap();
    assert!(matches!(client.drive(ClientEvent::Stop), ClientResult::Close { .. }));
    assert!(
        matches!(client.status(), ClientStatus::Batch { occupied: 1, .. }),
        "latching does not release the frame reservation"
    );
    let BatchEvent::Terminal(BatchTerminal::OutcomeUnknown { batch, handle: returned, .. }) =
        client.poll_event().unwrap()
    else {
        panic!("expected retained original")
    };
    assert_eq!(returned, handle);
    let request = ingress_api::stream_batch::Request::Submit { context: handle.context, batch };
    let encoded =
        ingress_api::stream_batch::encode_request(&request, FRAME_LIMIT as usize, COMMAND_LIMIT as usize).unwrap();
    assert_eq!(charged, encoded.len(), "reservation includes the actual request envelope and every member");
    let decoded =
        ingress_api::stream_batch::decode_request(&encoded, FRAME_LIMIT as usize, COMMAND_LIMIT as usize).unwrap();
    assert_eq!(*decoded.context(), handle.context);
    assert_eq!(decoded.batch().commitment_v1(), handle.intent);
    assert!(matches!(client.status(), ClientStatus::Batch { occupied: 0, .. }));
}

#[test_case::test_case(1; "minimum metadata")]
#[test_case::test_case(BATCH_CLIENT_MAX_ENDPOINTS; "maximum metadata")]
fn configured_endpoint_cap_is_inclusive(endpoint_count: usize) {
    let clock = Clock::new();
    let peers = (1..=endpoint_count).map(|id| (id as u64, endpoints()[0].1)).collect();
    let mut client = construct(&clock, idle_start(), peers, (RETRY, OPERATION, LIFETIME)).unwrap();
    connect(&mut client);
    assert_eq!(client.try_submit(batch(1)).unwrap().context.request_id.get(), 1);
}

#[test_case::test_case(0, 1, 41, None; "zero client")]
#[test_case::test_case(7, 0, 41, None; "zero stream")]
#[test_case::test_case(7, 1, 0, None; "zero cursor")]
#[test_case::test_case(7, 1, 41, Some("old request"); "request is not an idle cursor")]
fn invalid_known_idle_starts_are_refused(client_id: u64, stream_id: u32, client_seq: u64, request: Option<&str>) {
    let first = InputRef { client_id, stream_id, client_seq, request_id: request.map(str::to_owned) };
    assert!(matches!(
        construct(&Clock::new(), first, endpoints(), (RETRY, OPERATION, LIFETIME),),
        Err(ClientFailure::InvalidStream)
    ));
}

#[test_case::test_case(0, "127.0.0.1:12001"; "zero peer")]
#[test_case::test_case(1, "127.0.0.1:0"; "zero port")]
#[test_case::test_case(1, "0.0.0.0:12001"; "unspecified IPv4")]
#[test_case::test_case(1, "[::]:12001"; "unspecified IPv6")]
fn invalid_endpoint_identities_are_refused(peer: u64, address: &str) {
    assert!(matches!(
        construct(&Clock::new(), idle_start(), vec![(peer, address.parse().unwrap())], (RETRY, OPERATION, LIFETIME),),
        Err(ClientFailure::InvalidEndpoints)
    ));
}

#[test]
fn metadata_above_caps_is_refused_before_clock_or_any_slot_exists() {
    let clock = Clock::new();
    *clock.0.lock().unwrap() = Err(ClockReadFailure::OutOfRange);
    let peers = (1..=BATCH_CLIENT_MAX_ENDPOINTS + 1)
        .map(|id| (id as u64, endpoints()[0].1))
        .collect();
    assert!(matches!(
        construct(&clock, idle_start(), peers, (RETRY, OPERATION, LIFETIME),),
        Err(ClientFailure::InvalidEndpoints)
    ));
    assert!(matches!(
        construct(&clock, idle_start(), endpoints(), (RETRY, OPERATION, LIFETIME),),
        Err(ClientFailure::InvalidTime { source: ClockReadFailure::OutOfRange })
    ));
}

#[test]
fn reservation_never_advances_the_known_idle_start() {
    let clock = Clock::new();
    let first = InputRef { client_seq: u64::MAX - 1, ..idle_start() };
    let mut client = construct(&clock, first.clone(), endpoints(), (RETRY, OPERATION, LIFETIME)).unwrap();
    connect(&mut client);
    let last = client.try_submit(batch_at(first, 1)).unwrap();
    assert_eq!(last.intent.first_sequence(), u64::MAX - 1);
    let ClientState::Batch { stream, .. } = &client.state;
    assert_eq!(stream.client_seq, u64::MAX - 1, "only exact terminal consumption advances the cursor");
    assert!(matches!(client.try_submit(batch(1)).unwrap_err().reason, ClientFailure::Full));
}

#[statevec_domain_roles::semantic_evidence(transition = IngressClientOwner::drive, kind = OwnerPath)]
#[statevec_domain_roles::semantic_evidence(transition = IngressClientOwner::try_submit, kind = OwnerPath)]
#[statevec_domain_roles::semantic_evidence(transition = IngressClientOwner::poll_event, kind = OwnerPath)]
#[test]
fn explicit_stop_terminal_survives_later_deadline_and_physical_close() {
    let clock = Clock::new();
    let mut client = client(&clock);
    let operation_id = connect(&mut client);
    let handle = client.try_submit(batch(1)).unwrap();
    assert!(matches!(client.drive(ClientEvent::Stop), ClientResult::Close { .. }));
    clock.set(INITIAL_TIME + LIFETIME);
    assert!(matches!(client.drive(ClientEvent::Drive), ClientResult::Waiting { wake_at: None }));
    assert!(matches!(client.drive(ClientEvent::Stop), ClientResult::Waiting { wake_at: None }));
    assert!(matches!(client.drive(ClientEvent::Closed { operation_id }), ClientResult::Waiting { wake_at: None }));
    assert!(matches!(client.status(), ClientStatus::Batch { occupied: 1, stopped: true, .. }));
    let BatchEvent::Terminal(BatchTerminal::OutcomeUnknown { handle: returned, batch: original, cause, .. }) =
        client.poll_event().unwrap()
    else {
        panic!("expected retained original")
    };
    assert_eq!(returned, handle);
    assert_eq!(wire(&original), wire(&batch(1)));
    assert!(matches!(cause, ClientFailure::Stopped));
    assert!(client.poll_event().is_none());
    assert!(matches!(client.try_submit(batch(1)).unwrap_err().reason, ClientFailure::Stopped));
}

#[test]
fn lifetime_expiry_terminal_is_not_rewritten_by_later_explicit_stop() {
    let clock = Clock::new();
    let mut client = client(&clock);
    let operation_id = connect(&mut client);
    let handle = client.try_submit(batch(1)).unwrap();
    clock.set(INITIAL_TIME + LIFETIME);
    assert!(matches!(client.drive(ClientEvent::Drive), ClientResult::Close { .. }));
    assert!(matches!(client.drive(ClientEvent::Stop), ClientResult::Waiting { wake_at: None }));
    assert!(matches!(client.drive(ClientEvent::Closed { operation_id }), ClientResult::Waiting { wake_at: None }));
    let BatchEvent::Terminal(BatchTerminal::OutcomeUnknown { handle: returned, batch: original, cause, .. }) =
        client.poll_event().unwrap()
    else {
        panic!("expected retained original")
    };
    assert_eq!(returned, handle);
    assert_eq!(wire(&original), wire(&batch(1)));
    assert!(matches!(cause, ClientFailure::RequestLifetimeExpired));
    assert!(client.poll_event().is_none());
}

fn construct_limits(
    identity: (Digest32, Digest32, Digest32),
    commands: u32,
    frame: u32,
    outcome: u32,
    unavailable: u32,
) -> Result<BatchClient, ClientFailure> {
    BatchClient::bounded(
        identity,
        NonZeroU128::MIN,
        idle_start(),
        endpoints(),
        commands,
        frame,
        outcome,
        RETRY,
        OPERATION,
        LIFETIME,
        unavailable,
        || Ok(MonotonicMillis::new(INITIAL_TIME)),
    )
}

#[test_case::test_case(0; "cluster")]
#[test_case::test_case(1; "genesis")]
#[test_case::test_case(2; "profile")]
fn constructor_refuses_empty_context_identity(component: usize) {
    let mut identity = [Digest32::new([11; 32]), Digest32::new([22; 32]), Digest32::new([33; 32])];
    identity[component] = Digest32::new([0; 32]);
    assert!(matches!(
        construct_limits((identity[0], identity[1], identity[2]), COMMAND_LIMIT, FRAME_LIMIT, OUTCOME_LIMIT, 3),
        Err(ClientFailure::InvalidIdentity)
    ));
}

const MIN_FRAME: u32 = (ingress_api::stream_batch::HEADER_BYTES
    + ingress_api::stream_batch::CONTEXT_BYTES
    + statevec_frame::CLIENT_STREAM_BATCH_HEADER_BYTES
    + 4) as u32;

#[test_case::test_case(MIN_FRAME + 8; "request fits but refusal does not")]
#[test_case::test_case(REPLY_FRAME_BOUND - 1; "maximum completed reply is one byte too large")]
fn constructor_refuses_frame_capacity_below_necessary_replies(frame: u32) {
    assert!(
        matches!(
            construct_limits(
                (Digest32::new([11; 32]), Digest32::new([22; 32]), Digest32::new([33; 32])),
                COMMAND_LIMIT,
                frame,
                OUTCOME_LIMIT,
                3,
            ),
            Err(ClientFailure::InvalidLimit)
        ),
        "construction must reject a frame budget that cannot receive its replies"
    );
}

#[test_case::test_case(0, FRAME_LIMIT, 3; "zero member limit")]
#[test_case::test_case(COMMAND_LIMIT, MIN_FRAME - 1, 3; "frame cannot hold envelope")]
#[test_case::test_case(COMMAND_LIMIT, FRAME_LIMIT, 0; "zero unavailable threshold")]
#[test_case::test_case(COMMAND_LIMIT, FRAME_LIMIT, BATCH_CLIENT_MAX_UNAVAILABLE_REPLIES + 1; "unavailable threshold above cap")]
fn unrepresentable_capacity_and_unavailable_limits_are_refused(commands: u32, frame: u32, unavailable: u32) {
    assert!(matches!(
        construct_limits(
            (Digest32::new([11; 32]), Digest32::new([22; 32]), Digest32::new([33; 32])),
            commands,
            frame,
            OUTCOME_LIMIT,
            unavailable
        ),
        Err(ClientFailure::InvalidLimit)
    ));
}

#[test_case::test_case(REPLY_FRAME_BOUND, 1; "minimum")]
#[test_case::test_case(FRAME_LIMIT, BATCH_CLIENT_MAX_UNAVAILABLE_REPLIES; "configured maximum")]
fn capacity_and_unavailable_boundaries_are_inclusive(frame: u32, unavailable: u32) {
    let client = construct_limits(
        (Digest32::new([11; 32]), Digest32::new([22; 32]), Digest32::new([33; 32])),
        COMMAND_LIMIT,
        frame,
        OUTCOME_LIMIT,
        unavailable,
    )
    .unwrap();
    assert!(matches!(client.status(), ClientStatus::Batch { occupied: 0, connected: false, .. }));
}

#[test_case::test_case(0; "zero outcome budget")]
#[test_case::test_case(CommitOutcome::FIXED_ENCODED_BYTES as u32 - 1; "outcome cannot hold fixed fields")]
#[test_case::test_case(u32::MAX; "reply budget exceeds the frame cap")]
fn constructor_rejects_unusable_outcome_capacity(outcome: u32) {
    assert!(matches!(
        construct_limits(
            (Digest32::new([11; 32]), Digest32::new([22; 32]), Digest32::new([33; 32])),
            COMMAND_LIMIT,
            FRAME_LIMIT,
            outcome,
            3,
        ),
        Err(ClientFailure::InvalidLimit)
    ));
}
