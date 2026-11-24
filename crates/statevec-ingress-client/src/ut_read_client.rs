//! Scripted wire observations drive the production read owner. The double
//! supplies only time and physical results; replies are encoded with the
//! node's own codec.
use crate::{DeploymentIdentity, ReadClientSettings};
use super::*;
use crate::{Digest32, batch_monotonic_now};
use ingress_api::stream_batch::{
    FrameReader, ReadError, ReadProgress, ReadRecordRef, ReadReplyBody, StreamDirection, decode_read_request,
    encode_read_reply,
};
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use test_case::test_case;

const FRAME: u32 = 1 << 16;
const RETRY: u64 = 10;
const OPERATION: u64 = 100;
const START: u64 = 500;
const TIMEOUT: u64 = 1000;

#[derive(Clone)]
struct Clock(Arc<Mutex<Result<u64, ClockReadError>>>);
impl Clock {
    fn new() -> Self {
        Self(Arc::new(Mutex::new(Ok(START))))
    }
    fn set(&self, now: u64) {
        *self.0.lock().unwrap() = Ok(now);
    }
    /// Every later sample fails, until `set`.
    fn fail(&self) {
        *self.0.lock().unwrap() = Err(ClockReadError::OutOfRange);
    }
    fn sample(&self) -> Result<MonotonicMillis, ClockReadError> {
        (*self.0.lock().unwrap()).map(MonotonicMillis::new)
    }
}

fn owner(clock: &Clock, endpoints: u64, session: ReadSession) -> IngressReadOwner {
    let endpoints = (1..=endpoints)
        .map(|peer| (peer, format!("127.0.0.1:{}", 13000 + peer).parse().unwrap()))
        .collect();
    owner_on(clock, endpoints, session)
}

fn owner_on(clock: &Clock, endpoints: Vec<(u64, std::net::SocketAddr)>, session: ReadSession) -> IngressReadOwner {
    let clock = clock.clone();
    IngressReadOwner::bounded(
        ReadClientSettings::new(
            DeploymentIdentity::new(Digest32::new([11; 32]), Digest32::new([22; 32]), Digest32::new([33; 32])),
            NonZeroU128::new(9).unwrap(),
            endpoints,
            FRAME,
            session,
        )
        .with_retry_millis(RETRY)
        .with_operation_millis(OPERATION),
        move || clock.sample(),
    )
    .unwrap()
}

fn keys() -> Vec<ReadKey> {
    vec![ReadKey::SysId { kind: 3, sys_id: 7 }, ReadKey::UniqueKey { kind: 3, uk_id: 1, key: b"alice".to_vec() }]
}

fn read(minimum: Option<u64>) -> ReadQuery {
    ReadQuery { keys: keys(), min_applied_tx_seq: minimum }
}

/// One sent attempt: the peer of a connection opened for it, if any.
#[derive(Debug)]
struct Attempt {
    opened_to: Option<u64>,
    operation_id: u64,
    request: ReadRequest,
}

/// Drive once, connecting first when there is no connection, and return the
/// attempt the owner writes, after its write has completed.
#[track_caller]
fn next_attempt(owner: &mut IngressReadOwner) -> Attempt {
    let mut effect = owner.drive(ClientEvent::Drive);
    let mut opened_to = None;
    if let ClientResult::Connect { operation_id, peer_id, .. } = effect {
        opened_to = Some(peer_id);
        let connected = owner.drive(ClientEvent::Connected { operation_id, result: Ok(()) });
        assert!(matches!(connected, ClientResult::Waiting { .. }), "the stream is bound first: {connected:?}");
        effect = owner.drive(ClientEvent::Drive);
    }
    let ClientResult::Write { operation_id, request_id, bytes } = effect else {
        panic!("the owner sends the read's next attempt: {effect:?}; {owner:?}")
    };
    let request = decode_read_request(&bytes, FRAME as usize, CLIENT_READ_LIMITS).unwrap();
    assert_eq!(request.context.request_id, request_id);
    let written = owner.drive(ClientEvent::Written { operation_id, request_id, result: Ok(()) });
    assert!(matches!(written, ClientResult::Waiting { .. }), "{written:?}");
    Attempt { opened_to, operation_id, request }
}

fn observation(applied: u64) -> ReadObservation {
    ReadObservation::new(2, applied, 2, applied, None, ReadRole::Follower).unwrap()
}

fn record() -> ReadRecord {
    ReadRecord::new(3, 7, 2, vec![1, 2, 3])
}

fn deliver(owner: &mut IngressReadOwner, operation_id: u64, bytes: Vec<u8>) -> ClientResult {
    owner.drive(ClientEvent::Read { operation_id, result: Ok(bytes) })
}

/// The node's answer at `applied_tx_seq`: the record for the sys_id key and
/// none for the unique key.
fn answer(owner: &mut IngressReadOwner, attempt: &Attempt, applied_tx_seq: u64) -> ClientResult {
    let found = record();
    let records = [
        Some(ReadRecordRef { kind: found.kind, sys_id: found.sys_id, revision: found.revision, data: &found.data }),
        None,
    ];
    let body = ReadReplyBody::Answered { applied_tx_seq, tx_chain_hash: Digest32::new([5; 32]), records: &records };
    let bytes = encode_read_reply(&attempt.request.context, Some(observation(3)), body, FRAME as usize).unwrap();
    deliver(owner, attempt.operation_id, bytes)
}

fn refusal_observation(refusal: ReadRefusal) -> Option<ReadObservation> {
    (refusal != ReadRefusal::IncompatibleContext).then(|| observation(3))
}

