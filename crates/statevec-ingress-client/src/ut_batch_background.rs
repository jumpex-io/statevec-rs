//! The background/async presentations share the exact manual driver and owner.
//! Condvar gates control physical wait/notification, never request outcomes.
use super::*;
use crate::BatchBackground;
use std::future::Future;
use std::sync::Condvar;
use std::task::{Context, Wake, Waker};
use std::time::Instant;
use test_case::test_case;

const HARNESS_TIMEOUT: Duration = Duration::from_secs(3);

#[path = "ut_batch_byte_dst.rs"]
mod byte_dst;

#[derive(Default)]
struct Gate {
    notified: bool,
    waits: usize,
    credits: usize,
    released: bool,
    panic_on_release: bool,
    wait_error_on_release: bool,
    timeout: Option<Duration>,
    timer_expired: bool,
}
#[derive(Clone, Default)]
struct WakeGate(Arc<(Mutex<Gate>, Condvar)>);
impl WakeGate {
    fn notify(&self) {
        let (lock, cv) = &*self.0;
        lock.lock().unwrap().notified = true;
        cv.notify_all();
    }
    fn wait_entered(&self, count: usize) {
        let (lock, cv) = &*self.0;
        let (state, timed) = cv
            .wait_timeout_while(lock.lock().unwrap(), HARNESS_TIMEOUT, |state| state.waits < count)
            .unwrap();
        assert!(
            !timed.timed_out() && state.waits >= count,
            "background did not reach requested wait {count}, saw {}",
            state.waits
        );
    }
    fn release_one(&self) {
        let (lock, cv) = &*self.0;
        lock.lock().unwrap().credits += 1;
        cv.notify_all();
    }
    fn release_all(&self) {
        let (lock, cv) = &*self.0;
        lock.lock().unwrap().released = true;
        cv.notify_all();
    }
    fn expire_timer(&self) {
        let (lock, cv) = &*self.0;
        lock.lock().unwrap().timer_expired = true;
        cv.notify_all();
    }
}
struct GatedIo {
    io: ScriptIo,
    gate: WakeGate,
}
impl BatchConnectIo for GatedIo {
    type Stream = Bytes;
    fn connect(&mut self, address: SocketAddr) -> io::Result<Bytes> {
        self.io.connect(address)
    }
    fn connected(&mut self, stream: &mut Bytes) -> io::Result<Poll<()>> {
        self.io.connected(stream)
    }
    fn wait(
        &mut self,
        stream: Option<&Bytes>,
        interests: (bool, bool),
        timeout: Option<Duration>,
    ) -> io::Result<(bool, bool)> {
        let (lock, cv) = &*self.gate.0;
        let mut state = lock.lock().unwrap();
        state.waits += 1;
        state.timeout = timeout;
        cv.notify_all();
        let (next, timed) = cv
            .wait_timeout_while(state, HARNESS_TIMEOUT, |s| {
                !s.released && !s.timer_expired && (s.credits == 0 || !s.notified)
            })
            .unwrap();
        state = next;
        assert!(!timed.timed_out(), "harness must release physical wait; this is not a client timeout");
        state.credits = state.credits.saturating_sub(1);
        state.notified = false;
        state.timer_expired = false;
        let panic_on_release = std::mem::take(&mut state.panic_on_release);
        let wait_error_on_release = std::mem::take(&mut state.wait_error_on_release);
        drop(state);
        if panic_on_release {
            // Inject unwind without printing a fake assertion before the real
            // regression oracle (the negative runner checks that exact site).
            std::panic::resume_unwind(Box::new("injected physical wait panic"));
        }
        if wait_error_on_release {
            return Err(io::ErrorKind::Interrupted.into());
        }
        self.io.wait(stream, interests, timeout)
    }
}
// Always release the gate before Drop joins, including assertion/panic paths.
struct Running {
    gate: WakeGate,
    network: Arc<Mutex<Network>>,
    background: Option<BatchBackground>,
}
impl Running {
    fn client(&self) -> &BatchBackground {
        self.background.as_ref().unwrap()
    }
    fn client_mut(&mut self) -> &mut BatchBackground {
        self.background.as_mut().unwrap()
    }
    fn wait_or_finished(&self, count: usize) {
        let deadline = Instant::now() + HARNESS_TIMEOUT;
        while self.gate.0.0.lock().unwrap().waits < count && !self.client().driver_finished() {
            assert!(Instant::now() < deadline, "background must reach physical wait or finish retirement");
            std::thread::yield_now();
        }
    }
    fn shutdown(mut self) -> (BatchClient, Option<ClientFailure>) {
        self.client().request_stop();
        self.gate.release_all();
        self.background
            .take()
            .unwrap()
            .shutdown()
            .expect("fixture owner must remain intact")
    }
}
impl Drop for Running {
    fn drop(&mut self) {
        if let Some(background) = &self.background {
            background.request_stop();
        }
        self.gate.release_all();
    }
}
fn start(client: BatchClient, devices: &[Arc<Mutex<Device>>]) -> Running {
    let network = Arc::new(Mutex::new(Network {
        devices: devices.iter().cloned().collect(),
        connect_ready: true,
        ..Network::default()
    }));
    let gate = WakeGate::default();
    let notify = gate.clone();
    let io = GatedIo { io: ScriptIo(network.clone()), gate: gate.clone() };
    let background = BatchBackground::spawn_with_io(
        client,
        io,
        Arc::new(move || {
            notify.notify();
            Ok(())
        }),
    )
    .unwrap();
    gate.wait_entered(1); // Actual connect completed and both I/O lanes blocked.
    Running { gate, network, background: Some(background) }
}

