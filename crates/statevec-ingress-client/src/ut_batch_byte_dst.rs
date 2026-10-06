//! Bounded byte schedules over the production driver in all presentations.
//! The peer only decodes bytes and emits scripted codec replies; no server or
//! admission/retry implementation lives here. Faults are named directed cuts.
use super::*;
use raft_rand::{RngCore, SeedableRng, rngs::StdRng};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use test_case::test_case;

#[path = "ut_provenance.rs"]
mod provenance;

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
enum Style {
    Manual,
    Background,
    Async,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
enum Cut {
    Healthy,
    PartialDisconnect,
    OperationDeadline,
    ClockAtClose,
    Stop,
    Lifetime,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
enum Action {
    Pump { read: usize, write: usize },
    Deliver(usize),
    Advance(u64),
    FaultClockOnClose,
    RestoreClock(u64),
    Disconnect,
    Stop,
    Consume,
    CheckCharged(usize),
    CheckRequests(usize),
    CheckRetired(usize),
    CheckClockFailure,
    CheckSettled { exact: usize, unknown: usize },
}

enum Presentation {
    Manual { client: BatchClient, driver: BatchDriverWorker<ScriptIo> },
    Thread(Running),
    Returned(BatchClient),
}
impl Presentation {
    fn status(&self) -> ClientStatus {
        match self {
            Self::Manual { client, .. } | Self::Returned(client) => client.status(),
            Self::Thread(running) => running.client().status().unwrap(),
        }
    }
    fn pump(&mut self) {
        match self {
            Self::Manual { client, driver } => {
                driver.wait(client, Duration::ZERO).unwrap();
                for _ in 0..STEPS {
                    match driver.step(client) {
                        Ok(BatchDriverStep::Runnable) => (),
                        Ok(BatchDriverStep::Waiting) => return,
                        Ok(BatchDriverStep::Diagnostic {
                            cause: ClientFailure::BatchRead { source: stream_batch::ReadFailure::Io { source } },
                        }) if source.kind() == io::ErrorKind::UnexpectedEof => (),
                        Ok(BatchDriverStep::Diagnostic { cause: ClientFailure::InvalidTime { .. } }) => return,
                        other => panic!("unexpected driver report: {other:?}"),
                    }
                }
                panic!("byte schedule failed to yield");
            }
            Self::Thread(running) => {
                let target = running.gate.0.0.lock().unwrap().waits + 1;
                running.gate.notify();
                running.gate.release_one();
                running.wait_or_finished(target);
                match running.client().take_driver_error() {
                    None => (),
                    Some(ClientFailure::BatchRead { source: stream_batch::ReadFailure::Io { source } })
                        if source.kind() == io::ErrorKind::UnexpectedEof =>
                    {
                        ()
                    }
                    Some(ClientFailure::InvalidTime { .. }) => (),
                    cause => panic!("unexpected background failure: {cause:?}"),
                }
            }
            Self::Returned(_) => (),
        }
    }
    fn consume(&mut self, style: Style) -> Option<BatchEvent> {
        match self {
            Self::Manual { client, .. } | Self::Returned(client) => client.poll_event(),
            Self::Thread(running) if matches!(style, Style::Async) => {
                let mut future = std::pin::pin!(running.client_mut().next_event());
                match future.as_mut().poll(&mut Context::from_waker(Waker::noop())) {
                    Poll::Ready(result) => result.unwrap(),
                    Poll::Pending => None, // Drop waiter only; retain all custody.
                }
            }
            Self::Thread(running) => running.client().poll_event().unwrap(),
        }
    }
}

struct PeerBytes(VecDeque<u8>);
impl Read for PeerBytes {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if self.0.is_empty() {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        let count = bytes.len().min(self.0.len());
        for byte in &mut bytes[..count] {
            *byte = self.0.pop_front().unwrap();
        }
        Ok(count)
    }
}

struct ByteWorld {
    style: Style,
    presentation: Option<Presentation>,
    clock: Clock,
    devices: Vec<Arc<Mutex<Device>>>,
    peer_reader: stream_batch::FrameReader,
    peer_input: PeerBytes,
    responses: VecDeque<u8>,
    reply_sizes: Vec<usize>,
    received: Vec<(u64, u32)>,
    handles: Vec<BatchHandle>,
    consumed: BTreeSet<u32>,
    exact: usize,
    unknown: usize,
    original: Vec<Vec<u8>>,
}
impl ByteWorld {
    fn new(style: Style, original: &[Vec<u8>]) -> Self {
        let clock = Clock::new();
        let mut client = construct(&clock, idle_start(), vec![endpoints()[0]], (RETRY, OPERATION, LIFETIME)).unwrap();
        let devices: Vec<_> = (0..2).map(|_| bytes()).collect();
        for device in &devices {
            let mut device = device.lock().unwrap();
            device.read_calls = Some(0);
            device.write_calls = Some(0);
        }
        let (presentation, handles) = match style {
            Style::Manual => {
                let (mut driver, _) = driver(&devices);
                drain(&mut driver, &mut client);
                let handles = original
                    .iter()
                    .map(|bytes| {
                        client
                            .try_submit(
                                ClientStreamBatch::decode_v1(bytes, FRAME_LIMIT as usize, COMMAND_LIMIT as usize)
                                    .unwrap(),
                            )
                            .unwrap()
                    })
                    .collect();
                (Presentation::Manual { client, driver }, handles)
            }
            Style::Background | Style::Async => {
                let running = start(client, &devices);
                let handles = original
                    .iter()
                    .map(|bytes| {
                        running
                            .client()
                            .try_submit(
                                ClientStreamBatch::decode_v1(bytes, FRAME_LIMIT as usize, COMMAND_LIMIT as usize)
                                    .unwrap(),
                            )
                            .unwrap()
                    })
                    .collect();
                (Presentation::Thread(running), handles)
            }
        };
        Self {
            style,
            presentation: Some(presentation),
            clock,
            devices,
            peer_reader: Self::reader(),
            peer_input: PeerBytes(VecDeque::new()),
            responses: VecDeque::new(),
            reply_sizes: Vec::new(),
            received: Vec::new(),
            handles,
            consumed: BTreeSet::new(),
            exact: 0,
            unknown: 0,
            original: original.to_vec(),
        }
    }
    fn reader() -> stream_batch::FrameReader {
        stream_batch::FrameReader::new(
            stream_batch::StreamDirection::Requests,
            FRAME_LIMIT as usize,
            COMMAND_LIMIT as usize,
        )
        .unwrap()
    }
    fn active(&self) -> usize {
        usize::from(self.devices[0].lock().unwrap().drops != 0)
    }
    fn peer(&mut self) {
        let active = self.active();
        self.peer_input
            .0
            .extend(std::mem::take(&mut self.devices[active].lock().unwrap().outgoing));
        for _ in 0..STEPS {
            match self.peer_reader.poll_bytes(&mut self.peer_input).unwrap() {
                stream_batch::ReadProgress::Runnable => (),
                stream_batch::ReadProgress::WouldBlock => return,
                stream_batch::ReadProgress::Eof => panic!("virtual peer cannot invent EOF"),
                stream_batch::ReadProgress::Frame(bytes) => {
                    let request =
                        stream_batch::decode_request(&bytes, FRAME_LIMIT as usize, COMMAND_LIMIT as usize).unwrap();
                    let stream = request.batch().first_input().stream_id;
                    assert!(
                        wire(request.batch()) == self.original[(stream - 1) as usize],
                        "DST invariant: physical retry preserves the complete original intent for stream {stream}"
                    );
                    assert!(!self.consumed.contains(&stream), "DST invariant: consumed batch cannot be resent");
                    self.received.push((request.context().request_id.get(), stream));
                    let reply =
                        stream_batch::encode_reply(&completed(&request), FRAME_LIMIT as usize, COMMAND_LIMIT as usize)
                            .unwrap();
                    self.reply_sizes.push(reply.len());
                    self.responses.extend(reply);
                }
            }
        }
        panic!("peer framing did not yield");
    }
    fn act(&mut self, action: &Action) -> String {
        match *action {
            Action::Pump { read, write } => {
                let active = self.active();
                {
                    let mut device = self.devices[active].lock().unwrap();
                    device.read_calls = Some(usize::from(read != 0));
                    device.read_limit = Some(read.max(1));
                    device.write_calls = Some(usize::from(write != 0));
                    device.write_limit = Some(write.max(1));
                }
                self.presentation.as_mut().unwrap().pump();
                if self.active() != active {
                    // Physical disconnect loses peer half-frames, not client intent.
                    self.peer_reader = Self::reader();
                    self.peer_input.0.clear();
                    self.responses.clear();
                }
                self.peer();
            }
            Action::Deliver(count) => {
                let count = count.min(self.responses.len());
                self.devices[self.active()]
                    .lock()
                    .unwrap()
                    .incoming
                    .extend(self.responses.drain(..count));
            }
            Action::Advance(delta) => self.clock.set(self.clock.sample().unwrap().get() + delta),
            Action::FaultClockOnClose => {
                self.devices[self.active()].lock().unwrap().fail_clock_on_drop = Some(self.clock.clone())
            }
            Action::RestoreClock(now) => self.clock.set(now),
            Action::Disconnect => self.devices[self.active()].lock().unwrap().eof = true,
            Action::Stop => {
                let old = self.presentation.take().unwrap();
                self.presentation = Some(match old {
                    Presentation::Thread(running) => {
                        let (client, error) = running.shutdown();
                        assert!(error.is_none(), "{error:?}");
                        Presentation::Returned(client)
                    }
                    Presentation::Manual { mut client, mut driver } => {
                        driver.stop(&mut client).unwrap();
                        drain(&mut driver, &mut client);
                        assert!(driver.is_quiescent());
                        Presentation::Returned(client)
                    }
                    returned @ Presentation::Returned(_) => returned,
                });
            }
            Action::Consume => {
                if let Some(event) = self.presentation.as_mut().unwrap().consume(self.style) {
                    let stream = event.handle().intent.stream_id();
                    assert!(
                        event.handle() == self.handles[(stream - 1) as usize],
                        "DST invariant: original application correlation survives I/O: {event:?}"
                    );
                    match event {
                        BatchEvent::Terminal(BatchTerminal::Applied { results, .. }) => {
                            assert!(
                                results.len() == 1,
                                "DST invariant: complete single-member result, got {results:?}"
                            );
                            self.exact += 1;
                            assert!(self.consumed.insert(stream), "DST invariant: one terminal per original batch");
                        }
                        BatchEvent::Terminal(BatchTerminal::OutcomeUnknown { batch, .. }) => {
                            assert!(
                                wire(&batch) == self.original[(stream - 1) as usize],
                                "DST invariant: termination returns intact original stream {stream}"
                            );
                            self.unknown += 1;
                            assert!(self.consumed.insert(stream), "DST invariant: one terminal per original batch");
                        }
                        other => panic!(
                            "DST invariant: script supplies exact results or explicit termination, got {other:?}"
                        ),
                    }
                }
            }
            Action::CheckCharged(expected) => assert!(
                matches!(self.presentation.as_ref().unwrap().status(), ClientStatus::Batch { occupied, .. } if occupied == expected),
                "DST invariant: requested charged-slot observation"
            ),
            Action::CheckRequests(expected) => {
                assert_eq!(self.received.len(), expected, "cut requires actual decoded requests")
            }
            Action::CheckRetired(expected) => assert_eq!(
                self.devices[0].lock().unwrap().drops,
                expected,
                "recovery cut must retire the partial socket"
            ),
            Action::CheckClockFailure => assert_eq!(
                self.clock.sample(),
                Err(ClockReadFailure::OutOfRange),
                "actual physical close must exercise the clock fault"
            ),
            Action::CheckSettled { exact, unknown } => {
                assert_eq!(self.consumed.len(), 1, "DST liveness: fair byte tail must settle the original");
                assert!(
                    (self.exact, self.unknown) == (exact, unknown),
                    "DST invariant: recovery cannot pass by expiring unresolved requests: exact={}, unknown={}, expected=({exact}, {unknown})",
                    self.exact,
                    self.unknown
                );
            }
        }
        let status = self.presentation.as_ref().unwrap().status();
        assert!(
            matches!(status, ClientStatus::Batch { occupied, .. } if occupied == 1 - self.consumed.len()),
            "DST invariant: slot stay charged until application consumption"
        );
        format!(
            "{status:?}; received={:?}; consumed={:?}; exact={}; unknown={}",
            self.received, self.consumed, self.exact, self.unknown
        )
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ByteTape {
    version: u32,
    revision: String,
    worktree_diff: String,
    generator_version: u32,
    profile: String,
    seed: u64,
    style: Style,
    cut: Cut,
    inputs: Vec<Vec<u8>>,
    actions: Vec<Action>,
    observations: Vec<String>,
    failure: Option<String>,
}
impl ByteTape {
    fn new(seed: u64, style: Style, cut: Cut) -> Self {
        let (revision, worktree_diff) = provenance::source_snapshot();
        Self {
            version: 3,
            revision: revision.clone(),
            worktree_diff: worktree_diff.clone(),
            generator_version: 1,
            profile: format!(
                "scripted-v14; single-slot; frame={FRAME_LIMIT}; commands={COMMAND_LIMIT}; outcome={OUTCOME_LIMIT}; retry={RETRY}; operation={OPERATION}; lifetime={LIFETIME}; unavailable={UNAVAILABLE_LIMIT}; starts={:?}; two-sockets-one-endpoint; virtual-ms-start={INITIAL_TIME}",
                idle_start()
            ),
            seed,
            style,
            cut,
            inputs: (1..=1).map(|id| wire(&batch(id))).collect(),
            actions: Vec::new(),
            observations: Vec::new(),
            failure: None,
        }
    }
    fn record(&mut self, world: &mut ByteWorld, action: Action) {
        assert!(self.actions.len() < 2048, "byte tape action bound, not a product timeout");
        self.actions.push(action.clone());
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| world.act(&action))) {
            Ok(observation) => self.observations.push(observation),
            Err(cause) => {
                self.failure = Some(message(&*cause));
                let directory = tempfile::Builder::new().prefix("ingress-byte-dst-").tempdir().unwrap().keep();
                serde_json::to_writer(std::fs::File::create(directory.join("trace.json")).unwrap(), self).unwrap();
                eprintln!("byte DST failed; executable trace at {}", directory.display());
                std::panic::resume_unwind(cause);
            }
        }
    }
    fn replay(&self) -> Result<Vec<String>, String> {
        assert_eq!(self.version, 3);
        assert_eq!(self.generator_version, 1);
        assert!(self.actions.len() <= 2048);
        assert_eq!(self.inputs.len(), 1);
        assert_eq!(self.profile, Self::new(self.seed, self.style, self.cut).profile);
        let mut world = ByteWorld::new(self.style, &self.inputs);
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.actions.iter().map(|action| world.act(action)).collect()
        }))
        .map_err(|cause| message(&*cause))
    }
    fn minimize(&self) -> Self {
        let expected = self.replay().expect_err("failure control must reproduce");
        assert!(expected.starts_with("DST invariant:"), "do not reduce harness/liveness failures");
        let mut reduced = self.clone();
        for index in (0..reduced.actions.len()).rev() {
            let mut candidate = reduced.clone();
            candidate.actions.remove(index);
            if candidate.replay().err().as_ref() == Some(&expected) {
                reduced = candidate;
            }
        }
        reduced.observations.clear();
        reduced.failure = Some(expected);
        reduced
    }
}
fn message(cause: &(dyn std::any::Any + Send)) -> String {
    cause
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| cause.downcast_ref::<&str>().map(|message| message.to_string()))
        .unwrap_or_else(|| "unclassified harness failure".into())
}