fn refuse(owner: &mut IngressReadOwner, attempt: &Attempt, refusal: ReadRefusal) -> ClientResult {
    let bytes = encode_read_reply(
        &attempt.request.context,
        refusal_observation(refusal),
        ReadReplyBody::Refused(refusal),
        FRAME as usize,
    )
    .unwrap();
    deliver(owner, attempt.operation_id, bytes)
}

/// The owner retires this connection: Close, then the physical Closed.
#[track_caller]
fn retire(owner: &mut IngressReadOwner, effect: ClientResult, operation_id: u64) {
    let effect = match effect {
        ClientResult::ConnectionFault { .. } => owner.drive(ClientEvent::Drive),
        effect => effect,
    };
    assert!(matches!(effect, ClientResult::Close { operation_id: closed } if closed == operation_id), "{effect:?}");
    let closed = owner.drive(ClientEvent::Closed { operation_id });
    assert!(matches!(closed, ClientResult::Waiting { .. }), "{closed:?}");
}

#[track_caller]
fn assert_same_read(first: &ReadRequest, later: &ReadRequest) {
    assert_eq!(later.keys, first.keys, "every attempt sends the original keys");
    assert_eq!(later.min_applied_tx_seq, first.min_applied_tx_seq, "and the minimum fixed at acceptance");
    assert!(later.context.request_id > first.context.request_id, "with its own request id");
}

#[track_caller]
fn assert_waiting_until(owner: &mut IngressReadOwner, wake: u64) {
    let effect = owner.drive(ClientEvent::Drive);
    assert!(
        matches!(effect, ClientResult::Waiting { wake_at: Some(at) } if at.get() == wake),
        "expected a wait until {wake}: {effect:?}"
    );
}

#[test_case(ReadRefusal::NotVoter; "not_voter")]
#[test_case(ReadRefusal::LeaderReadsDisabled; "leader_reads_disabled")]
#[test_case(ReadRefusal::ReadsDisabled; "reads_disabled_on_one_endpoint")]
#[test_case(ReadRefusal::RecoveryPending; "recovery_pending")]
#[test_case(ReadRefusal::SnapshotInstalling; "snapshot_installing")]
#[test_case(ReadRefusal::NotYetVisible { applied_tx_seq: 4 }; "not_yet_visible")]
#[test_case(ReadRefusal::Busy; "busy")]
#[test_case(ReadRefusal::Draining; "draining")]
#[test_case(ReadRefusal::Fatal; "fatal")]
#[test_case(ReadRefusal::Unavailable; "unavailable")]
fn a_node_refusal_moves_the_read_to_the_next_endpoint_at_once(refusal: ReadRefusal) {
    let clock = Clock::new();
    let mut owner = owner(&clock, 2, ReadSession::Independent);
    owner.try_read(read(Some(5)), TIMEOUT).unwrap();
    let first = next_attempt(&mut owner);
    assert_eq!(first.opened_to, Some(1));

    let refused = refuse(&mut owner, &first, refusal);

    retire(&mut owner, refused, first.operation_id);
    let second = next_attempt(&mut owner);
    assert_eq!(second.opened_to, Some(2), "the next endpoint, with no wait inside a round");
    assert_same_read(&first.request, &second.request);
    assert!(owner.poll_event().is_none());
}

#[test_case(ReadRefusal::RequestTooLarge; "request_too_large")]
#[test_case(ReadRefusal::UnknownKind; "unknown_kind")]
#[test_case(ReadRefusal::UnknownUniqueKey; "unknown_unique_key")]
#[test_case(ReadRefusal::ReplyTooLarge; "reply_too_large")]
#[test_case(ReadRefusal::IncompatibleContext; "incompatible_context")]
#[test_case(ReadRefusal::Unrecognized { observed: u8::MAX, argument: 123 }; "unassigned reason and argument")]
fn a_refusal_no_other_endpoint_changes_returns_at_once(refusal: ReadRefusal) {
    let clock = Clock::new();
    let mut owner = owner(&clock, 2, ReadSession::Independent);
    let handle = owner.try_read(read(None), TIMEOUT).unwrap();
    let attempt = next_attempt(&mut owner);

    let refused = refuse(&mut owner, &attempt, refusal);

    assert!(matches!(refused, ClientResult::Waiting { wake_at: None }), "{refused:?}");
    let event = owner.poll_event().unwrap();
    assert!(
        matches!(&event, ReadEvent::Refused { handle: h, refusal: r, observation: o }
            if *h == handle && *r == refusal && *o == refusal_observation(refusal)),
        "{event:?}"
    );
    assert!(owner.status().connected, "the connection stays for the next read");
}

#[test]
fn a_correlated_frozen_kind_refusal_returns_the_read_and_keeps_its_connection() {
    let clock = Clock::new();
    let mut owner = owner(&clock, 1, ReadSession::Floor);
    let handle = owner.try_read(read(None), TIMEOUT).unwrap();
    let attempt = next_attempt(&mut owner);
    let refusal = IngressRefusal::new(1, 4, Some(attempt.request.context.request_id), 1, 1);

    let effect = deliver(&mut owner, attempt.operation_id, refusal.encode().to_vec());

    let event = owner.poll_event();
    assert!(matches!(event, Some(ReadEvent::IngressKindUnsupported { handle: h, refusal: r }) if h == handle && r == refusal),
        "the verified refusal must settle its read: {effect:?}, {event:?}");
    assert!(owner.poll_event().is_none());
    assert_eq!(owner.status().floor, Some(0));
    owner.try_read(read(None), TIMEOUT).unwrap();
    let next = next_attempt(&mut owner);
    assert_eq!((next.opened_to, next.operation_id), (None, attempt.operation_id));
    let _ = answer(&mut owner, &next, 9);
    assert!(matches!(owner.poll_event(), Some(ReadEvent::Answered { .. })));
}