#[derive(Debug, Clone, Copy)]
enum RecoverableFault {
    Eof,
    ReconnectRefused,
}

#[test_case(RecoverableFault::Eof; "eof_after_submit")]
#[test_case(RecoverableFault::ReconnectRefused; "connect_refused_during_recovery")]
fn async_recovery_keeps_physical_faults_out_of_the_terminal_channel(fault: RecoverableFault) {
    let clock = Clock::new();
    let old = bytes();
    let refused = bytes();
    let recovered = bytes();
    let devices = match fault {
        RecoverableFault::Eof => vec![old.clone(), recovered.clone()],
        RecoverableFault::ReconnectRefused => vec![old.clone(), refused.clone(), recovered.clone()],
    };
    let mut running = start(client(&clock), &devices);
    let handle = running.client().try_submit(batch(1)).unwrap();
    running.gate.release_one();
    running.gate.wait_entered(2);
    assert_eq!(requests(&old).len(), 1, "original Submit was physically sent before EOF");
    old.lock().unwrap().eof = true;
    if matches!(fault, RecoverableFault::ReconnectRefused) {
        running.network.lock().unwrap().connect_error = Some(io::ErrorKind::ConnectionRefused);
    }
    running.gate.notify();
    running.gate.release_one();
    running.gate.wait_entered(3);
    assert_eq!(old.lock().unwrap().drops, 1);
    let waits = if matches!(fault, RecoverableFault::ReconnectRefused) {
        // Consume the earlier EOF diagnostic so this row isolates the next
        // connect refusal, with the original batch still in owner custody.
        assert!(matches!(running.client().take_driver_error(), Some(ClientFailure::BatchRead { .. })));
        clock.set(INITIAL_TIME + RETRY);
        running.gate.expire_timer();
        running.gate.wait_entered(4);
        assert_eq!(refused.lock().unwrap().drops, 1);
        4
    } else {
        3
    };
    let event = std::pin::pin!(running.client_mut().next_event())
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()));
    assert!(
        event.is_pending(),
        "recoverable physical faults must remain diagnostics while the owner recovers: {event:?}"
    );
    match (fault, running.client().take_driver_error()) {
        (RecoverableFault::Eof, Some(ClientFailure::BatchRead { .. })) => (),
        (RecoverableFault::ReconnectRefused, Some(ClientFailure::BatchConnect { source })) => {
            assert_eq!(source.kind(), io::ErrorKind::ConnectionRefused);
        }
        other => panic!("the physical typed diagnostic remains independently consumable: {other:?}"),
    }
    clock.set(INITIAL_TIME + 2 * RETRY);
    running.gate.expire_timer();
    running.gate.wait_entered(waits + 1);
    let request = requests(&recovered).remove(0);
    assert_eq!(request.batch().commitment_v1(), batch(1).commitment_v1());
    enqueue(&recovered, completed(&request));
    running.gate.notify();
    running.gate.release_one();
    running.gate.wait_entered(waits + 2);
    let event = std::pin::pin!(running.client_mut().next_event())
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()));
    assert!(
        matches!(event, Poll::Ready(Ok(Some(BatchEvent::Terminal(BatchTerminal::Applied { handle: actual, .. }))))
        if actual == handle),
        "recovery must settle the original accepted handle"
    );
    let (mut owner, diagnostic) = running.shutdown();
    assert!(diagnostic.is_none());
    assert!(owner.poll_event().is_none());
    assert_eq!(recovered.lock().unwrap().drops, 1);
}

