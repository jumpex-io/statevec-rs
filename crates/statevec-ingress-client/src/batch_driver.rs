// Included in port.rs: no owner, retry facts, slots or completion queue here.
use super::{ClientEvent, ClientResult, DrivenClient};

/// Mechanical connection/readiness seam. `connect` starts a nonblocking
/// operation; `connected` observes it, never waits. `wait` may block only for
/// the supplied duration or actual device readiness/notification. A virtual
/// implementation pairs with the owner's virtual clock. None means no socket.
pub trait BatchConnectIo {
    type Stream: Read + Write;
    fn connect(&mut self, address: SocketAddr) -> io::Result<Self::Stream>;
    fn connected(&mut self, stream: &mut Self::Stream) -> io::Result<Poll<()>>;
    fn wait(
        &mut self,
        stream: Option<&Self::Stream>,
        interests: (bool, bool),
        timeout: Option<Duration>,
    ) -> io::Result<(bool, bool)>;

    /// Wait for the ordinary stream and its optional discovery stream. Devices
    /// with a shared poller can register both. The default bounds waits on a
    /// single readiness source while an additional source is active.
    fn wait_pair(
        &mut self,
        sockets: [Option<(&Self::Stream, (bool, bool))>; 2],
        timeout: Option<Duration>,
    ) -> io::Result<[(bool, bool); 2]> {
        let [primary, probe] = sockets;
        let (stream, interests) = primary.map_or((None, (false, false)), |(socket, interests)| (Some(socket), interests));
        let Some((probe_socket, probe_interests)) = probe else {
            return self.wait(stream, interests, timeout).map(|ready| [ready, (false, false)]);
        };
        let probe_ready = self.wait(Some(probe_socket), probe_interests, Some(Duration::ZERO))?;
        let quantum = if probe_ready.0 || probe_ready.1 { Duration::ZERO } else { Duration::from_millis(5) };
        let primary_ready = self.wait(stream, interests, Some(timeout.map_or(quantum, |budget| budget.min(quantum))))?;
        Ok([primary_ready, probe_ready])
    }

    /// Physical elapsed-time source for reducing an already authorized wait
    /// after an I/O error. It never expires owner work. Virtual devices must
    /// provide the same elapsed-time axis as their owner clock.
    fn monotonic_now(&self) -> Instant;

    /// Consume the remainder of one failed manual wait without reusing its
    /// failed readiness mechanism. Zero is nonblocking. Virtual devices may
    /// record/advance this physical delay; it never authorizes owner work.
    /// Every implementation selects its own physical delay explicitly.
    fn pause_after_wait_error(&mut self, remaining: Duration);
}

/// One mechanical turn, not a batch outcome. Diagnostics report an input
/// already classified by the owner or a physical readiness-wait error. They
/// do not authorize Stop: keep driving the same owner, including its own
/// possible termination. Only `Waiting` permits parking. No report is retained
/// here; callers may render/coalesce diagnostics independently.
#[derive(Debug)]
pub enum BatchDriverStep {
    Runnable,
    Waiting,
    Diagnostic { cause: ClientError },
}

impl BatchDriverStep {
    pub fn is_runnable(&self) -> bool {
        !matches!(self, Self::Waiting)
    }
}

/// Manual, bounded mechanical driver. The caller retains the only client
/// owner. Each step services an unconsumed observation, otherwise it drives the
/// owner and visits each live socket at most once. It delivers at most one
/// physical observation. Waiting never consumes application events.
pub struct BatchDriverWorker<P: BatchConnectIo> {
    io: P,
    // The original stream is first. Once it retires, the remaining stream
    // occupies the first position. Selection and promotion belong to the owner.
    sockets: [Option<DriverSocket<P::Stream>>; 2],
    // One actual physical observation rejected before consumption (e.g. invariant failure),
    // not another inbox. Stop/Close disposes its resource with the connection.
    observation: Option<ClientEvent>,
}

enum DriverSocket<S: Read + Write> {
    Connecting { operation_id: u64, stream: S, ready: bool },
    Connected(BatchIoWorker<S>),
}

impl<S: Read + Write> DriverSocket<S> {
    fn operation_id(&self) -> u64 {
        match self {
            Self::Connecting { operation_id, .. } => *operation_id,
            Self::Connected(worker) => worker.operation_id,
        }
    }
}

impl<P: BatchConnectIo> BatchDriverWorker<P> {
    /// Assemble before the owner's first Drive. Do not independently deliver
    /// connection/byte events while this driver holds the physical resources.
    pub fn new(io: P) -> Self {
        Self { io, sockets: [None, None], observation: None }
    }