#[test]
fn a_protocol_refusal_before_context_decode_settles_the_read_and_retires_the_socket() {
    let clock = Clock::new();
    let mut owner = owner(&clock, 1, ReadSession::Independent);
    let handle = owner.try_read(read(None), TIMEOUT).unwrap();
    let attempt = next_attempt(&mut owner);
    let refusal = IngressRefusal::new(1, 4, None, 2, 3);

    let close = deliver(&mut owner, attempt.operation_id, refusal.encode().to_vec());

    retire(&mut owner, close, attempt.operation_id);
    let event = owner.poll_event();
    assert!(matches!(event, Some(ReadEvent::IngressProtocolUnsupported { handle: h, refusal: r }) if h == handle && r == refusal),
        "{event:?}");
}

#[test_case(2, 4, Some(1); "another protocol")]
#[test_case(1, 6, Some(1); "another kind")]
#[test_case(1, 4, Some(2); "another request id")]
#[test_case(1, 4, None; "known protocol without the request id")]
fn a_mismatched_frozen_refusal_leaves_the_read_pending(protocol: u16, kind: u16, request_id: Option<u64>) {
    let clock = Clock::new();
    let mut owner = owner(&clock, 1, ReadSession::Independent);
    let handle = owner.try_read(read(None), TIMEOUT).unwrap();
    let attempt = next_attempt(&mut owner);
    assert_eq!(attempt.request.context.request_id.get(), 1);
    let refusal = IngressRefusal::new(protocol, kind, request_id.and_then(NonZeroU64::new), 1, 1);

    let observed = deliver(&mut owner, attempt.operation_id, refusal.encode().to_vec());

    assert!(matches!(observed, ClientResult::Waiting { .. }), "{observed:?}");
    assert!(owner.poll_event().is_none());
    let _ = answer(&mut owner, &attempt, 9);
    assert!(matches!(owner.poll_event(), Some(ReadEvent::Answered { handle: h, .. }) if handle == h));
}

#[test]
fn an_answer_returns_its_records_and_the_next_read_reuses_the_connection() {
    let clock = Clock::new();
    let mut owner = owner(&clock, 2, ReadSession::Independent);
    let handle = owner.try_read(read(Some(5)), TIMEOUT).unwrap();
    let first = next_attempt(&mut owner);

    let answered = answer(&mut owner, &first, 9);

    assert!(matches!(answered, ClientResult::Waiting { wake_at: None }), "{answered:?}");
    let ReadEvent::Answered { handle: answered, observation: seen, answer } = owner.poll_event().unwrap() else {
        panic!("the answer is the read's outcome")
    };
    assert_eq!(answered, handle);
    assert_eq!(seen, observation(3));
    assert_eq!((answer.applied_tx_seq, answer.records), (9, vec![Some(record()), None]));
    owner.try_read(read(None), TIMEOUT).unwrap();
    let second = next_attempt(&mut owner);
    assert_eq!((second.opened_to, second.operation_id), (None, first.operation_id));
}

#[test]
fn reads_disabled_is_returned_only_when_every_endpoint_of_one_round_refused_it() {
    let clock = Clock::new();
    let mut owner = owner(&clock, 3, ReadSession::Independent);
    let handle = owner.try_read(read(None), TIMEOUT).unwrap();
    // Round one: one endpoint is Busy, so the read waits out the round and
    // starts again at the first endpoint.
    for refusal in [ReadRefusal::ReadsDisabled, ReadRefusal::Busy, ReadRefusal::ReadsDisabled] {
        let attempt = next_attempt(&mut owner);
        let refused = refuse(&mut owner, &attempt, refusal);
        retire(&mut owner, refused, attempt.operation_id);
    }
    assert!(owner.poll_event().is_none());
    assert_waiting_until(&mut owner, START + RETRY);
    clock.set(START + RETRY);
    // Round two: every endpoint refuses ReadsDisabled.
    for _ in 0..2 {
        let attempt = next_attempt(&mut owner);
        let refused = refuse(&mut owner, &attempt, ReadRefusal::ReadsDisabled);
        retire(&mut owner, refused, attempt.operation_id);
    }
    let last = next_attempt(&mut owner);
    assert_eq!(last.opened_to, Some(3));
    let _ = refuse(&mut owner, &last, ReadRefusal::ReadsDisabled);

    let event = owner.poll_event().unwrap();
    assert!(
        matches!(event, ReadEvent::Refused { handle: h, refusal: ReadRefusal::ReadsDisabled, observation: Some(_) }
            if h == handle),
        "{event:?}"
    );
}