#[derive(Default)]
struct CountWake(Mutex<usize>, Condvar);
impl CountWake {
    fn wait(&self) {
        let (count, timed) = self
            .1
            .wait_timeout_while(self.0.lock().unwrap(), HARNESS_TIMEOUT, |n| *n == 0)
            .unwrap();
        assert!(!timed.timed_out() && *count != 0, "future must be notified without another application request");
    }
}
impl Wake for CountWake {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        *self.0.lock().unwrap() += 1;
        self.1.notify_all();
    }
}

#[test_case(1; "one_slot")]
fn stop_between_empty_poll_and_finished_delivers_every_terminal_before_eof(slot: u32) {
    let clock = Clock::new();
    let device = bytes();
    let mut running = start(client(&clock), &[device.clone()]);
    let handles: Vec<_> = (1..=slot)
        .map(|stream| running.client().try_submit(batch(stream)).unwrap())
        .collect();
    running.client().request_stop();
    assert!(running.client().poll_event().unwrap().is_none());

    // Let the actual driver finish Stop/Closed precisely after the empty read.
    let gate = running.gate.clone();
    running.client_mut().after_empty = Some(Box::new(move |finished| {
        gate.release_all();
        let deadline = Instant::now() + HARNESS_TIMEOUT;
        while !finished.load(std::sync::atomic::Ordering::Acquire) {
            assert!(Instant::now() < deadline, "actual driver must finish Stop/Closed");
            std::thread::yield_now();
        }
    }));
    for (stream, handle) in (1..=slot).zip(handles) {
        let observed = std::pin::pin!(running.client_mut().next_event())
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()));
        assert!(
            matches!(observed, Poll::Ready(Ok(Some(BatchEvent::Terminal(BatchTerminal::OutcomeUnknown {
                handle: actual, batch: original, ..
            })))) if actual == handle && wire(&original) == wire(&batch(stream))),
            "Stop must deliver each original terminal before async EOF"
        );
    }
    assert!(matches!(
        std::pin::pin!(running.client_mut().next_event())
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop())),
        Poll::Ready(Ok(None))
    ));
    let (mut owner, error) = running.shutdown();
    assert!(error.is_none(), "{error:?}");
    assert!(owner.poll_event().is_none(), "terminal custody must be consumed exactly once");
    assert!(matches!(owner.status(), ClientStatus::Batch { occupied: 0, .. }));
    assert_eq!(device.lock().unwrap().drops, 1);
}

#[test]
fn background_never_holds_the_owner_lock_while_waiting_or_consumes_terminals() {
    let clock = Clock::new();
    let device = bytes();
    device.lock().unwrap().write_limit = Some(5);
    device.lock().unwrap().read_limit = Some(3);
    let running = start(client(&clock), &[device.clone()]);
    // Thread is blocked in physical wait, not in a turn or an owner lock.
    let handles: Vec<_> = (1..=1).map(|id| running.client().try_submit(batch(id)).unwrap()).collect();
    assert!(matches!(running.client().try_submit(batch(1)).unwrap_err().reason, ClientFailure::Full));
    running.gate.release_one();
    running.gate.wait_entered(2);
    let sent = requests(&device);
    assert_eq!(sent.len(), 1);
    for request in &sent {
        enqueue(&device, completed(request));
    }
    running.gate.notify();
    running.gate.release_one();
    running.gate.wait_entered(3);
    assert!(
        matches!(running.client().status().unwrap(), ClientStatus::Batch { occupied: 1, .. }),
        "background may latch but never consume application results"
    );
    for handle in handles {
        assert!(
            matches!(running.client().poll_event().unwrap(), Some(BatchEvent::Terminal(BatchTerminal::Applied { handle: actual, .. })) if handle == actual)
        );
    }
    let (client, error) = running.shutdown();
    assert!(error.is_none(), "{error:?}");
    assert!(matches!(client.status(), ClientStatus::Batch { occupied: 0, stopped: true, .. }));
    assert_eq!(device.lock().unwrap().drops, 1);
}

