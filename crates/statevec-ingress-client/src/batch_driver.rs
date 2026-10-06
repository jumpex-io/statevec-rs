// Included in port.rs: no owner, retry facts, slots or completion queue here.
use super::{ClientEvent, ClientResult, IngressClientOwner};

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

    /// Consume the remainder of one failed manual wait without reusing its
    /// failed readiness mechanism. Zero is nonblocking. Virtual devices may
    /// record/advance this physical delay; it never authorizes owner work.
    fn pause_after_wait_error(&mut self, remaining: Duration) {
        std::thread::sleep(remaining);
    }
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
    Diagnostic { cause: ClientFailure },
}

impl BatchDriverStep {
    pub fn is_runnable(&self) -> bool {
        !matches!(self, Self::Waiting)
    }
}

/// Manual, bounded mechanical driver. The caller retains the only client
/// owner. Each step first services an unconsumed observation, otherwise Drive
/// (including deadlines) followed by at most one physical I/O call. Waiting
/// never consumes application events or releases their slots.
#[statevec_domain_roles::physical_worker]
pub struct BatchDriverWorker<P: BatchConnectIo> {
    io: P,
    connecting: Option<(u64, P::Stream)>,
    connected: Option<BatchIoWorker<P::Stream>>,
    connect_ready: bool,
    // One actual physical observation rejected before consumption (e.g. invariant failure),
    // not another inbox. Stop/Close disposes its resource with the connection.
    observation: Option<ClientEvent>,
}

impl<P: BatchConnectIo> BatchDriverWorker<P> {
    /// Assemble before the owner's first Drive. Do not independently deliver
    /// connection/byte events while this driver holds the physical resources.
    pub fn new(io: P) -> Self {
        Self { io, connecting: None, connected: None, connect_ready: false, observation: None }
    }

    /// Only Waiting permits wait(). Owner-consumed faults are Diagnostic, not
    /// failed calls, and must not be reclassified by presentation code. Err is
    /// a rejected drive/assembly: retain both owner and driver. An unconsumed
    /// Read/Connected stays here until a later step or explicit Stop.
    pub fn step(&mut self, client: &mut IngressClientOwner) -> Result<BatchDriverStep, ClientFailure> {
        if let Some(event) = self.observation.take() {
            return self.deliver(client, event);
        }
        let effect = client.drive(ClientEvent::Drive);
        let progress = self.execute(client, effect)?;
        if progress.is_runnable() {
            return Ok(progress);
        }
        if self.connect_ready
            && let Some((operation_id, stream)) = &mut self.connecting
        {
            let operation_id = *operation_id;
            match self.io.connected(stream) {
                Ok(Poll::Pending) => self.connect_ready = false,
                Ok(Poll::Ready(())) => {
                    return self.deliver(client, ClientEvent::Connected { operation_id, result: Ok(()) });
                }
                Err(source) => {
                    // Failure certifies disposal, including registration/partial
                    // construction errors, before the owner's failed return.
                    drop(self.connecting.take());
                    return self.deliver(client, ClientEvent::Connected { operation_id, result: Err(source) });
                }
            }
        }
        if let Some(worker) = &mut self.connected {
            match worker.poll() {
                Poll::Ready(Some(event)) => return self.deliver(client, event),
                Poll::Ready(None) => return Ok(BatchDriverStep::Runnable),
                Poll::Pending => (),
            }
        }
        Ok(BatchDriverStep::Waiting)
    }

    /// Forward the explicit session-stop input; always dispose physical custody
    /// before returning Closed on the next step, even with a failed clock.
    pub fn stop(&mut self, client: &mut IngressClientOwner) -> Result<(), ClientFailure> {
        let effect = client.drive(ClientEvent::Stop);
        self.execute(client, effect).map(|_| ())
    }

    /// A mechanical teardown projection, not proof that application results
    /// have been consumed. Useful when joining a stopped driver.
    pub fn is_quiescent(&self) -> bool {
        self.connecting.is_none() && self.connected.is_none() && self.observation.is_none()
    }

    /// Wait only after step returned Waiting (or after surfacing a driver error).
    /// max_wait is a host scheduling ceiling, never a product timeout. All
    /// socket registrations are scoped to wait and retirement is attempted
    /// before it returns. A physical wait error consumes the remaining wait
    /// budget through an independent pause, then reports the original diagnostic.
    /// This prevents repeated poller errors from spinning; max_wait = 0 remains
    /// explicitly nonblocking. Due owner effects run before either kind of wait.
    pub fn wait(
        &mut self,
        client: &mut IngressClientOwner,
        max_wait: Duration,
    ) -> Result<BatchDriverStep, ClientFailure> {
        let (progress, timeout) = self.prepare_wait(client)?;
        if progress.is_runnable() {
            return Ok(progress);
        }
        let budget = timeout.map_or(max_wait, |due| due.min(max_wait));
        let started = Instant::now();
        Ok(match self.wait_io(Some(budget)) {
            Ok(()) => BatchDriverStep::Runnable,
            Err(source) => {
                // Physical elapsed time only reduces this already-authorized
                // wait; it cannot expire, retry or retire owner work.
                self.io.pause_after_wait_error(budget.saturating_sub(started.elapsed()));
                BatchDriverStep::Diagnostic { cause: ClientFailure::BatchWait { source } }
            }
        })
    }