#[test]
fn round_waits_double_and_the_read_ends_at_its_deadline_not_after_a_wait() {
    let clock = Clock::new();
    let mut owner = owner(&clock, 1, ReadSession::Independent);
    let timeout = 100;
    let handle = owner.try_read(read(Some(5)), timeout).unwrap();
    let first = next_attempt(&mut owner);
    let mut attempt = first.operation_id;
    let mut previous = first.request.clone();
    let _ = refuse(&mut owner, &first, ReadRefusal::Busy);
    // One endpoint: every refusal ends a round. Waits are 10, 20 and 40.
    let mut now = START;
    for wait in [RETRY, 2 * RETRY, 4 * RETRY] {
        clock.set(now + wait - 1);
        assert_waiting_until(&mut owner, now + wait);
        now += wait;
        clock.set(now);
        let next = next_attempt(&mut owner);
        assert_eq!((next.opened_to, next.operation_id), (None, attempt), "the same endpoint keeps its connection");
        assert_same_read(&previous, &next.request);
        (attempt, previous) = (next.operation_id, next.request.clone());
        let _ = refuse(&mut owner, &next, ReadRefusal::Busy);
    }
    // The next wait, 80, would end after the deadline at 600: the read ends there.
    assert_eq!(now, START + 70);
    assert_waiting_until(&mut owner, START + timeout);
    clock.set(START + timeout);

    let ended = owner.drive(ClientEvent::Drive);

    assert!(matches!(ended, ClientResult::Waiting { wake_at: None }), "no attempt after the deadline: {ended:?}");
    let event = owner.poll_event().unwrap();
    assert!(
        matches!(event, ReadEvent::DeadlineExceeded { handle: h, last_refusal: Some(ReadRefusal::Busy) } if h == handle),
        "{event:?}"
    );
}

#[test]
fn round_waits_stop_doubling_at_five_seconds() {
    let clock = Clock::new();
    let mut owner = owner(&clock, 1, ReadSession::Independent);
    owner.try_read(read(None), 30_000).unwrap();
    let mut now = START;
    // One endpoint: every refusal ends a round.
    for wait in [10, 20, 40, 80, 160, 320, 640, 1280, 2560, 5000, 5000] {
        let attempt = next_attempt(&mut owner);
        let _ = refuse(&mut owner, &attempt, ReadRefusal::Busy);
        assert_waiting_until(&mut owner, now + wait);
        now += wait;
        clock.set(now);
    }
}

#[derive(Debug, Clone, Copy)]
enum Fault {
    ClosedWithoutReply,
    ReadError,
    WriteError,
    NoReplyWithinTheOperationBound,
}

#[test_case(Fault::ClosedWithoutReply; "closed_without_reply")]
#[test_case(Fault::ReadError; "read_error")]
#[test_case(Fault::WriteError; "write_error")]
#[test_case(Fault::NoReplyWithinTheOperationBound; "no_reply_within_the_operation_bound")]
fn a_connection_fault_moves_the_read_to_the_next_endpoint_through_the_reconnect_backoff(fault: Fault) {
    let clock = Clock::new();
    let mut owner = owner(&clock, 2, ReadSession::Independent);
    owner.try_read(read(Some(5)), TIMEOUT).unwrap();
    let connect = owner.drive(ClientEvent::Drive);
    let ClientResult::Connect { operation_id, peer_id: 1, .. } = connect else { panic!("{connect:?}") };
    let _ = owner.drive(ClientEvent::Connected { operation_id, result: Ok(()) });
    let ClientResult::Write { request_id, bytes, .. } = owner.drive(ClientEvent::Drive) else {
        panic!("the first attempt is written")
    };
    let first = decode_read_request(&bytes, FRAME as usize, CLIENT_READ_LIMITS).unwrap();
    let mut at = START;

    let faulted = match fault {
        Fault::WriteError => owner.drive(ClientEvent::Written {
            operation_id,
            request_id,
            result: Err(std::io::ErrorKind::BrokenPipe.into()),
        }),
        fault => {
            let _ = owner.drive(ClientEvent::Written { operation_id, request_id, result: Ok(()) });
            match fault {
                Fault::ClosedWithoutReply => {
                    owner.drive(ClientEvent::Read { operation_id, result: Err(ReadError::Closed) })
                }
                Fault::ReadError => owner.drive(ClientEvent::Read {
                    operation_id,
                    result: Err(ReadError::Io { source: std::io::ErrorKind::ConnectionReset.into() }),
                }),
                _ => {
                    at += OPERATION;
                    clock.set(at);
                    owner.drive(ClientEvent::Drive)
                }
            }
        }
    };

    assert!(matches!(faulted, ClientResult::ConnectionFault { operation_id: o, .. } if o == operation_id));
    retire(&mut owner, faulted, operation_id);
    assert_waiting_until(&mut owner, at + RETRY);
    clock.set(at + RETRY);
    let second = next_attempt(&mut owner);
    assert_eq!(second.opened_to, Some(2));
    assert_same_read(&first, &second.request);
}

#[test]
fn a_failed_connect_ends_the_round_and_the_wait_is_the_longer_backoff() {
    let clock = Clock::new();
    let mut owner = owner(&clock, 2, ReadSession::Independent);
    owner.try_read(read(None), TIMEOUT).unwrap();
    let first = next_attempt(&mut owner);
    let refused = refuse(&mut owner, &first, ReadRefusal::Busy);
    retire(&mut owner, refused, first.operation_id);
    let connect = owner.drive(ClientEvent::Drive);
    let ClientResult::Connect { operation_id, peer_id: 2, .. } = connect else { panic!("{connect:?}") };

    let failed =
        owner.drive(ClientEvent::Connected { operation_id, result: Err(std::io::ErrorKind::ConnectionRefused.into()) });

    assert!(matches!(failed, ClientResult::ConnectionFailed { .. }), "{failed:?}");
    // The round ended: its wait (10) and the reconnect backoff (10) agree.
    assert_waiting_until(&mut owner, START + RETRY);
    clock.set(START + RETRY);
    let again = next_attempt(&mut owner);
    assert_eq!(again.opened_to, Some(1), "the next round starts at the first endpoint");
    // A second failed connect in a row doubles the reconnect backoff (20),
    // longer than an in-round move.
    let refused = refuse(&mut owner, &again, ReadRefusal::Busy);
    retire(&mut owner, refused, again.operation_id);
    let ClientResult::Connect { operation_id, .. } = owner.drive(ClientEvent::Drive) else { panic!() };
    let _ =
        owner.drive(ClientEvent::Connected { operation_id, result: Err(std::io::ErrorKind::ConnectionRefused.into()) });
    assert_waiting_until(&mut owner, START + RETRY + 2 * RETRY);
}