    fn drive<O: DrivenClient>(&self, client: &mut O, event: ClientEvent) -> ClientResult {
        let socket = match &event {
            ClientEvent::Drive => self.sockets[0].as_ref(),
            ClientEvent::Written { operation_id, .. } => self.socket_index(*operation_id).and_then(|i| self.sockets[i].as_ref()),
            _ => None,
        };
        let event = match socket {
            Some(DriverSocket::Connected(worker)) => worker.observe_partial_reply(event),
            _ => event,
        };
        client.drive(event)
    }

    fn socket_index(&self, operation_id: u64) -> Option<usize> {
        self.sockets.iter().position(|socket| socket.as_ref().is_some_and(|s| s.operation_id() == operation_id))
    }

    /// Only Waiting permits wait(). Owner-consumed faults are Diagnostic, not
    /// failed calls, and must not be reclassified by presentation code. Err is
    /// a rejected drive/assembly: retain both owner and driver. An unconsumed
    /// Read/Connected stays here until a later step or explicit Stop.
    pub fn step<O: DrivenClient>(&mut self, client: &mut O) -> Result<BatchDriverStep, ClientError> {
        if let Some(event) = self.observation.take() {
            return self.deliver(client, event);
        }
        let effect = self.drive(client, ClientEvent::Drive);
        let progress = self.execute(effect)?;
        if progress.is_runnable() {
            return Ok(progress);
        }
        for index in 0..self.sockets.len() {
            match self.sockets[index].as_mut() {
                Some(DriverSocket::Connecting { operation_id, stream, ready }) if *ready => {
                    let operation_id = *operation_id;
                    match self.io.connected(stream) {
                        Ok(Poll::Pending) => *ready = false,
                        Ok(Poll::Ready(())) => {
                            return self.deliver(client, ClientEvent::Connected { operation_id, result: Ok(()) });
                        }
                        Err(source) => {
                            drop(self.take_socket(index));
                            return self.deliver(client, ClientEvent::Connected { operation_id, result: Err(source) });
                        }
                    }
                }
                Some(DriverSocket::Connected(worker)) => match worker.poll() {
                    Poll::Ready(Some(event)) => return self.deliver(client, event),
                    Poll::Ready(None) => return Ok(BatchDriverStep::Runnable),
                    Poll::Pending => (),
                },
                _ => (),
            }
        }
        Ok(BatchDriverStep::Waiting)
    }

    /// Forward the explicit session-stop input; always dispose physical custody
    /// before returning Closed on the next step, even with a failed clock.
    pub fn stop<O: DrivenClient>(&mut self, client: &mut O) -> Result<(), ClientError> {
        let effect = client.drive(ClientEvent::Stop);
        self.execute(effect).map(|_| ())
    }

    /// A mechanical teardown projection, not proof that application results
    /// have been consumed. Useful when joining a stopped driver.
    pub fn is_quiescent(&self) -> bool {
        self.sockets.iter().all(Option::is_none) && self.observation.is_none()
    }

    /// Wait only after step returned Waiting (or after surfacing a driver error).
    /// max_wait is a host scheduling ceiling, never a product timeout. All
    /// socket registrations are scoped to wait and retirement is attempted
    /// before it returns. A physical wait error consumes the remaining wait
    /// budget through an independent pause, then reports the original diagnostic.
    /// This prevents repeated poller errors from spinning; max_wait = 0 remains
    /// explicitly nonblocking. Due owner effects run before either kind of wait.
    pub fn wait<O: DrivenClient>(
        &mut self,
        client: &mut O,
        max_wait: Duration,
    ) -> Result<BatchDriverStep, ClientError> {
        let (progress, timeout) = self.prepare_wait(client)?;
        if progress.is_runnable() {
            return Ok(progress);
        }
        let budget = timeout.map_or(max_wait, |due| due.min(max_wait));
        let started = self.io.monotonic_now();
        Ok(match self.wait_io(Some(budget)) {
            Ok(()) => BatchDriverStep::Runnable,
            Err(source) => {
                // Physical elapsed time only reduces this already-authorized
                // wait; it cannot expire, retry or retire owner work.
                let elapsed = self.io.monotonic_now().saturating_duration_since(started);
                self.io.pause_after_wait_error(budget.saturating_sub(elapsed));
                BatchDriverStep::Diagnostic { cause: ClientError::BatchWait { source } }
            }
        })
    }

    /// Prepare a physical wait through the same Drive ingress as step. The
    /// sole owner samples current time and may publish a due effect instead.
    /// Only Waiting permits parking; the returned duration is a scheduling
    /// projection, not authority to retry or settle. A runner can release its
    /// owner lock and call wait_io, then re-enter step after readiness or wake.
    pub fn prepare_wait<O: DrivenClient>(
        &mut self,
        client: &mut O,
    ) -> Result<(BatchDriverStep, Option<Duration>), ClientError> {
        if self.observation.is_some() {
            return Ok((BatchDriverStep::Runnable, Some(Duration::ZERO)));
        }
        let effect = self.drive(client, ClientEvent::Drive);
        if matches!(effect, ClientResult::Waiting { .. }) {
            Ok((BatchDriverStep::Waiting, client.wait_timeout()?))
        } else {
            self.execute(effect).map(|runnable| (runnable, Some(Duration::ZERO)))
        }
    }