    // Same Drive ingress as step: time is sampled once by the sole owner,
    // which may publish a due effect instead of authorizing a wait.
    pub(crate) fn prepare_wait(
        &mut self,
        client: &mut IngressClientOwner,
    ) -> Result<(BatchDriverStep, Option<Duration>), ClientFailure> {
        if self.observation.is_some() {
            return Ok((BatchDriverStep::Runnable, Some(Duration::ZERO)));
        }
        let effect = client.drive(ClientEvent::Drive);
        if matches!(effect, ClientResult::Waiting { .. }) {
            Ok((BatchDriverStep::Waiting, client.batch_wait_timeout()?))
        } else {
            self.execute(client, effect).map(|runnable| (runnable, Some(Duration::ZERO)))
        }
    }

    /// Wait using an already sampled scheduling duration. Used by background
    /// presentation after releasing the owner lock. A reliable notifier must
    /// interrupt this wait after publication; this does not authorize retries.
    pub fn wait_io(&mut self, timeout: Option<Duration>) -> io::Result<()> {
        let (stream, interests) = if let Some((_, stream)) = &self.connecting {
            (Some(stream), (false, true))
        } else if let Some(worker) = &self.connected {
            (Some(&worker.stream), worker.interests())
        } else {
            (None, (false, false))
        };
        let (readable, writable) = self.io.wait(stream, interests, timeout)?;
        self.connect_ready |= readable || writable;
        if let Some(worker) = &mut self.connected {
            worker.ready(readable, writable);
        }
        Ok(())
    }

    fn deliver(
        &mut self,
        client: &mut IngressClientOwner,
        event: ClientEvent,
    ) -> Result<BatchDriverStep, ClientFailure> {
        let connected = match &event {
            ClientEvent::Connected { operation_id, result: Ok(()) } => Some(*operation_id),
            _ => None,
        };
        let effect = client.drive(event);
        if let ClientResult::DriveRejected { event, cause } = effect {
            self.observation = Some(event);
            return Err(cause);
        }
        let progress = self.execute(client, effect)?;
        if matches!(progress, BatchDriverStep::Diagnostic { .. }) {
            return Ok(progress);
        }
        if let Some(operation_id) = connected
            && let Some((id, stream)) = self.connecting.take()
        {
            debug_assert_eq!(id, operation_id);
            match client.bind_batch_stream(operation_id, stream) {
                Ok(worker) => self.connected = Some(worker),
                Err((stream, cause)) => {
                    // Keep actual custody if a deadline rejected installation.
                    // The owner's Close is the only disposal instruction.
                    self.connecting = Some((id, stream));
                    return Err(cause);
                }
            }
        }
        Ok(BatchDriverStep::Runnable)
    }

    fn execute(
        &mut self,
        _client: &mut IngressClientOwner,
        effect: ClientResult,
    ) -> Result<BatchDriverStep, ClientFailure> {
        match effect {
            ClientResult::Connect { operation_id, address, .. } => {
                if !self.is_quiescent() {
                    return Err(ClientFailure::BatchConnectionInvariant);
                }
                match self.io.connect(address) {
                    Ok(stream) => {
                        self.connecting = Some((operation_id, stream));
                        self.connect_ready = true;
                    }
                    Err(source) => {
                        self.observation = Some(ClientEvent::Connected { operation_id, result: Err(source) })
                    }
                }
                Ok(BatchDriverStep::Runnable)
            }
            ClientResult::Close { operation_id } => {
                drop(self.connecting.take());
                drop(self.connected.take());
                self.observation = Some(ClientEvent::Closed { operation_id });
                self.connect_ready = false;
                Ok(BatchDriverStep::Runnable)
            }
            effect @ ClientResult::Write { .. } => {
                self.connected
                    .as_mut()
                    .ok_or(ClientFailure::BatchConnectionInvariant)?
                    .try_write(effect)
                    .map_err(|_| ClientFailure::BatchConnectionInvariant)?;
                Ok(BatchDriverStep::Runnable)
            }
            ClientResult::Waiting { .. } => Ok(BatchDriverStep::Waiting),
            ClientResult::ConnectionFault { cause, .. } | ClientResult::ClockFailed { cause } => {
                Ok(BatchDriverStep::Diagnostic { cause })
            }
            ClientResult::ConnectionFailed { source, .. } => {
                Ok(BatchDriverStep::Diagnostic { cause: ClientFailure::BatchConnect { source } })
            }
            ClientResult::DriveRejected { cause, .. } => Err(cause),
        }
    }
}