#[test]
fn an_idle_connection_closed_by_its_peer_is_not_a_visit_of_the_waiting_read() {
    let clock = Clock::new();
    let mut owner = owner(&clock, 1, ReadSession::Independent);
    owner.try_read(read(None), TIMEOUT).unwrap();
    let first = next_attempt(&mut owner);
    // One endpoint: the refusal ends the round, and the read waits (10) on
    // the connection it keeps.
    let _ = refuse(&mut owner, &first, ReadRefusal::Busy);
    clock.set(START + 5);

    let closed = owner.drive(ClientEvent::Read { operation_id: first.operation_id, result: Err(ReadError::Closed) });

    retire(&mut owner, closed, first.operation_id);
    // The read keeps its round wait; the reconnect backoff (10) ends later.
    assert_waiting_until(&mut owner, START + 5 + RETRY);
    clock.set(START + 5 + RETRY);
    let second = next_attempt(&mut owner);
    let _ = refuse(&mut owner, &second, ReadRefusal::Busy);
    assert_waiting_until(&mut owner, START + 5 + RETRY + 2 * RETRY);
}

#[test]
fn a_reply_with_a_retired_request_id_keeps_the_reconnect_backoff() {
    let clock = Clock::new();
    let mut owner = owner(&clock, 1, ReadSession::Independent);
    owner.try_read(read(None), 50).unwrap();
    let ClientResult::Connect { operation_id, .. } = owner.drive(ClientEvent::Drive) else { panic!() };
    let _ =
        owner.drive(ClientEvent::Connected { operation_id, result: Err(std::io::ErrorKind::ConnectionRefused.into()) });
    clock.set(START + RETRY);
    // The failed connect doubled the reconnect backoff (20).
    let expired = next_attempt(&mut owner);
    clock.set(START + 50);
    let _ = owner.drive(ClientEvent::Drive);
    let _ = owner.poll_event().unwrap();
    owner.try_read(read(None), TIMEOUT).unwrap();
    let current = next_attempt(&mut owner);
    let _ = answer(&mut owner, &expired, 9);

    let faulted = owner.drive(ClientEvent::Read { operation_id: current.operation_id, result: Err(ReadError::Closed) });

    retire(&mut owner, faulted, current.operation_id);
    assert_waiting_until(&mut owner, START + 50 + 2 * RETRY);
}

#[derive(Debug, Clone, Copy)]
enum Mismatch {
    RecordCount,
    RecordSysId,
    BelowMinimum,
}

#[test_case(Mismatch::RecordCount; "one_record_for_two_keys")]
#[test_case(Mismatch::RecordSysId; "a_record_for_another_sys_id")]
#[test_case(Mismatch::BelowMinimum; "an_answer_below_the_minimum")]
fn an_answer_that_does_not_answer_the_read_is_a_connection_fault(mismatch: Mismatch) {
    let clock = Clock::new();
    let mut owner = owner(&clock, 2, ReadSession::Independent);
    owner.try_read(read(Some(5)), TIMEOUT).unwrap();
    let attempt = next_attempt(&mut owner);
    let found = record();
    let sys_id = if matches!(mismatch, Mismatch::RecordSysId) { 8 } else { found.sys_id };
    let records = [Some(ReadRecordRef { kind: found.kind, sys_id, revision: 1, data: &found.data }), None];
    let (records, applied_tx_seq) = match mismatch {
        Mismatch::RecordCount => (&records[..1], 9),
        Mismatch::RecordSysId => (&records[..], 9),
        Mismatch::BelowMinimum => (&records[..], 4),
    };
    let body = ReadReplyBody::Answered { applied_tx_seq, tx_chain_hash: Digest32::new([5; 32]), records };
    let bytes = encode_read_reply(&attempt.request.context, Some(observation(3)), body, FRAME as usize).unwrap();

    let faulted = deliver(&mut owner, attempt.operation_id, bytes);

    assert!(
        matches!(faulted, ClientResult::ConnectionFault { cause: ClientError::ReadReplyMismatch, .. }),
        "{faulted:?}"
    );
    assert!(owner.poll_event().is_none(), "the read continues at the next endpoint");
}

#[test]
fn the_session_floor_raises_later_minimums_and_an_accepted_read_keeps_its_own() {
    let clock = Clock::new();
    let mut owner = owner(&clock, 1, ReadSession::Floor);
    owner.try_read(read(None), TIMEOUT).unwrap();
    let first = next_attempt(&mut owner);
    assert_eq!(first.request.min_applied_tx_seq, None, "a fresh session has no floor");
    let _ = answer(&mut owner, &first, 40);
    let _ = owner.poll_event().unwrap();

    owner.try_read(read(None), TIMEOUT).unwrap();
    let after_answer = next_attempt(&mut owner);
    // A write observed while this read is held does not change its minimum.
    let raised = owner.observe_write(55);
    let _ = refuse(&mut owner, &after_answer, ReadRefusal::Busy);
    clock.set(START + RETRY);
    let retried = next_attempt(&mut owner);
    let _ = answer(&mut owner, &retried, 45);
    let _ = owner.poll_event().unwrap();
    let older = owner.observe_write(50);
    owner.try_read(read(Some(30)), TIMEOUT).unwrap();
    let after_write = next_attempt(&mut owner);
    let _ = answer(&mut owner, &after_write, 60);
    let _ = owner.poll_event().unwrap();
    // A read's own minimum is sent, but never enters the floor.
    owner.try_read(read(Some(70)), TIMEOUT).unwrap();
    let own_higher = next_attempt(&mut owner);

    assert_eq!(raised, ReadFloorResult::Raised { floor: 55 });
    assert_eq!(older, ReadFloorResult::Unchanged { floor: 55 });
    assert_eq!(after_answer.request.min_applied_tx_seq, Some(40), "raised to the answer's applied_tx_seq");
    assert_eq!(retried.request.min_applied_tx_seq, Some(40), "a retry keeps the read's minimum");
    assert_eq!(after_write.request.min_applied_tx_seq, Some(55), "raised to the observed write");
    assert_eq!(own_higher.request.min_applied_tx_seq, Some(70), "a higher own minimum stays");
    assert_eq!(owner.status().floor, Some(60));
}