    /// Wait using an already sampled scheduling duration. Used by background
    /// presentation after releasing the owner lock. A reliable notifier must
    /// interrupt this wait after publication; this does not authorize retries.
    pub fn wait_io(&mut self, timeout: Option<Duration>) -> io::Result<()> {
        let sockets = self.sockets.each_ref().map(|socket| match socket {
            Some(DriverSocket::Connecting { stream, .. }) => Some((stream, (false, true))),
            Some(DriverSocket::Connected(worker)) => Some((&worker.stream, worker.interests())),
            None => None,
        });
        let readiness = self.io.wait_pair(sockets, timeout)?;
        for (socket, (readable, writable)) in self.sockets.iter_mut().zip(readiness) {
            match socket {
                Some(DriverSocket::Connecting { ready, .. }) => *ready |= readable || writable,
                Some(DriverSocket::Connected(worker)) => worker.ready(readable, writable),
                None => (),
            }
        }
        Ok(())
    }

    fn deliver<O: DrivenClient>(&mut self, client: &mut O, event: ClientEvent) -> Result<BatchDriverStep, ClientError> {
        let connected = match &event {
            ClientEvent::Connected { operation_id, result: Ok(()) } => Some(*operation_id),
            _ => None,
        };
        let effect = self.drive(client, event);
        if let ClientResult::DriveRejected { event, cause } = effect {
            self.observation = Some(event);
            return Err(cause);
        }
        let progress = self.execute(effect)?;
        if matches!(progress, BatchDriverStep::Diagnostic { .. }) {
            return Ok(progress);
        }
        if let Some(index) = connected.and_then(|id| self.socket_index(id))
            && matches!(self.sockets[index], Some(DriverSocket::Connecting { .. }))
        {
            let Some(DriverSocket::Connecting { operation_id, stream, ready }) = self.sockets[index].take() else {
                unreachable!("matching stream is connecting")
            };
            match client.bind_stream(operation_id, stream) {
                Ok(worker) => self.sockets[index] = Some(DriverSocket::Connected(worker)),
                Err((stream, cause)) => {
                    self.sockets[index] = Some(DriverSocket::Connecting { operation_id, stream, ready });
                    return Err(cause);
                }
            }
        }
        Ok(BatchDriverStep::Runnable)
    }

    fn take_socket(&mut self, index: usize) -> Option<DriverSocket<P::Stream>> {
        let socket = self.sockets[index].take();
        if index == 0 {
            self.sockets.swap(0, 1);
        }
        socket
    }

    fn execute(&mut self, effect: ClientResult) -> Result<BatchDriverStep, ClientError> {
        match effect {
            ClientResult::Connect { operation_id, address, .. } => {
                let Some(index) = self.sockets.iter().position(Option::is_none) else {
                    return Err(ClientError::BatchConnectionInvariant);
                };
                if self.observation.is_some() || self.socket_index(operation_id).is_some() {
                    return Err(ClientError::BatchConnectionInvariant);
                }
                match self.io.connect(address) {
                    Ok(stream) => {
                        self.sockets[index] = Some(DriverSocket::Connecting { operation_id, stream, ready: true });
                    }
                    Err(source) => {
                        self.observation = Some(ClientEvent::Connected { operation_id, result: Err(source) })
                    }
                }
                Ok(BatchDriverStep::Runnable)
            }
            ClientResult::Close { operation_id } => {
                let socket = self.socket_index(operation_id).and_then(|index| self.take_socket(index));
                self.observation = Some(match socket {
                    Some(DriverSocket::Connected(worker)) => worker.close(),
                    _ => ClientEvent::Closed { operation_id },
                });
                Ok(BatchDriverStep::Runnable)
            }
            effect @ ClientResult::Write { operation_id, .. } => {
                let Some(DriverSocket::Connected(worker)) = self.socket_index(operation_id)
                    .and_then(|index| self.sockets[index].as_mut()) else {
                    return Err(ClientError::BatchConnectionInvariant);
                };
                worker
                    .try_write(effect)
                    .map_err(|_| ClientError::BatchConnectionInvariant)?;
                Ok(BatchDriverStep::Runnable)
            }
            ClientResult::Waiting { .. } => Ok(BatchDriverStep::Waiting),
            ClientResult::ConnectionFault { cause, .. } | ClientResult::ClockFailed { cause } => {
                Ok(BatchDriverStep::Diagnostic { cause })
            }
            ClientResult::ConnectionFailed { source, .. } => {
                Ok(BatchDriverStep::Diagnostic { cause: ClientError::BatchConnect { source } })
            }
            ClientResult::DriveRejected { cause, .. } => Err(cause),
        }
    }
}
