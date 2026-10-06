//! Connection and byte scheduling doubles; no alternate retry/reducer model.
use super::*;
use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::task::Poll;
use std::time::Duration;

#[path = "ut_batch_background.rs"]
mod background;

#[path = "ut_batch_driver_diagnostics.rs"]
mod diagnostics;

const STEPS: usize = 4096;

#[derive(Default)]
struct Device {
    incoming: VecDeque<u8>,
    outgoing: Vec<u8>,
    read_limit: Option<usize>,
    write_limit: Option<usize>,
    eof: bool,
    drops: usize,
    reads: usize,
    writes: usize,
    read_calls: Option<usize>,
    write_calls: Option<usize>,
    // Fault precisely between the Drive sample and delivery of a physical read.
    fail_clock_on_read: Option<Clock>,
    fail_clock_on_drop: Option<Clock>,
}
struct Bytes(Arc<Mutex<Device>>);
impl Read for Bytes {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        let mut device = self.0.lock().unwrap();
        device.reads += 1;
        if let Some(remaining) = &mut device.read_calls {
            if *remaining == 0 {
                return Err(io::ErrorKind::WouldBlock.into());
            }
            *remaining -= 1;
        }
        if let Some(clock) = device.fail_clock_on_read.take() {
            *clock.0.lock().unwrap() = Err(ClockReadFailure::OutOfRange);
        }
        if device.incoming.is_empty() {
            return if device.eof { Ok(0) } else { Err(io::ErrorKind::WouldBlock.into()) };
        }
        let count = bytes
            .len()
            .min(device.incoming.len())
            .min(device.read_limit.unwrap_or(usize::MAX));
        for byte in &mut bytes[..count] {
            *byte = device.incoming.pop_front().unwrap();
        }
        Ok(count)
    }
}
impl Write for Bytes {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let mut device = self.0.lock().unwrap();
        device.writes += 1;
        if let Some(remaining) = &mut device.write_calls {
            if *remaining == 0 {
                return Err(io::ErrorKind::WouldBlock.into());
            }
            *remaining -= 1;
        }
        let count = bytes.len().min(device.write_limit.unwrap_or(usize::MAX));
        if count == 0 {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        device.outgoing.extend_from_slice(&bytes[..count]);
        Ok(count)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
impl Drop for Bytes {
    fn drop(&mut self) {
        let mut device = self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        device.drops += 1;
        if let Some(clock) = device.fail_clock_on_drop.take() {
            *clock.0.lock().unwrap() = Err(ClockReadFailure::OutOfRange);
        }
    }
}
#[derive(Default)]
struct Network {
    devices: VecDeque<Arc<Mutex<Device>>>,
    addresses: Vec<SocketAddr>,
    connect_ready: bool,
    connect_polls: usize,
    connect_error: Option<io::ErrorKind>,
    wait_error: Option<io::ErrorKind>,
    wait_error_delay: Duration,
    physical_now: Option<std::time::Instant>,
    failed_wait_pauses: Vec<Duration>,
    waits: Vec<(bool, bool, Option<Duration>)>,
}
struct ScriptIo(Arc<Mutex<Network>>);
impl BatchConnectIo for ScriptIo {
    type Stream = Bytes;
    fn connect(&mut self, address: SocketAddr) -> io::Result<Bytes> {
        let mut network = self.0.lock().unwrap();
        network.addresses.push(address);
        Ok(Bytes(network.devices.pop_front().expect("scripted physical socket")))
    }
    fn connected(&mut self, _stream: &mut Bytes) -> io::Result<Poll<()>> {
        let mut network = self.0.lock().unwrap();
        network.connect_polls += 1;
        if let Some(kind) = network.connect_error.take() {
            return Err(kind.into());
        }
        Ok(if network.connect_ready { Poll::Ready(()) } else { Poll::Pending })
    }
    fn wait(
        &mut self,
        stream: Option<&Bytes>,
        interests: (bool, bool),
        timeout: Option<Duration>,
    ) -> io::Result<(bool, bool)> {
        let mut network = self.0.lock().unwrap();
        network.waits.push((interests.0, interests.1, timeout));
        if let Some(kind) = network.wait_error.take() {
            let delay = network.wait_error_delay;
            *network.physical_now.as_mut().expect("scripted physical clock") += delay;
            return Err(kind.into());
        }
        let readable = interests.0
            && stream.is_some_and(|s| {
                let device = s.0.lock().unwrap();
                !device.incoming.is_empty() || device.eof
            });
        Ok((readable, interests.1 && network.connect_ready))
    }

    fn pause_after_wait_error(&mut self, remaining: Duration) {
        self.0.lock().unwrap().failed_wait_pauses.push(remaining);
    }

    fn monotonic_now(&self) -> std::time::Instant {
        self.0.lock().unwrap().physical_now.expect("scripted physical clock")
    }
}

fn driver(devices: &[Arc<Mutex<Device>>]) -> (BatchDriverWorker<ScriptIo>, Arc<Mutex<Network>>) {
    let network = Arc::new(Mutex::new(Network {
        devices: devices.iter().cloned().collect(),
        connect_ready: true,
        physical_now: Some(std::time::Instant::now()), // Epoch only; no elapsed samples.
        ..Network::default()
    }));
    (BatchDriverWorker::new(ScriptIo(network.clone())), network)
}
fn bytes() -> Arc<Mutex<Device>> {
    Arc::new(Mutex::new(Device::default()))
}

#[track_caller]
fn drain(driver: &mut BatchDriverWorker<ScriptIo>, client: &mut BatchClient) {
    for _ in 0..STEPS {
        if !driver.step(client).unwrap().is_runnable() {
            return;
        }
    }
    panic!("bounded physical driver must reach WouldBlock without spinning");
}

fn requests(device: &Arc<Mutex<Device>>) -> Vec<Request> {
    let mut device = device.lock().unwrap();
    let mut cursor = io::Cursor::new(std::mem::take(&mut device.outgoing));
    let mut reader = stream_batch::FrameReader::new(
        stream_batch::StreamDirection::Requests,
        FRAME_LIMIT as usize,
        COMMAND_LIMIT as usize,
    )
    .unwrap();
    let mut requests = Vec::new();
    for _ in 0..STEPS {
        match reader.poll_bytes(&mut cursor).unwrap() {
            stream_batch::ReadProgress::Frame(bytes) => requests
                .push(stream_batch::decode_request(&bytes, FRAME_LIMIT as usize, COMMAND_LIMIT as usize).unwrap()),
            stream_batch::ReadProgress::Runnable => (),
            stream_batch::ReadProgress::Eof => return requests,
            stream_batch::ReadProgress::WouldBlock => panic!("memory cursor never blocks"),
        }
    }
    panic!("scripted peer failed to decode bounded request stream");
}
fn enqueue(device: &Arc<Mutex<Device>>, reply: Reply) {
    device
        .lock()
        .unwrap()
        .incoming
        .extend(stream_batch::encode_reply(&reply, FRAME_LIMIT as usize, COMMAND_LIMIT as usize).unwrap());
}
fn ready(driver: &mut BatchDriverWorker<ScriptIo>, client: &mut BatchClient) {
    driver.wait(client, Duration::ZERO).unwrap();
}

#[test]
fn manual_driver_decodes_short_request_and_retains_duplicate_coalesced_terminal() {
    let clock = Clock::new();
    let mut client = client(&clock);
    let device = bytes();
    device.lock().unwrap().write_limit = Some(7);
    device.lock().unwrap().read_limit = Some(3);
    let (mut driver, _) = driver(&[device.clone()]);
    drain(&mut driver, &mut client);
    let handles: Vec<_> = (1..=1).map(|stream| client.try_submit(batch(stream)).unwrap()).collect();
    drain(&mut driver, &mut client);
    let received = requests(&device);
    assert_eq!(received.len(), 1);
    for (request, handle) in received.iter().zip(&handles) {
        assert_eq!(request.batch().commitment_v1(), handle.intent);
        for _ in 0..4 {
            enqueue(&device, completed(request));
        }
    }
    ready(&mut driver, &mut client); // All fragmented/coalesced frames on ONE edge.
    drain(&mut driver, &mut client);
    assert!(matches!(client.try_submit(batch(1)).unwrap_err().reason, ClientFailure::Full));
    for handle in handles {
        assert!(
            matches!(client.poll_event(), Some(BatchEvent::Terminal(BatchTerminal::Applied { handle: actual, .. })) if actual == handle)
        );
    }
    assert!(client.poll_event().is_none());
    driver.stop(&mut client).unwrap();
    assert_eq!(device.lock().unwrap().drops, 1, "Close disposes real bytes before Closed");
    drain(&mut driver, &mut client);
    assert!(driver.is_quiescent());
}

#[test]
fn pending_connect_requires_readiness_and_uses_the_owner_clock_deadline() {
    let clock = Clock::new();
    let mut client = client(&clock);
    let device = bytes();
    let (mut driver, network) = driver(&[device.clone()]);
    network.lock().unwrap().connect_ready = false;
    drain(&mut driver, &mut client);
    for _ in 0..8 {
        assert!(!driver.step(&mut client).unwrap().is_runnable());
    }
    assert_eq!(network.lock().unwrap().connect_polls, 1, "pending connect must not busy-poll");
    clock.set(INITIAL_TIME + 25);
    driver.wait(&mut client, Duration::from_secs(1)).unwrap();
    assert_eq!(network.lock().unwrap().waits.last(), Some(&(false, true, Some(Duration::from_millis(OPERATION - 25)))));
    clock.set(INITIAL_TIME + OPERATION);
    drain(&mut driver, &mut client);
    assert_eq!(device.lock().unwrap().drops, 1, "deadline closes the still-connecting socket");
    assert!(driver.is_quiescent());
}

#[test]
fn failed_connect_drops_socket_before_owner_return_and_preserves_backoff() {
    let clock = Clock::new();
    let mut client = client(&clock);
    let device = bytes();
    let (mut driver, network) = driver(&[device.clone()]);
    network.lock().unwrap().connect_error = Some(io::ErrorKind::ConnectionRefused);
    assert!(driver.step(&mut client).unwrap().is_runnable());
    assert!(
        matches!(driver.step(&mut client), Ok(BatchDriverStep::Diagnostic { cause: ClientFailure::BatchConnect { source } }) if source.kind() == io::ErrorKind::ConnectionRefused)
    );
    assert_eq!(device.lock().unwrap().drops, 1);
    assert!(
        matches!(client.status(), ClientStatus::Batch { connected: false, next_deadline: Some(at), .. } if at.get() == INITIAL_TIME + RETRY)
    );
    assert!(!driver.step(&mut client).unwrap().is_runnable());
}

#[test]
fn read_clock_failure_stops_without_replaying_the_frame() {
    let clock = Clock::new();
    let mut client = client(&clock);
    let device = bytes();
    let (mut driver, _) = driver(&[device.clone()]);
    drain(&mut driver, &mut client);
    let handle = client.try_submit(batch(1)).unwrap();
    drain(&mut driver, &mut client);
    enqueue(&device, completed(&requests(&device).remove(0)));
    ready(&mut driver, &mut client);
    assert!(driver.step(&mut client).unwrap().is_runnable());
    device.lock().unwrap().fail_clock_on_read = Some(clock.clone());
    assert!(matches!(
        driver.step(&mut client),
        Ok(BatchDriverStep::Diagnostic { cause: ClientFailure::InvalidTime { .. } })
    ));
    let reads = device.lock().unwrap().reads;
    assert!(
        matches!(client.poll_event(), Some(BatchEvent::Terminal(BatchTerminal::OutcomeUnknown { handle: actual, cause: ClientFailure::InvalidTime { .. }, .. })) if actual == handle),
        "untrusted reply time stops with original custody"
    );
    clock.set(INITIAL_TIME + 1);
    drain(&mut driver, &mut client);
    assert_eq!(device.lock().unwrap().reads, reads, "stopped read is never replayed or reread");
    assert!(driver.is_quiescent());
    assert_eq!(device.lock().unwrap().drops, 1);
    assert!(client.poll_event().is_none());
}

#[test]
fn partial_reply_is_discarded_on_reconnect_but_a_latched_terminal_survives() {
    let clock = Clock::new();
    let mut client = client(&clock);
    let old = bytes();
    let replacement = bytes();
    let (mut driver, _) = driver(&[old.clone(), replacement.clone()]);
    drain(&mut driver, &mut client);
    let handle = client.try_submit(batch(1)).unwrap();
    drain(&mut driver, &mut client);
    let sent = requests(&old);
    enqueue(&old, completed(&sent[0]));
    let mut partial =
        stream_batch::encode_reply(&completed(&sent[0]), FRAME_LIMIT as usize, COMMAND_LIMIT as usize).unwrap();
    partial.truncate(stream_batch::HEADER_BYTES + 2);
    old.lock().unwrap().incoming.extend(partial);
    old.lock().unwrap().eof = true;
    ready(&mut driver, &mut client);
    for _ in 0..STEPS {
        match driver.step(&mut client) {
            Ok(BatchDriverStep::Runnable) => (),
            Ok(BatchDriverStep::Diagnostic {
                cause: ClientFailure::BatchRead { source: stream_batch::ReadFailure::Io { source } },
            }) if source.kind() == io::ErrorKind::UnexpectedEof => {
                break;
            }
            other => panic!("expected partial-frame EOF: {other:?}"),
        }
    }
    drain(&mut driver, &mut client);
    assert_eq!(old.lock().unwrap().drops, 1);
    clock.set(INITIAL_TIME + RETRY);
    drain(&mut driver, &mut client);
    let retried = requests(&replacement);
    assert!(retried.is_empty(), "latched terminal must not be retransmitted");
    assert!(
        matches!(client.poll_event(), Some(BatchEvent::Terminal(BatchTerminal::Applied { handle: actual, .. })) if actual == handle)
    );
    assert!(client.poll_event().is_none());
}

#[test]
fn os_nonblocking_connect_and_scoped_readiness_registration_use_real_socket() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let clock = Clock::new();
    let mut client =
        construct(&clock, idle_start(), vec![(1, listener.local_addr().unwrap())], (RETRY, OPERATION, LIFETIME))
            .unwrap();
    let mut driver = BatchDriverWorker::new(BatchTcpWorker::new().unwrap());
    assert!(driver.step(&mut client).unwrap().is_runnable());
    let (mut peer, _) = listener.accept().unwrap();
    peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    peer.set_write_timeout(Some(Duration::from_secs(2))).unwrap();
    for _ in 0..STEPS {
        if !driver.step(&mut client).unwrap().is_runnable() {
            break;
        }
    }
    let handle = client.try_submit(batch(1)).unwrap();
    for _ in 0..STEPS {
        if !driver.step(&mut client).unwrap().is_runnable() {
            break;
        }
    }
    let mut reader = stream_batch::FrameReader::new(
        stream_batch::StreamDirection::Requests,
        FRAME_LIMIT as usize,
        COMMAND_LIMIT as usize,
    )
    .unwrap();
    let request = loop {
        if let stream_batch::ReadProgress::Frame(bytes) = reader.poll_bytes(&mut peer).unwrap() {
            break stream_batch::decode_request(&bytes, FRAME_LIMIT as usize, COMMAND_LIMIT as usize).unwrap();
        }
    };
    peer.write_all(
        &stream_batch::encode_reply(&completed(&request), FRAME_LIMIT as usize, COMMAND_LIMIT as usize).unwrap(),
    )
    .unwrap();
    driver.wait(&mut client, Duration::from_secs(2)).unwrap();
    for _ in 0..STEPS {
        if !driver.step(&mut client).unwrap().is_runnable() {
            break;
        }
    }
    assert!(
        matches!(client.poll_event(), Some(BatchEvent::Terminal(BatchTerminal::Applied { handle: actual, .. })) if actual == handle)
    );
    driver.stop(&mut client).unwrap();
    assert_eq!(peer.read(&mut [0]).unwrap(), 0, "wait deregisters before socket close");
    assert!(driver.step(&mut client).unwrap().is_runnable());
    assert!(driver.is_quiescent());
}

#[test]
fn wait_dispatches_a_due_close_without_blocking_or_discarding_its_return() {
    let clock = Clock::new();
    let mut client = client(&clock);
    let device = bytes();
    let (mut driver, network) = driver(&[device.clone()]);
    drain(&mut driver, &mut client);
    let handle = client.try_submit(batch(1)).unwrap();
    drain(&mut driver, &mut client);
    let waits = network.lock().unwrap().waits.len();
    clock.set(INITIAL_TIME + OPERATION);
    driver.wait(&mut client, Duration::from_millis(999)).unwrap();
    assert_eq!(network.lock().unwrap().waits.len(), waits, "due owner effect runs instead of physical wait");
    assert!(network.lock().unwrap().failed_wait_pauses.is_empty(), "due Close cannot be padded by a fallback pause");
    assert_eq!(device.lock().unwrap().drops, 1);
    assert!(!driver.is_quiescent(), "actual Closed still belongs to the mechanical driver");
    assert!(driver.step(&mut client).unwrap().is_runnable());
    assert!(driver.is_quiescent());
    assert!(client.poll_event().is_none(), "operation timeout is not a terminal proof");
    driver.stop(&mut client).unwrap();
    assert!(matches!(client.poll_event(), Some(BatchEvent::Terminal(BatchTerminal::OutcomeUnknown {
        handle: actual, batch: original, ..
    })) if actual == handle && wire(&original) == wire(&batch(1))));
}

#[test_case::test_case(false; "unreadable_clock")]
#[test_case::test_case(true; "regressed_clock")]
fn wait_reenters_drive_before_projecting_timeout_and_stops_on_untrusted_time(regressed: bool) {
    let clock = Clock::new();
    let mut client = client(&clock);
    let device = bytes();
    let (mut driver, network) = driver(&[device.clone()]);
    drain(&mut driver, &mut client);
    let handle = client.try_submit(batch(1)).unwrap();
    drain(&mut driver, &mut client);
    *clock.0.lock().unwrap() =
        if regressed { Ok(MonotonicMillis::new(INITIAL_TIME - 1)) } else { Err(ClockReadFailure::OutOfRange) };
    assert!(
        matches!(
            driver.wait(&mut client, Duration::from_millis(999)),
            Ok(BatchDriverStep::Diagnostic {
                cause: ClientFailure::InvalidTime { .. } | ClientFailure::TimeRegressed { .. }
            })
        ),
        "wait must deliver current bound time through Drive before blocking"
    );
    assert!(network.lock().unwrap().failed_wait_pauses.is_empty(), "clock fail-stop cannot park before Close");
    assert!(
        matches!(client.poll_event(), Some(BatchEvent::Terminal(BatchTerminal::OutcomeUnknown { handle: actual, batch: original, .. })) if actual == handle && wire(&original) == wire(&batch(1)))
    );
    assert!(driver.step(&mut client).unwrap().is_runnable());
    assert!(matches!(
        driver.step(&mut client),
        Ok(BatchDriverStep::Diagnostic {
            cause: ClientFailure::InvalidTime { .. } | ClientFailure::TimeRegressed { .. }
        })
    ));
    assert!(driver.is_quiescent(), "clock failure cannot retain the physical connection");
    assert_eq!(device.lock().unwrap().drops, 1);
    clock.set(INITIAL_TIME + RETRY);
    assert!(!driver.step(&mut client).unwrap().is_runnable());
    assert!(client.poll_event().is_none());
}