#[test]
fn an_independent_session_sends_only_each_read_s_own_minimum() {
    let clock = Clock::new();
    let mut owner = owner(&clock, 1, ReadSession::Independent);
    owner.try_read(read(None), TIMEOUT).unwrap();
    let first = next_attempt(&mut owner);
    let _ = answer(&mut owner, &first, 40);
    let _ = owner.poll_event().unwrap();
    let observed = owner.observe_write(55);

    owner.try_read(read(None), TIMEOUT).unwrap();
    let second = next_attempt(&mut owner);

    assert_eq!(observed, ReadFloorResult::Independent);
    assert_eq!(second.request.min_applied_tx_seq, None);
    assert_eq!(owner.status().floor, None);
}

#[test]
fn a_late_reply_on_a_retired_connection_completes_nothing() {
    let clock = Clock::new();
    let mut owner = owner(&clock, 2, ReadSession::Independent);
    let handle = owner.try_read(read(None), TIMEOUT).unwrap();
    let first = next_attempt(&mut owner);
    let refused = refuse(&mut owner, &first, ReadRefusal::Busy);
    retire(&mut owner, refused, first.operation_id);
    let second = next_attempt(&mut owner);

    let late = answer(&mut owner, &first, 9);
    let late_fault =
        owner.drive(ClientEvent::Read { operation_id: first.operation_id, result: Err(ReadError::Closed) });

    assert!(matches!(late, ClientResult::Waiting { .. }), "{late:?}");
    assert!(
        matches!(late_fault, ClientResult::Waiting { .. }),
        "nor does its fault touch the current one: {late_fault:?}"
    );
    assert!(owner.poll_event().is_none(), "the retired connection's frame is discarded");
    let _ = answer(&mut owner, &second, 10);
    let event = owner.poll_event().unwrap();
    assert!(
        matches!(event, ReadEvent::Answered { handle: h, ref answer, .. } if h == handle && answer.applied_tx_seq == 10)
    );
}

#[test]
fn a_late_reply_with_a_retired_request_id_completes_nothing() {
    let clock = Clock::new();
    let mut owner = owner(&clock, 1, ReadSession::Independent);
    let expired = owner.try_read(read(None), 50).unwrap();
    let first = next_attempt(&mut owner);
    clock.set(START + 50);
    let _ = owner.drive(ClientEvent::Drive);
    let event = owner.poll_event().unwrap();
    assert!(matches!(event, ReadEvent::DeadlineExceeded { handle, last_refusal: None } if handle == expired));
    let current = owner.try_read(read(None), TIMEOUT).unwrap();
    let second = next_attempt(&mut owner);
    assert_eq!(second.operation_id, first.operation_id, "both on one connection");

    for refusal in [
        IngressRefusal::new(1, 4, Some(first.request.context.request_id), 1, 1),
        IngressRefusal::new(1, 4, None, 2, 3),
    ] {
        let observation = deliver(&mut owner, second.operation_id, refusal.encode().to_vec());
        assert!(matches!(observation, ClientResult::Waiting { .. }), "{observation:?}");
        assert!(owner.poll_event().is_none(), "a refusal for the expired request cannot settle its successor");
    }
    let late = answer(&mut owner, &first, 9);

    assert!(matches!(late, ClientResult::Waiting { .. }), "{late:?}");
    assert!(owner.poll_event().is_none(), "the earlier read's reply does not complete this one");
    let _ = answer(&mut owner, &second, 10);
    let event = owner.poll_event().unwrap();
    assert!(
        matches!(event, ReadEvent::Answered { handle, ref answer, .. } if handle == current && answer.applied_tx_seq == 10)
    );
}

#[test]
fn an_unfinished_write_keeps_its_own_bound_after_its_read_ends() {
    let clock = Clock::new();
    let mut owner = owner(&clock, 2, ReadSession::Independent);
    let expired = owner.try_read(read(None), 50).unwrap();
    let connect = owner.drive(ClientEvent::Drive);
    let ClientResult::Connect { operation_id, peer_id: 1, .. } = connect else { panic!("{connect:?}") };
    let _ = owner.drive(ClientEvent::Connected { operation_id, result: Ok(()) });
    let write = owner.drive(ClientEvent::Drive);
    assert!(matches!(write, ClientResult::Write { .. }), "{write:?}");
    // The peer takes no more bytes, and the read's deadline comes first.
    clock.set(START + 50);
    let _ = owner.drive(ClientEvent::Drive);
    let event = owner.poll_event().unwrap();
    assert!(matches!(event, ReadEvent::DeadlineExceeded { handle, last_refusal: None } if handle == expired));
    let current = owner.try_read(read(None), TIMEOUT).unwrap();

    assert_waiting_until(&mut owner, START + OPERATION);
    clock.set(START + OPERATION);
    let faulted = owner.drive(ClientEvent::Drive);

    assert!(
        matches!(faulted, ClientResult::ConnectionFault { operation_id: o, .. } if o == operation_id),
        "the write's bound faults its connection: {faulted:?}"
    );
    retire(&mut owner, faulted, operation_id);
    clock.set(START + OPERATION + RETRY);
    let next = next_attempt(&mut owner);
    assert_eq!(next.opened_to, Some(2), "the next read moves to the next endpoint");
    let _ = answer(&mut owner, &next, 10);
    let event = owner.poll_event().unwrap();
    assert!(matches!(event, ReadEvent::Answered { handle, .. } if handle == current), "{event:?}");
}