#[test_case(true; "notification_before_waiter_registration")]
#[test_case(false; "notification_after_waiter_registration")]
fn async_wake_and_future_drop_leave_terminal_in_its_original_slot(reply_first: bool) {
    let clock = Clock::new();
    let device = bytes();
    let mut running = start(client(&clock), &[device.clone()]);
    let handle = running.client().try_submit(batch(1)).unwrap();
    running.gate.release_one();
    running.gate.wait_entered(2);
    let sent = requests(&device).remove(0);
    let wake = Arc::new(CountWake::default());
    let waker = Waker::from(wake.clone());
    let mut cx = Context::from_waker(&waker);
    {
        let mut abandoned = std::pin::pin!(running.client_mut().next_event());
        assert!(abandoned.as_mut().poll(&mut cx).is_pending());
    } // Dropped waiter must neither cancel nor consume.
    assert!(matches!(running.client().status().unwrap(), ClientStatus::Batch { occupied: 1, .. }));
    enqueue(&device, completed(&sent));
    if reply_first {
        running.gate.notify();
        running.gate.release_one();
        running.gate.wait_entered(3);
    }
    let gate = running.gate.clone();
    {
        let mut future = std::pin::pin!(running.client_mut().next_event());
        if !reply_first {
            assert!(future.as_mut().poll(&mut cx).is_pending());
            gate.notify();
            gate.release_one();
            gate.wait_entered(3);
            assert!(*wake.0.lock().unwrap() > 0, "registered waiter must be notified by the completed driver turn");
        }
        assert!(
            matches!(future.as_mut().poll(&mut cx), Poll::Ready(Ok(Some(BatchEvent::Terminal(BatchTerminal::Applied { handle: actual, .. })))) if actual == handle)
        );
    }
    let (client, error) = running.shutdown();
    assert!(error.is_none(), "{error:?}");
    assert!(matches!(client.status(), ClientStatus::Batch { occupied: 0, .. }));
}

#[test]
fn shutdown_joins_partial_io_and_returns_original_unknown_custody_even_with_bad_clock() {
    let clock = Clock::new();
    let device = bytes();
    device.lock().unwrap().write_limit = Some(0);
    let running = start(client(&clock), &[device.clone()]);
    let handle = running.client().try_submit(batch(1)).unwrap();
    running.gate.release_one();
    running.gate.wait_entered(2);
    assert!(device.lock().unwrap().outgoing.is_empty());
    *clock.0.lock().unwrap() = Err(ClockReadFailure::OutOfRange);
    let (mut client, error) = running.shutdown();
    assert!(matches!(error, Some(ClientFailure::InvalidTime { .. })));
    assert_eq!(device.lock().unwrap().drops, 1);
    assert!(
        matches!(client.poll_event(), Some(BatchEvent::Terminal(BatchTerminal::OutcomeUnknown { handle: actual, batch: original, .. })) if actual == handle && wire(&original) == wire(&batch(1)))
    );
    assert!(client.poll_event().is_none());
}

#[test]
fn panic_after_empty_diagnostic_is_delivered_before_async_eof() {
    let clock = Clock::new();
    let device = bytes();
    let mut running = start(client(&clock), &[device.clone()]);
    running.gate.0.0.lock().unwrap().panic_on_release = true;

    // The first diagnostic check sees None. Only then may the real wait
    // panic, retire the socket and publish its error followed by finished.
    let gate = running.gate.clone();
    running.client_mut().after_no_error = Some(Box::new(move |finished| {
        gate.release_all();
        let deadline = Instant::now() + HARNESS_TIMEOUT;
        while !finished.load(std::sync::atomic::Ordering::Acquire) {
            assert!(Instant::now() < deadline, "actual driver must retire after the wait panic");
            std::thread::yield_now();
        }
    }));

    let observed = std::pin::pin!(running.client_mut().next_event())
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()));
    assert!(
        matches!(observed, Poll::Ready(Err(ClientFailure::DriverPanicked))),
        "a published driver panic must precede async EOF: {observed:?}"
    );
    assert!(matches!(
        std::pin::pin!(running.client_mut().next_event())
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop())),
        Poll::Ready(Ok(None))
    ));
    let (mut owner, error) = running.shutdown();
    assert!(error.is_none(), "presentation error was consumed once");
    assert!(owner.poll_event().is_none());
    assert_eq!(device.lock().unwrap().drops, 1);
}

