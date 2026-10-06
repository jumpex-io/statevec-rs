//! The peer scripts bytes and physical failures only. Real client transitions,
//! framing and request/reply codecs decide all correlation and custody.
use super::*;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::rc::Rc;
use std::task::Poll;
use test_case::test_case;

const POLL_BUDGET: usize = 4096;

#[derive(Debug)]
struct Device {
    incoming: VecDeque<u8>,
    outgoing: Vec<u8>,
    read_chunk: usize,
    write_chunk: usize,
    read_error: Option<io::ErrorKind>,
    write_error: Option<io::ErrorKind>,
    reads: usize,
    writes: usize,
    drops: usize,
}
impl Default for Device {
    fn default() -> Self {
        Self {
            incoming: VecDeque::new(),
            outgoing: Vec::new(),
            read_chunk: usize::MAX,
            write_chunk: usize::MAX,
            read_error: None,
            write_error: None,
            reads: 0,
            writes: 0,
            drops: 0,
        }
    }
}
#[derive(Debug)]
struct Bytes(Rc<RefCell<Device>>);
impl Read for Bytes {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        let mut device = self.0.borrow_mut();
        device.reads += 1;
        if let Some(kind) = device.read_error.take() {
            return Err(kind.into());
        }
        if device.incoming.is_empty() {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        let count = bytes.len().min(device.read_chunk).min(device.incoming.len());
        for byte in &mut bytes[..count] {
            *byte = device.incoming.pop_front().unwrap();
        }
        Ok(count)
    }
}
impl Write for Bytes {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let mut device = self.0.borrow_mut();
        device.writes += 1;
        if let Some(kind) = device.write_error.take() {
            return Err(kind.into());
        }
        let count = bytes.len().min(device.write_chunk);
        device.outgoing.extend_from_slice(&bytes[..count]);
        Ok(count)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
impl Drop for Bytes {
    fn drop(&mut self) {
        self.0.borrow_mut().drops += 1;
    }
}

fn bind(client: &BatchClient, operation_id: u64) -> (BatchIoWorker<Bytes>, Rc<RefCell<Device>>) {
    let device = Rc::new(RefCell::new(Device::default()));
    let port = client.bind_batch_stream(operation_id, Bytes(device.clone())).unwrap();
    (port, device)
}

#[track_caller]
fn dispatch(client: &mut BatchClient, port: &mut BatchIoWorker<Bytes>) -> Request {
    let effect = client.drive(ClientEvent::Drive);
    let ClientResult::Write { bytes, .. } = &effect else {
        panic!("expected owner write: {effect:?}");
    };
    let request = stream_batch::decode_request(bytes, FRAME_LIMIT as usize, COMMAND_LIMIT as usize).unwrap();
    port.try_write(effect).unwrap();
    request
}

#[track_caller]
fn next_event(port: &mut BatchIoWorker<Bytes>) -> ClientEvent {
    for _ in 0..POLL_BUDGET {
        match port.poll() {
            Poll::Ready(Some(event)) => return event,
            Poll::Ready(None) => (),
            Poll::Pending => panic!("script requires readiness; interests={:?}", port.interests()),
        }
    }
    panic!("bounded byte schedule did not return an observation");
}

fn enqueue(device: &Rc<RefCell<Device>>, reply: Reply) {
    device
        .borrow_mut()
        .incoming
        .extend(stream_batch::encode_reply(&reply, FRAME_LIMIT as usize, COMMAND_LIMIT as usize).unwrap());
}

#[derive(Clone, Copy, Debug)]
enum WriteReturnOrder {
    BeforeReply,
    AfterTerminalConsumption,
}

#[test_case(WriteReturnOrder::BeforeReply; "written_before_reply")]
#[test_case(WriteReturnOrder::AfterTerminalConsumption; "terminal_consumed_before_written")]
fn fragmented_coalesced_replies_preserve_independent_write_custody(order: WriteReturnOrder) {
    let clock = Clock::new();
    let mut client = client(&clock);
    let connection = connect(&mut client);
    let (mut port, device) = bind(&client, connection);
    device.borrow_mut().write_chunk = 7;
    device.borrow_mut().read_chunk = 3;
    let handle = client.try_submit(batch(1)).unwrap();
    let request = dispatch(&mut client, &mut port);
    let written = next_event(&mut port);
    assert!(matches!(&written, ClientEvent::Written { operation_id, request_id, result: Ok(()) }
        if *operation_id == connection && *request_id == request.context().request_id));
    assert_eq!(
        device.borrow().outgoing,
        stream_batch::encode_request(&request, FRAME_LIMIT as usize, COMMAND_LIMIT as usize).unwrap(),
        "short writes must emit exactly the original frame, without replayed prefixes"
    );
    let decoded =
        stream_batch::decode_request(&device.borrow().outgoing, FRAME_LIMIT as usize, COMMAND_LIMIT as usize).unwrap();
    assert_eq!(decoded.context(), request.context());
    assert_eq!(wire(decoded.batch()), wire(request.batch()), "scripted peer decodes the actual emitted bytes");
    let delayed = match order {
        WriteReturnOrder::BeforeReply => {
            assert!(matches!(client.drive(written), ClientResult::Waiting { .. }));
            None
        }
        WriteReturnOrder::AfterTerminalConsumption => Some(written),
    };
    // Both frames arrive on one readable edge; header/body and short-read yields
    // must not wait for an edge that will never arrive.
    enqueue(&device, reply(&request, ReplyDisposition::IngressAdmitted(entry())));
    enqueue(&device, completed(&request));
    port.ready(true, false);
    assert!(matches!(client.drive(next_event(&mut port)), ClientResult::Waiting { .. }));
    assert!(matches!(client.poll_event(), Some(BatchEvent::Progress(BatchProgress::IngressAdmitted { .. }))));
    assert!(matches!(client.drive(next_event(&mut port)), ClientResult::Waiting { .. }));
    assert!(matches!(client.status(), ClientStatus::Batch { occupied: 1, .. }), "terminal stays charged");
    assert!(matches!(client.poll_event(), Some(BatchEvent::Terminal(BatchTerminal::Applied { handle: actual, .. }))
        if actual == handle));
    client
        .try_submit(batch_at(InputRef { client_seq: 42, ..idle_start() }, 1))
        .unwrap();
    if let Some(written) = delayed {
        assert!(
            matches!(client.drive(ClientEvent::Drive), ClientResult::Waiting { .. }),
            "terminal consumption cannot release an unreturned physical write"
        );
        assert!(matches!(client.drive(written), ClientResult::Waiting { .. }));
    }
    let next = dispatch(&mut client, &mut port);
    assert_ne!(next.context().request_id, request.context().request_id);
    assert!(matches!(client.drive(next_event(&mut port)), ClientResult::Waiting { .. }));
}

#[test]
fn would_block_waits_for_readiness_but_short_writes_and_interruptions_remain_runnable() {
    let clock = Clock::new();
    let mut client = client(&clock);
    let connection = connect(&mut client);
    let (mut port, device) = bind(&client, connection);
    client.try_submit(batch(1)).unwrap();
    let request = dispatch(&mut client, &mut port);
    device.borrow_mut().write_error = Some(io::ErrorKind::WouldBlock);
    assert!(matches!(port.poll(), Poll::Ready(None))); // Read blocks, write is runnable.
    assert!(port.poll().is_pending());
    assert_eq!(port.interests(), (true, true));
    let counts = (device.borrow().reads, device.borrow().writes);
    for _ in 0..8 {
        assert!(port.poll().is_pending());
    }
    assert_eq!((device.borrow().reads, device.borrow().writes), counts, "WouldBlock must not busy-spin");
    device.borrow_mut().write_error = Some(io::ErrorKind::Interrupted);
    device.borrow_mut().write_chunk = 11;
    port.ready(false, true);
    assert!(matches!(port.poll(), Poll::Ready(None)), "Interrupted preserves writable progress");
    assert!(matches!(port.poll(), Poll::Ready(None)), "short write is not Written");
    assert!(matches!(client.drive(ClientEvent::Drive), ClientResult::Waiting { .. }));
    let written = next_event(&mut port);
    assert!(matches!(written, ClientEvent::Written { result: Ok(()), .. }));
    assert!(matches!(client.drive(written), ClientResult::Waiting { .. }));
    assert_eq!(device.borrow().reads, counts.0, "write readiness cannot spuriously poll blocked reads");
    enqueue(&device, completed(&request));
    assert!(port.poll().is_pending());
    port.ready(true, false);
    assert!(matches!(client.drive(next_event(&mut port)), ClientResult::Waiting { .. }));
    assert!(matches!(client.poll_event(), Some(BatchEvent::Terminal(BatchTerminal::Applied { .. }))));
}

#[test]
fn header_progress_keeps_the_byte_lane_runnable_without_another_readable_edge() {
    let clock = Clock::new();
    let mut client = client(&clock);
    let connection = connect(&mut client);
    let (mut port, device) = bind(&client, connection);
    client.try_submit(batch(1)).unwrap();
    let request = dispatch(&mut client, &mut port);
    assert!(matches!(client.drive(next_event(&mut port)), ClientResult::Waiting { .. }));
    enqueue(&device, completed(&request));
    port.ready(true, false);
    assert!(matches!(port.poll(), Poll::Ready(None)), "header progress must not require a second readiness edge");
    let Poll::Ready(Some(event @ ClientEvent::Read { result: Ok(_), .. })) = port.poll() else {
        panic!("the coalesced body must be delivered without another edge");
    };
    assert!(matches!(client.drive(event), ClientResult::Waiting { .. }));
    assert!(matches!(client.poll_event(), Some(BatchEvent::Terminal(BatchTerminal::Applied { .. }))));
}

#[test]
fn runnable_reads_cannot_starve_a_short_write_and_each_poll_has_one_io_call() {
    let clock = Clock::new();
    let mut client = client(&clock);
    let connection = connect(&mut client);
    let (mut port, device) = bind(&client, connection);
    client.try_submit(batch(1)).unwrap();
    let first = dispatch(&mut client, &mut port);
    assert!(matches!(client.drive(next_event(&mut port)), ClientResult::Waiting { .. }));
    receive(&mut client, connection, completed(&first));
    let _ = client.poll_event().unwrap();
    enqueue(&device, completed(&first)); // Old frame still exercises concurrent readable bytes.
    device.borrow_mut().read_chunk = 1;
    device.borrow_mut().write_chunk = 3;
    port.ready(true, false);
    client
        .try_submit(batch_at(InputRef { client_seq: 42, ..idle_start() }, 1))
        .unwrap();
    dispatch(&mut client, &mut port);
    let before = (device.borrow().reads, device.borrow().writes);
    for _ in 0..8 {
        assert!(matches!(port.poll(), Poll::Ready(None)));
    }
    assert_eq!(
        (device.borrow().reads - before.0, device.borrow().writes - before.1),
        (4, 4),
        "bounded polls must alternate runnable reads and writes"
    );
    assert!(client.poll_event().is_none());
    assert!(matches!(client.status(), ClientStatus::Batch { occupied: 1, .. }));
}

#[test]
fn coalesced_duplicate_terminals_do_not_release_capacity_until_consumed() {
    let clock = Clock::new();
    let mut client = client(&clock);
    let connection = connect(&mut client);
    let (mut port, device) = bind(&client, connection);
    let handles: Vec<_> = (1..=1).map(|id| client.try_submit(batch(id)).unwrap()).collect();
    let mut replies = Vec::new();
    for _ in 0..1 {
        let request = dispatch(&mut client, &mut port);
        assert!(matches!(client.drive(next_event(&mut port)), ClientResult::Waiting { .. }));
        for _ in 0..4 {
            replies.push(completed(&request));
        }
    }
    for reply in replies.into_iter().rev() {
        enqueue(&device, reply);
    }
    port.ready(true, false);
    for _ in 0..4 {
        assert!(matches!(client.drive(next_event(&mut port)), ClientResult::Waiting { .. }));
    }
    assert!(matches!(client.try_submit(batch(1)).unwrap_err().reason, ClientFailure::Full));
    for handle in handles {
        assert!(
            matches!(client.poll_event(), Some(BatchEvent::Terminal(BatchTerminal::Applied { handle: actual, .. }))
            if actual == handle)
        );
    }
    assert!(client.poll_event().is_none());
    assert!(matches!(client.status(), ClientStatus::Batch { occupied: 0, .. }));
}

#[derive(Clone, Copy, Debug)]
enum ReadFault {
    BadHeader,
    BadBody,
    PartialEof,
}

#[test_case(ReadFault::BadHeader; "header_rejected_before_body_allocation")]
#[test_case(ReadFault::BadBody; "body_decoded_only_by_owner")]
#[test_case(ReadFault::PartialEof; "partial_frame_lost_on_eof")]
fn typed_framing_failures_require_real_close_without_settling_intent(fault: ReadFault) {
    let clock = Clock::new();
    let mut client = client(&clock);
    let connection = connect(&mut client);
    let (mut port, device) = bind(&client, connection);
    let handle = client.try_submit(batch(1)).unwrap();
    let request = dispatch(&mut client, &mut port);
    assert!(matches!(client.drive(next_event(&mut port)), ClientResult::Waiting { .. }));
    let mut bytes =
        stream_batch::encode_reply(&completed(&request), FRAME_LIMIT as usize, COMMAND_LIMIT as usize).unwrap();
    match fault {
        ReadFault::BadHeader => bytes[0] ^= 0xff,
        ReadFault::BadBody => bytes[stream_batch::HEADER_BYTES..].fill(0),
        ReadFault::PartialEof => bytes.truncate(stream_batch::HEADER_BYTES + 3),
    }
    device.borrow_mut().incoming.extend(bytes);
    port.ready(true, false);
    if matches!(fault, ReadFault::PartialEof) {
        assert!(matches!(port.poll(), Poll::Ready(None)));
        assert!(matches!(port.poll(), Poll::Ready(None)));
        device.borrow_mut().read_chunk = 0;
        device.borrow_mut().incoming.push_back(0); // The Read double returns zero, not WouldBlock.
    }
    let event = next_event(&mut port);
    assert_eq!(device.borrow().drops, 0, "read failure is not physical retirement");
    let ClientResult::ConnectionFault { cause, .. } = client.drive(event) else {
        panic!("typed read fault");
    };
    match (fault, cause) {
        (ReadFault::BadHeader, ClientFailure::BatchRead { source: stream_batch::ReadFailure::Frame { .. } }) => (),
        (ReadFault::BadBody, ClientFailure::BatchFrame { .. }) => (),
        (ReadFault::PartialEof, ClientFailure::BatchRead { source: stream_batch::ReadFailure::Io { source } }) => {
            assert_eq!(source.kind(), io::ErrorKind::UnexpectedEof);
        }
        other => panic!("wrong failure boundary: {other:?}"),
    }
    assert!(
        matches!(client.drive(ClientEvent::Drive), ClientResult::Close { operation_id } if operation_id == connection)
    );
    let closed = port.close();
    assert_eq!(device.borrow().drops, 1, "Closed is issued only after disposal");
    assert!(matches!(client.drive(closed), ClientResult::Waiting { .. }));
    assert!(client.poll_event().is_none());
    assert!(matches!(client.drive(ClientEvent::Stop), ClientResult::Waiting { .. }));
    assert!(
        matches!(client.poll_event(), Some(BatchEvent::Terminal(BatchTerminal::OutcomeUnknown { handle: actual, batch: original, .. }))
        if actual == handle && wire(&original) == wire(&batch(1)))
    );
}

#[test]
fn completed_raw_frame_with_invalid_time_stops_without_settling_or_replay() {
    let clock = Clock::new();
    let mut client = client(&clock);
    let connection = connect(&mut client);
    let (mut port, device) = bind(&client, connection);
    let handle = client.try_submit(batch(1)).unwrap();
    let request = dispatch(&mut client, &mut port);
    assert!(matches!(client.drive(next_event(&mut port)), ClientResult::Waiting { .. }));
    enqueue(&device, completed(&request));
    port.ready(true, false);
    let event = next_event(&mut port);
    *clock.0.lock().unwrap() = Err(ClockReadFailure::OutOfRange);
    assert!(matches!(client.drive(event), ClientResult::ClockFailed { .. }));
    assert!(
        matches!(client.poll_event(), Some(BatchEvent::Terminal(BatchTerminal::OutcomeUnknown { handle: actual, cause: ClientFailure::InvalidTime { .. }, .. })) if actual == handle)
    );
    assert!(
        matches!(client.drive(ClientEvent::Drive), ClientResult::Close { operation_id } if operation_id == connection)
    );
    assert!(matches!(client.drive(port.close()), ClientResult::ClockFailed { .. }));
    clock.set(INITIAL_TIME + 1);
    assert!(matches!(client.drive(ClientEvent::Drive), ClientResult::Waiting { wake_at: None }));
    assert_eq!(device.borrow().drops, 1);
    assert!(client.poll_event().is_none());
}

#[test]
fn complete_reply_read_at_operation_deadline_still_retires_connection_without_settlement() {
    let clock = Clock::new();
    let mut client = client(&clock);
    let connection = connect(&mut client);
    let (mut port, device) = bind(&client, connection);
    client.try_submit(batch(1)).unwrap();
    let request = dispatch(&mut client, &mut port);
    assert!(matches!(client.drive(next_event(&mut port)), ClientResult::Waiting { .. }));
    enqueue(&device, completed(&request));
    port.ready(true, false);
    let frame = next_event(&mut port);
    clock.set(INITIAL_TIME + OPERATION);
    assert!(
        matches!(client.drive(frame), ClientResult::Close { operation_id } if operation_id == connection),
        "read readiness cannot outrank the owner's operation deadline"
    );
    assert!(client.poll_event().is_none());
    assert!(matches!(client.status(), ClientStatus::Batch { occupied: 1, stopped: false, .. }));
    assert!(matches!(client.drive(port.close()), ClientResult::Waiting { .. }));
}

#[test_case(io::ErrorKind::BrokenPipe; "failed_write")]
#[test_case(io::ErrorKind::WriteZero; "zero_progress_write")]
fn partial_write_failure_returns_exact_physical_debt_but_does_not_prove_non_admission(kind: io::ErrorKind) {
    let clock = Clock::new();
    let mut client = client(&clock);
    let connection = connect(&mut client);
    let (mut port, device) = bind(&client, connection);
    device.borrow_mut().write_chunk = 5;
    client.try_submit(batch(1)).unwrap();
    let request = dispatch(&mut client, &mut port);
    assert!(matches!(port.poll(), Poll::Ready(None)));
    assert!(matches!(port.poll(), Poll::Ready(None)));
    assert_eq!(device.borrow().outgoing.len(), 5);
    if kind == io::ErrorKind::WriteZero {
        device.borrow_mut().write_chunk = 0;
    } else {
        device.borrow_mut().write_error = Some(kind);
    }
    let event = next_event(&mut port);
    assert!(matches!(&event, ClientEvent::Written { operation_id, request_id, result: Err(source) }
        if *operation_id == connection && *request_id == request.context().request_id && source.kind() == kind));
    assert!(
        matches!(client.drive(event), ClientResult::ConnectionFault { cause: ClientFailure::BatchWrite { source }, .. }
        if source.kind() == kind)
    );
    assert!(client.poll_event().is_none());
    assert!(port.poll().is_pending(), "failed write reports its return only once");
    assert!(matches!(client.drive(ClientEvent::Drive), ClientResult::Close { .. }));
    assert!(matches!(client.drive(port.close()), ClientResult::Waiting { .. }));
    assert!(matches!(client.status(), ClientStatus::Batch { occupied: 1, .. }));
}

#[test]
fn binding_wrong_connection_returns_the_original_device_without_retirement_or_io() {
    let clock = Clock::new();
    let mut client = client(&clock);
    let connection = connect(&mut client);
    let device = Rc::new(RefCell::new(Device::default()));
    let (returned, cause) = match client.bind_batch_stream(connection + 1, Bytes(device.clone())) {
        Err(returned) => returned,
        Ok(_) => panic!("wrong physical connection cannot bind"),
    };
    assert!(matches!(cause, ClientFailure::NotConnected));
    assert_eq!((device.borrow().reads, device.borrow().writes, device.borrow().drops), (0, 0, 0));
    let port = client.bind_batch_stream(connection, returned).unwrap();
    assert!(matches!(client.drive(ClientEvent::Stop), ClientResult::Close { .. }));
    assert!(matches!(client.drive(port.close()), ClientResult::Waiting { .. }));
    assert_eq!(device.borrow().drops, 1);
}

#[test]
fn stop_disposes_partial_write_before_closed_and_keeps_the_original_uncertain_batch() {
    let clock = Clock::new();
    let mut client = client(&clock);
    let connection = connect(&mut client);
    let (mut port, device) = bind(&client, connection);
    device.borrow_mut().write_chunk = 1;
    let handle = client.try_submit(batch(1)).unwrap();
    dispatch(&mut client, &mut port);
    assert!(matches!(port.poll(), Poll::Ready(None)));
    assert!(matches!(port.poll(), Poll::Ready(None)));
    assert_eq!(device.borrow().outgoing.len(), 1);
    assert!(
        matches!(client.drive(ClientEvent::Stop), ClientResult::Close { operation_id } if operation_id == connection)
    );
    let closed = port.close();
    assert_eq!(device.borrow().drops, 1);
    assert!(matches!(client.drive(closed), ClientResult::Waiting { .. }));
    assert!(
        matches!(client.poll_event(), Some(BatchEvent::Terminal(BatchTerminal::OutcomeUnknown { handle: actual, batch: original, .. }))
        if actual == handle && wire(&original) == wire(&batch(1)))
    );
}

#[test]
fn busy_or_wrong_connection_write_is_returned_whole_and_never_queued() {
    let clock = Clock::new();
    let mut client = client(&clock);
    let connection = connect(&mut client);
    let (mut port, device) = bind(&client, connection);
    client.try_submit(batch(1)).unwrap();
    let first_effect = client.drive(ClientEvent::Drive);
    let ClientResult::Write { request_id, .. } = &first_effect else {
        panic!("first write");
    };
    let request_id = *request_id;
    let mut first_effect = Some(first_effect);
    for operation_id in [connection + 1, connection] {
        let bytes = vec![1, 2, 3];
        let allocation = bytes.as_ptr();
        let effect = ClientResult::Write { operation_id, request_id, bytes };
        let ClientResult::Write { bytes, operation_id: returned, .. } = port.try_write(effect).unwrap_err() else {
            panic!("exact refused effect");
        };
        assert_eq!(returned, operation_id);
        assert_eq!(bytes.as_ptr(), allocation);
        assert_eq!(bytes, [1, 2, 3]);
        if operation_id != connection {
            // Wrong connection is rejected even when the port is otherwise idle.
            // Then occupy the one write slot using the real owner's frame.
            port.try_write(first_effect.take().unwrap()).unwrap();
        }
    }
    assert!(device.borrow().outgoing.is_empty());
}