#[test]
fn one_read_is_held_at_a_time_and_stop_ends_it_and_retires_the_connection() {
    let clock = Clock::new();
    let mut owner = owner(&clock, 1, ReadSession::Independent);
    let handle = owner.try_read(read(None), TIMEOUT).unwrap();
    let attempt = next_attempt(&mut owner);
    let refused = owner.try_read(read(Some(1)), TIMEOUT).unwrap_err();
    assert!(matches!(refused.reason, ClientError::Full));
    assert_eq!(refused.query, read(Some(1)), "a refused read is returned whole");

    let stopped = owner.drive(ClientEvent::Stop);

    assert!(matches!(stopped, ClientResult::Close { operation_id } if operation_id == attempt.operation_id));
    let event = owner.poll_event().unwrap();
    assert!(matches!(event, ReadEvent::Stopped { handle: h, cause: ClientError::Stopped } if h == handle));
    let _ = owner.drive(ClientEvent::Closed { operation_id: attempt.operation_id });
    assert!(owner.status().stopped);
    assert!(matches!(owner.try_read(read(None), TIMEOUT).unwrap_err().reason, ClientError::Stopped));
}

#[derive(Debug, Clone, Copy)]
enum ClockFault {
    Unreadable,
    Regressed,
}

#[test_case(ClockFault::Unreadable; "unreadable")]
#[test_case(ClockFault::Regressed; "regressed")]
fn a_clock_that_stays_failed_ends_the_read_and_still_retires_the_connection(fault: ClockFault) {
    let clock = Clock::new();
    let mut owner = owner(&clock, 1, ReadSession::Independent);
    let handle = owner.try_read(read(None), TIMEOUT).unwrap();
    let attempt = next_attempt(&mut owner);
    match fault {
        ClockFault::Unreadable => clock.fail(),
        ClockFault::Regressed => clock.set(START - 1),
    }

    let failed = owner.drive(ClientEvent::Drive);
    let retired = owner.drive(ClientEvent::Drive);

    assert!(matches!(failed, ClientResult::ClockFailed { .. }), "{failed:?}");
    let event = owner.poll_event().unwrap();
    let cause_matches = |cause: &ClientError| match fault {
        ClockFault::Unreadable => matches!(cause, ClientError::InvalidTime { .. }),
        ClockFault::Regressed => matches!(cause, ClientError::TimeRegressed { .. }),
    };
    assert!(
        matches!(&event, ReadEvent::Stopped { handle: h, cause } if *h == handle && cause_matches(cause)),
        "{event:?}"
    );
    assert!(
        matches!(retired, ClientResult::Close { operation_id } if operation_id == attempt.operation_id),
        "the owed Close is sent with the clock still failed: {retired:?}"
    );
    let closed = owner.drive(ClientEvent::Closed { operation_id: attempt.operation_id });
    assert!(matches!(closed, ClientResult::ClockFailed { .. }), "{closed:?}");
    assert!(owner.status().stopped);
    assert!(!owner.status().connected);
}

#[test]
fn an_owner_holding_a_connection_cannot_be_handed_to_a_background() {
    let clock = Clock::new();
    let mut owner = owner(&clock, 1, ReadSession::Independent);
    let handle = owner.try_read(read(None), TIMEOUT).unwrap();
    let attempt = next_attempt(&mut owner);

    let (cause, mut owner) = match crate::ReadBackground::spawn(owner) {
        Err(refused) => refused,
        Ok(_) => panic!("a new driver holds no socket and cannot take over this one"),
    };

    assert!(matches!(cause, ClientError::BatchConnectionInvariant), "{cause:?}");
    let _ = answer(&mut owner, &attempt, 10);
    let event = owner.poll_event();
    assert!(matches!(event, Some(ReadEvent::Answered { handle: h, .. }) if h == handle), "returned whole: {event:?}");
}

#[test]
fn nothing_connects_until_a_read_is_accepted() {
    let clock = Clock::new();
    let mut owner = owner(&clock, 2, ReadSession::Independent);

    let idle = owner.drive(ClientEvent::Drive);

    assert!(matches!(idle, ClientResult::Waiting { wake_at: None }), "{idle:?}");
    owner.try_read(read(None), TIMEOUT).unwrap();
    assert_eq!(next_attempt(&mut owner).opened_to, Some(1));
}

/// Accept one connection and its first read request.
fn accept_one_read(listener: std::net::TcpListener) -> (std::net::TcpStream, ReadRequest) {
    let (mut peer, _) = listener.accept().unwrap();
    peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut reader = FrameReader::new(StreamDirection::Requests, FRAME as usize, 1).unwrap();
    loop {
        if let ReadProgress::Frame(bytes) = reader.poll_bytes(&mut peer).unwrap() {
            return (peer, decode_read_request(&bytes, FRAME as usize, CLIENT_READ_LIMITS).unwrap());
        }
    }
}