#[test]
fn driver_panic_overrides_an_earlier_diagnostic_without_poisoning_the_owner() {
    let clock = Clock::new();
    let device = bytes();
    let running = start(client(&clock), &[device.clone()]);
    let handle = running.client().try_submit(batch(1)).unwrap();
    // Unlike an untrusted owner clock, a physical wait error does not stop
    // the session. It leaves a real earlier diagnostic for panic to override.
    running.gate.0.0.lock().unwrap().wait_error_on_release = true;
    running.gate.release_one();
    running.gate.wait_entered(2); // Driver has recorded the earlier wait error.
    running.gate.0.0.lock().unwrap().panic_on_release = true;

    let (mut owner, error) = running.shutdown();
    assert!(matches!(error, Some(ClientFailure::DriverPanicked)), "driver panic must dominate earlier diagnostics");
    assert!(matches!(owner.poll_event(), Some(BatchEvent::Terminal(BatchTerminal::OutcomeUnknown {
        handle: actual, batch: original, ..
    })) if actual == handle && wire(&original) == wire(&batch(1))));
    assert_eq!(device.lock().unwrap().drops, 1);
}

#[test]
fn panicking_clock_quarantines_owner_instead_of_draining_untrusted_state() {
    let clock = Clock::new();
    let device = bytes();
    let mut running = start(client(&clock), &[device.clone()]);
    running.client().try_submit(batch(1)).unwrap();
    // A poisoned physical clock makes its next sample panic *inside* the real
    // owner entry. Do not fabricate/poison the owner mutex from the fixture.
    let _panic = std::panic::catch_unwind(|| {
        let _clock = clock.0.lock().unwrap();
        std::panic::resume_unwind(Box::new("injected clock callback failure"));
    });
    running.gate.release_all();
    let deadline = Instant::now() + HARNESS_TIMEOUT;
    while device.lock().unwrap().drops == 0 {
        assert!(Instant::now() < deadline, "panic must retire physical stream custody");
        std::thread::yield_now();
    }

    assert!(
        matches!(running.client().poll_event(), Err(ClientFailure::DriverPanicked)),
        "poisoned owner must not resume terminal consumption"
    );
    assert!(matches!(running.client().status(), Err(ClientFailure::DriverPanicked)));
    let refused = running.client().try_submit(batch(2)).unwrap_err();
    assert!(matches!(refused.reason, ClientFailure::DriverPanicked));
    assert_eq!(wire(&refused.batch), wire(&batch(2)));
    let quarantined = running
        .background
        .take()
        .unwrap()
        .shutdown()
        .expect_err("shutdown must not return a reusable poisoned owner");
    assert!(matches!(quarantined.take_driver_error(), Some(ClientFailure::DriverPanicked)));
    assert!(matches!(quarantined.poll_event(), Err(ClientFailure::DriverPanicked)));
    let refused = quarantined.try_import(batch(2), None).unwrap_err();
    assert!(matches!(refused.reason, ClientFailure::DriverPanicked));
    assert_eq!(wire(&refused.batch), wire(&batch(2)));
}

#[derive(Clone, Copy)]
enum LocalSubmission {
    Fresh,
    Imported,
}

#[test_case(LocalSubmission::Fresh; "submit")]
#[test_case(LocalSubmission::Imported; "import")]
fn local_lock_contention_is_not_admission_refusal_and_async_stays_pending(operation: LocalSubmission) {
    use std::sync::atomic::{AtomicBool, Ordering};
    let block = Arc::new(AtomicBool::new(false));
    let sample_block = block.clone();
    let (entered, sampled) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel();
    let owner = BatchClient::bounded(
        (Digest32::new([11; 32]), Digest32::new([22; 32]), Digest32::new([33; 32])),
        NonZeroU128::new(9).unwrap(),
        idle_start(),
        endpoints(),
        COMMAND_LIMIT,
        FRAME_LIMIT,
        OUTCOME_LIMIT,
        RETRY,
        OPERATION,
        LIFETIME,
        UNAVAILABLE_LIMIT,
        move || {
            if sample_block.swap(false, Ordering::AcqRel) {
                entered.send(()).unwrap();
                released
                    .recv_timeout(HARNESS_TIMEOUT)
                    .expect("fixture must release clock sample");
            }
            Ok(MonotonicMillis::new(INITIAL_TIME))
        },
    )
    .unwrap();
    let device = bytes();
    let mut running = start(owner, &[device]);
    block.store(true, Ordering::Release);
    running.gate.notify();
    running.gate.release_one();
    sampled
        .recv_timeout(HARNESS_TIMEOUT)
        .expect("driver must enter its owner-locked clock sample");

    assert!(
        std::pin::pin!(running.client_mut().next_event())
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
            .is_pending(),
        "async must not block on the owner lock"
    );
    // Release only AFTER the synchronous call has actually observed contention.
    running.client_mut().on_contention = Some(Box::new(move || release.send(()).unwrap()));
    let accepted = match operation {
        LocalSubmission::Fresh => running.client().try_submit(batch(1)),
        LocalSubmission::Imported => running.client().try_import(batch(1), None),
    };
    running.client_mut().on_contention = None;
    assert!(accepted.is_ok(), "local mutex contention must not refuse an admissible original: {accepted:?}");
    let handle = accepted.unwrap();
    let (mut owner, error) = running.shutdown();
    assert!(error.is_none(), "{error:?}");
    assert!(matches!(owner.poll_event(), Some(BatchEvent::Terminal(BatchTerminal::OutcomeUnknown {
        handle: actual, batch: original, ..
    })) if actual == handle && wire(&original) == wire(&batch(1))));
}