#[test_case(Style::Manual, Cut::Healthy; "manual_healthy")]
#[test_case(Style::Background, Cut::Healthy; "background_healthy")]
#[test_case(Style::Async, Cut::Healthy; "async_healthy")]
#[test_case(Style::Manual, Cut::PartialDisconnect; "manual_partial_disconnect")]
#[test_case(Style::Background, Cut::PartialDisconnect; "background_partial_disconnect")]
#[test_case(Style::Async, Cut::PartialDisconnect; "async_partial_disconnect")]
#[test_case(Style::Manual, Cut::OperationDeadline; "manual_operation_deadline")]
#[test_case(Style::Background, Cut::OperationDeadline; "background_operation_deadline")]
#[test_case(Style::Async, Cut::OperationDeadline; "async_operation_deadline")]
#[test_case(Style::Manual, Cut::ClockAtClose; "manual_clock_at_close")]
#[test_case(Style::Background, Cut::ClockAtClose; "background_clock_at_close")]
#[test_case(Style::Async, Cut::ClockAtClose; "async_clock_at_close")]
#[test_case(Style::Manual, Cut::Stop; "manual_stop")]
#[test_case(Style::Background, Cut::Stop; "background_stop")]
#[test_case(Style::Async, Cut::Stop; "async_stop")]
#[test_case(Style::Manual, Cut::Lifetime; "manual_lifetime")]
#[test_case(Style::Background, Cut::Lifetime; "background_lifetime")]
#[test_case(Style::Async, Cut::Lifetime; "async_lifetime")]
fn single_slot_byte_schedules_replay_across_all_presentations(style: Style, cut: Cut) {
    for seed in [1, 3, 7, 11, 17, 31] {
        let mut tape = ByteTape::new(seed, style, cut);
        let mut world = ByteWorld::new(style, &tape.inputs);
        for _ in 0..4 {
            tape.record(&mut world, Action::Pump { read: 0, write: FRAME_LIMIT as usize });
        }
        tape.record(&mut world, Action::CheckRequests(1));
        // Keep a partial reply on the only connection; no terminal may be inferred from it.
        let delivered = stream_batch::HEADER_BYTES + 2;
        tape.record(&mut world, Action::Deliver(delivered));
        for read in [stream_batch::HEADER_BYTES, FRAME_LIMIT as usize, stream_batch::HEADER_BYTES, 2] {
            tape.record(&mut world, Action::Pump { read, write: 0 });
        }
        tape.record(&mut world, Action::CheckCharged(1));
        match cut {
            Cut::Healthy => (),
            Cut::PartialDisconnect => {
                tape.record(&mut world, Action::Disconnect);
                tape.record(&mut world, Action::Pump { read: 1, write: 0 });
                // A surfaced physical error and the next owner Close are
                // separate turns. Release the background's intervening wait.
                tape.record(&mut world, Action::Pump { read: 0, write: 0 });
                tape.record(&mut world, Action::CheckRetired(1));
                tape.record(&mut world, Action::Advance(RETRY));
            }
            Cut::OperationDeadline | Cut::ClockAtClose => {
                if matches!(cut, Cut::ClockAtClose) {
                    tape.record(&mut world, Action::FaultClockOnClose);
                }
                tape.record(&mut world, Action::Advance(OPERATION));
                tape.record(&mut world, Action::Pump { read: 0, write: 0 });
                tape.record(&mut world, Action::CheckRetired(1));
                if matches!(cut, Cut::ClockAtClose) {
                    tape.record(&mut world, Action::CheckClockFailure);
                    tape.record(&mut world, Action::RestoreClock(INITIAL_TIME + OPERATION + RETRY));
                } else {
                    tape.record(&mut world, Action::Advance(RETRY));
                }
            }
            Cut::Stop => tape.record(&mut world, Action::Stop),
            Cut::Lifetime => {
                tape.record(&mut world, Action::Advance(LIFETIME));
                tape.record(&mut world, Action::Pump { read: 1, write: 1 });
            }
        }
        let mut rng = StdRng::seed_from_u64(seed);
        for _ in 0..64 {
            let action = match rng.next_u32() % 5 {
                0 => Action::Consume,
                1 => Action::Deliver((rng.next_u32() % 97) as usize),
                2 => Action::Advance(1),
                _ => Action::Pump { read: (rng.next_u32() % 67) as usize, write: (rng.next_u32() % 53) as usize },
            };
            tape.record(&mut world, action);
        }
        for _ in 0..64 {
            tape.record(&mut world, Action::Deliver(FRAME_LIMIT as usize));
            tape.record(&mut world, Action::Pump { read: FRAME_LIMIT as usize, write: FRAME_LIMIT as usize });
            tape.record(&mut world, Action::Consume);
            if world.consumed.len() == 1 {
                break;
            }
        }
        let (exact, unknown) =
            if matches!(cut, Cut::Stop | Cut::Lifetime | Cut::ClockAtClose) { (0, 1) } else { (1, 0) };
        tape.record(&mut world, Action::CheckSettled { exact, unknown });
        let decoded: ByteTape = serde_json::from_slice(&serde_json::to_vec(&tape).unwrap()).unwrap();
        assert_eq!(
            decoded.replay().unwrap(),
            tape.observations,
            "exact byte/action replay for {style:?}/{cut:?}/seed={seed}"
        );
    }
}