/// A one-connection server: it answers the first read with the record.
fn answer_one_read(listener: std::net::TcpListener) -> std::net::TcpStream {
    let (mut peer, request) = accept_one_read(listener);
    let found = record();
    let records = [
        Some(ReadRecordRef { kind: found.kind, sys_id: found.sys_id, revision: found.revision, data: &found.data }),
        None,
    ];
    let body = ReadReplyBody::Answered { applied_tx_seq: 9, tx_chain_hash: Digest32::new([5; 32]), records: &records };
    let reply = encode_read_reply(&request.context, Some(observation(3)), body, FRAME as usize).unwrap();
    peer.write_all(&reply).unwrap();
    peer
}

#[test]
fn the_background_reads_over_real_tcp_and_shutdown_returns_the_owner() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || answer_one_read(listener));
    let owner = IngressReadOwner::bounded(
        ReadClientSettings::new(
            DeploymentIdentity::new(Digest32::new([11; 32]), Digest32::new([22; 32]), Digest32::new([33; 32])),
            NonZeroU128::new(9).unwrap(),
            vec![(1, address)],
            FRAME,
            ReadSession::Floor,
        )
        .with_retry_millis(RETRY)
        .with_operation_millis(2_000),
        batch_monotonic_now,
    )
    .unwrap();
    let background = crate::ReadBackground::spawn(owner).unwrap();

    let handle = background.try_read(read(None), 5_000).unwrap();

    let deadline = Instant::now() + Duration::from_secs(5);
    let event = loop {
        if let Some(event) = background.poll_event().unwrap() {
            break event;
        }
        assert!(Instant::now() < deadline, "answered: {:?}", background.take_driver_error());
        std::thread::sleep(Duration::from_millis(1));
    };
    assert!(
        matches!(&event, ReadEvent::Answered { handle: h, answer, .. }
            if *h == handle && answer.records == vec![Some(record()), None]),
        "{event:?}"
    );
    let _peer = server.join().unwrap();
    let (owner, error) = background.shutdown().unwrap();
    assert!(error.is_none(), "{error:?}");
    assert_eq!(owner.status().floor, Some(9));
}

#[test]
fn after_the_clock_fails_the_background_closes_its_socket() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (received, request_received) = std::sync::mpsc::channel();
    let server = std::thread::spawn(move || {
        let (mut peer, _) = accept_one_read(listener);
        received.send(()).unwrap();
        // End of stream, not the 5 s read timeout: the client retired it.
        std::io::Read::read(&mut peer, &mut [0; 1])
    });
    let clock = Clock::new();
    let background =
        crate::ReadBackground::spawn(owner_on(&clock, vec![(1, address)], ReadSession::Independent)).unwrap();
    let handle = background.try_read(read(None), TIMEOUT).unwrap();
    request_received.recv_timeout(Duration::from_secs(5)).unwrap();

    clock.fail();
    let at_peer = server.join().unwrap();

    assert!(matches!(at_peer, Ok(0)), "the peer reads end of stream, not its read timeout: {at_peer:?}");
    let event = background.poll_event().unwrap();
    assert!(matches!(event, Some(ReadEvent::Stopped { handle: h, .. }) if h == handle), "{event:?}");
    let (owner, error) = background.shutdown().unwrap();
    assert!(matches!(error, Some(ClientError::InvalidTime { .. })), "{error:?}");
    assert!(owner.status().stopped);
}

#[test]
fn a_state_whose_relations_do_not_hold_is_refused_before_any_transition() {
    let clock = Clock::new();
    let mut owner = owner(&clock, 2, ReadSession::Independent);
    // A connection id the owner never issued.
    owner.state.connection = ReadConnection::Connected { operation_id: 9, peer: 0, write: None, first_unanswered: None };

    let driven = owner.drive(ClientEvent::Drive);
    let refused = owner.try_read(read(Some(5)), TIMEOUT).unwrap_err();

    assert!(
        matches!(
            driven,
            ClientResult::DriveRejected { event: ClientEvent::Drive, cause: ClientError::ReadStateInvariant }
        ),
        "{driven:?}"
    );
    assert!(matches!(refused.reason, ClientError::ReadStateInvariant));
    assert_eq!(refused.query, read(Some(5)), "the read is returned whole");
    assert!(!owner.status().occupied);
}

#[test_case(None; "scheduling while a reply is partial")]
#[test_case(NonZeroU64::new(71); "a write completed while a reply is partial")]
fn a_rejected_read_turn_returns_its_partial_reply_and_exact_write_receipt(written: Option<NonZeroU64>) {
    let clock = Clock::new();
    let mut owner = owner(&clock, 2, ReadSession::Independent);
    owner.state.connection = ReadConnection::Connected { operation_id: 9, peer: 0, write: None, first_unanswered: None };
    let mut reader = FrameReader::read_replies(FRAME as usize).unwrap();
    assert!(matches!(reader.poll_frame(&mut b"SVIN".as_slice()).unwrap(), ReadProgress::Runnable));
    let cursor = reader.incomplete_cursor().unwrap();
    let observed = owner.drive(ClientEvent::PartialReply { operation_id: 9, cursor, written });

    let ClientResult::DriveRejected {
        event: ClientEvent::PartialReply { operation_id: 9, cursor: returned, written: receipt },
        cause: ClientError::ReadStateInvariant,
    } = observed else { panic!("expected the original partial reply: {observed:?}") };
    assert_eq!(returned, cursor);
    assert_eq!(receipt, written);
}