#[derive(Clone, Copy)]
enum WaitExpiry {
    Operation,
    Lifetime,
}

#[test_case(WaitExpiry::Operation; "operation_then_backoff")]
#[test_case(WaitExpiry::Lifetime; "lifetime_and_async_terminal")]
fn background_wait_uses_owner_deadline_without_application_or_socket_wakeup(expiry: WaitExpiry) {
    let clock = Clock::new();
    let lifetime = match expiry {
        WaitExpiry::Operation => LIFETIME,
        WaitExpiry::Lifetime => OPERATION,
    };
    let owner = construct(&clock, idle_start(), endpoints(), (RETRY, OPERATION, lifetime)).unwrap();
    let device = bytes();
    let mut running = start(owner, &[device.clone()]);
    let handles: Vec<_> = (1..=1)
        .map(|stream| running.client().try_submit(batch(stream)).unwrap())
        .collect();
    running.gate.release_one();
    running.gate.wait_entered(2);
    assert_eq!(requests(&device).len(), 1, "all exchanges must be physically written before the timer cut");
    let timeout = running.gate.0.0.lock().unwrap().timeout;
    assert_eq!(
        timeout,
        Some(Duration::from_millis(OPERATION)),
        "background wait must use the owner's earliest deadline, not the physical ceiling"
    );

    let wake = Arc::new(CountWake::default());
    let waker = Waker::from(wake.clone());
    let gate = running.gate.clone();
    let event = {
        let mut future = std::pin::pin!(running.client_mut().next_event());
        let mut cx = Context::from_waker(&waker);
        assert!(future.as_mut().poll(&mut cx).is_pending());
        clock.set(INITIAL_TIME + OPERATION);
        gate.expire_timer(); // No publish/notify or socket readiness rescues this wait.
        match expiry {
            WaitExpiry::Operation => {
                gate.wait_entered(3);
                let notifications = *wake.0.lock().unwrap();
                assert!(notifications > 0, "a timer-only owner turn must wake the async consumer");
                future.as_mut().poll(&mut cx)
            }
            WaitExpiry::Lifetime => {
                wake.wait();
                let deadline = Instant::now() + HARNESS_TIMEOUT;
                loop {
                    let event = future.as_mut().poll(&mut cx);
                    if event.is_ready() {
                        break event;
                    }
                    assert!(Instant::now() < deadline, "lifetime expiry must deliver its terminal");
                    std::thread::yield_now();
                }
            }
        }
    };
    if matches!(expiry, WaitExpiry::Lifetime) {
        running.wait_or_finished(3);
    }
    let drops = device.lock().unwrap().drops;
    assert_eq!(drops, 1, "deadline must physically retire the socket");
    match expiry {
        WaitExpiry::Operation => {
            assert!(event.is_pending(), "operation timeout does not settle an uncertain batch");
            assert!(running.client().poll_event().unwrap().is_none());
            let timeout = running.gate.0.0.lock().unwrap().timeout;
            assert_eq!(
                timeout,
                Some(Duration::from_millis(RETRY)),
                "disconnected backoff also uses the owner's earlier deadline"
            );
            assert!(matches!(running.client().status().unwrap(), ClientStatus::Batch { occupied: 1, .. }));
        }
        WaitExpiry::Lifetime => {
            assert!(matches!(event, Poll::Ready(Ok(Some(BatchEvent::Terminal(BatchTerminal::OutcomeUnknown {
                handle, cause: ClientFailure::RequestLifetimeExpired, ..
            })))) if handle == handles[0]));
            for handle in &handles[1..] {
                assert!(matches!(running.client().poll_event().unwrap(), Some(BatchEvent::Terminal(
                    BatchTerminal::OutcomeUnknown { handle: actual, cause: ClientFailure::RequestLifetimeExpired, .. }
                )) if actual == *handle));
            }
            assert!(matches!(
                running.client().status().unwrap(),
                ClientStatus::Batch { occupied: 0, stopped: true, .. }
            ));
        }
    }
    let (_owner, error) = running.shutdown();
    assert!(error.is_none(), "{error:?}");
}