#[test]
fn byte_failure_control_replays_and_reduces_without_losing_source_provenance() {
    let mut tape = ByteTape::new(17, Style::Manual, Cut::Healthy);
    tape.actions = vec![Action::Advance(1), Action::Pump { read: 0, write: 1 }, Action::CheckCharged(0)];
    assert_eq!(tape.replay().unwrap_err(), "DST invariant: requested charged-slot observation");
    let reduced = tape.minimize();
    assert_eq!(reduced.actions.len(), 1, "control shrink keeps only the reached owner observation");
    assert_eq!(reduced.revision, tape.revision);
    assert_eq!(reduced.worktree_diff, tape.worktree_diff);
    assert_eq!(reduced.inputs, tape.inputs);
    assert_eq!(reduced.replay().unwrap_err(), tape.replay().unwrap_err());
}

#[test]
#[ignore = "set STATEVEC_CLIENT_BYTE_REPLAY to a recorded trace.json"]
fn replay_saved_byte_schedule() {
    let path = std::env::var_os("STATEVEC_CLIENT_BYTE_REPLAY").expect("supply the concrete failure tape path");
    let tape: ByteTape = serde_json::from_reader(std::fs::File::open(path).unwrap()).unwrap();
    match (&tape.failure, tape.replay()) {
        (Some(expected), Err(observed)) => assert_eq!(&observed, expected),
        (None, Ok(observed)) => assert_eq!(observed, tape.observations),
        (expected, observed) => panic!("recorded/replayed outcome differs: {expected:?} vs {observed:?}"),
    }
}