#[test]
fn active_connection_cannot_be_replaced_by_background_assembly() {
    let clock = Clock::new();
    let mut original = client(&clock);
    connect(&mut original);
    let handle = original.try_submit(batch(1)).unwrap();
    let (cause, mut original) = match BatchBackground::spawn(original) {
        Err(failure) => failure,
        Ok(_) => panic!("assembly cannot discard a live socket's physical debt"),
    };
    assert!(matches!(cause, ClientFailure::BatchConnectionInvariant));
    let _effect = original.drive(ClientEvent::Stop);
    assert!(
        matches!(original.poll_event(), Some(BatchEvent::Terminal(BatchTerminal::OutcomeUnknown { handle: actual, .. })) if actual == handle)
    );
}

#[test_case(false; "no_prior_diagnostic")]
#[test_case(true; "waiter_failure_overrides_io_diagnostic")]
fn panicking_waiter_cannot_kill_the_driver_or_consume_the_result(prior_diagnostic: bool) {
    struct PanickingWake;
    impl Wake for PanickingWake {
        fn wake(self: Arc<Self>) {
            panic!("injected application waker panic");
        }
    }
    let clock = Clock::new();
    let device = bytes();
    let mut running = start(client(&clock), &[device.clone()]);
    let handle = running.client().try_submit(batch(1)).unwrap();
    running.gate.release_one();
    running.gate.wait_entered(2);
    let request = requests(&device).remove(0);
    let waker = Waker::from(Arc::new(PanickingWake));
    {
        let mut future = std::pin::pin!(running.client_mut().next_event());
        assert!(future.as_mut().poll(&mut Context::from_waker(&waker)).is_pending());
    }
    if prior_diagnostic {
        running.gate.0.0.lock().unwrap().wait_error_on_release = true;
        running.gate.notify();
        running.gate.release_one();
        running.gate.wait_entered(3);
    }
    enqueue(&device, completed(&request));
    running.gate.notify();
    running.gate.release_one();
    running.gate.wait_entered(if prior_diagnostic { 4 } else { 3 });
    let observed = std::pin::pin!(running.client_mut().next_event())
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()));
    assert!(
        matches!(observed, Poll::Ready(Ok(Some(BatchEvent::Terminal(BatchTerminal::Applied { handle: actual, .. })))) if actual == handle)
    );
    let diagnostic = std::pin::pin!(running.client_mut().next_event())
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()));
    assert!(
        matches!(diagnostic, Poll::Ready(Err(ClientFailure::WaiterPanicked))),
        "waiter failure cannot be hidden behind an earlier physical diagnostic"
    );
    let (_, error) = running.shutdown();
    assert!(error.is_none(), "{error:?}");
}

#[test]
fn os_background_wakes_on_publication_and_async_consumes_a_real_frame() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let owner =
        construct(&Clock::new(), idle_start(), vec![(1, listener.local_addr().unwrap())], (RETRY, OPERATION, LIFETIME))
            .unwrap();
    // Keep the injected source alive in the owner; no ambient-time retry here.
    assert!(matches!(owner.status(), ClientStatus::Batch { connected: false, .. }));
    let mut background = BatchBackground::spawn(owner).unwrap();
    let (mut peer, _) = listener.accept().unwrap();
    peer.set_read_timeout(Some(HARNESS_TIMEOUT)).unwrap();
    peer.set_write_timeout(Some(HARNESS_TIMEOUT)).unwrap();
    let deadline = Instant::now() + HARNESS_TIMEOUT;
    let mut original = batch(1);
    let handle = loop {
        match background.try_submit(original) {
            Ok(handle) => break handle,
            Err(refused) if matches!(refused.reason, ClientFailure::NotConnected) => original = refused.batch,
            Err(refused) => panic!("unexpected local refusal: {refused:?}"),
        }
        assert!(Instant::now() < deadline, "OS driver must observe the accepted socket");
        std::thread::yield_now();
    };
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
    let wake = Arc::new(CountWake::default());
    let waker = Waker::from(wake.clone());
    let mut cx = Context::from_waker(&waker);
    let response =
        stream_batch::encode_reply(&completed(&request), FRAME_LIMIT as usize, COMMAND_LIMIT as usize).unwrap();
    peer.write_all(&response).unwrap();
    {
        let mut future = std::pin::pin!(background.next_event());
        loop {
            if let Poll::Ready(event) = future.as_mut().poll(&mut cx) {
                assert!(
                    matches!(event.unwrap(), Some(BatchEvent::Terminal(BatchTerminal::Applied { handle: actual, .. })) if actual == handle)
                );
                break;
            }
            wake.wait();
            assert!(Instant::now() < deadline, "real reply must be consumed without a new submission");
        }
    }
    let (client, error) = background.shutdown().unwrap();
    assert!(error.is_none(), "{error:?}");
    assert!(matches!(client.status(), ClientStatus::Batch { occupied: 0, stopped: true, .. }));
    assert_eq!(peer.read(&mut [0]).unwrap(), 0);
}

#[test]
fn clock_fail_stop_finishes_background_and_delivers_terminal_before_async_eof() {
    let clock = Clock::new();
    let device = bytes();
    let mut running = start(client(&clock), &[device.clone()]);
    let handle = running.client().try_submit(batch(1)).unwrap();
    *clock.0.lock().unwrap() = Err(ClockReadFailure::OutOfRange);
    let gate = running.gate.clone();
    running.client_mut().after_empty = Some(Box::new(move |finished| {
        gate.release_all();
        let deadline = Instant::now() + HARNESS_TIMEOUT;
        while !finished.load(std::sync::atomic::Ordering::Acquire) {
            assert!(Instant::now() < deadline, "clock fail-stop must finish the background without explicit Stop");
            std::thread::yield_now();
        }
    }));
    let observed = std::pin::pin!(running.client_mut().next_event())
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()));
    assert!(
        matches!(observed, Poll::Ready(Ok(Some(BatchEvent::Terminal(BatchTerminal::OutcomeUnknown {
        handle: actual, batch: original, cause: ClientFailure::InvalidTime { .. }, ..
    })))) if actual == handle && wire(&original) == wire(&batch(1))),
        "clock fail-stop must deliver original custody before EOF"
    );
    assert!(matches!(
        std::pin::pin!(running.client_mut().next_event())
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop())),
        Poll::Ready(Ok(None))
    ));
    let (mut owner, cause) = running.shutdown();
    assert!(
        matches!(cause, Some(ClientFailure::InvalidTime { .. })),
        "clock diagnostic stays separate from its terminal"
    );
    assert!(owner.poll_event().is_none());
    assert_eq!(device.lock().unwrap().drops, 1);
}

#[test]
fn wait_sample_clock_failure_drains_close_before_blocking_again() {
    let remaining = Arc::new(Mutex::new(None::<usize>));
    let sample = remaining.clone();
    let owner = BatchClient::bounded(
        (Digest32::new([11; 32]), Digest32::new([22; 32]), Digest32::new([33; 32])),
        NonZeroU128::new(9).unwrap(),
        idle_start(),
        endpoints(),
        COMMAND_LIMIT,
        FRAME_LIMIT,
        OUTCOME_LIMIT,
        RETRY,
        OPERATION,
        LIFETIME,
        UNAVAILABLE_LIMIT,
        move || {
            if let Some(left) = &mut *sample.lock().unwrap() {
                if *left == 0 {
                    return Err(ClockReadFailure::OutOfRange);
                }
                *left -= 1;
            }
            Ok(MonotonicMillis::new(INITIAL_TIME))
        },
    )
    .unwrap();
    let device = bytes();
    let running = start(owner, &[device.clone()]);
    let handle = running.client().try_submit(batch(1)).unwrap();
    running.gate.release_one();
    running.gate.wait_entered(2);
    assert_eq!(requests(&device).len(), 1, "the request has actually been written");

    // step sees trusted time and no runnable I/O; only prepare_wait fails.
    *remaining.lock().unwrap() = Some(1);
    running.gate.expire_timer();
    running.wait_or_finished(3);
    let drops = device.lock().unwrap().drops;
    let waits = running.gate.0.0.lock().unwrap().waits;
    assert_eq!(drops, 1, "a newly due Close must run before another physical wait");
    assert_eq!(waits, 2, "clock fail-stop must finish without another wait or explicit Stop");
    assert!(running.client().driver_finished());
    let (mut owner, error) = running.shutdown();
    assert!(matches!(error, Some(ClientFailure::InvalidTime { source: ClockReadFailure::OutOfRange })));
    assert!(matches!(owner.poll_event(), Some(BatchEvent::Terminal(BatchTerminal::OutcomeUnknown {
        handle: actual, batch: original, cause: ClientFailure::InvalidTime { .. }, ..
    })) if actual == handle && wire(&original) == wire(&batch(1))));
    assert!(owner.poll_event().is_none());
}
