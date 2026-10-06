//! Thread/async presentation over the exact same owner and manual driver.
//! No request mailbox, result queue, retry facts or mirrored client phase.
use crate::{
    BatchConnectIo, BatchDriverStep, BatchDriverWorker, BatchEvent, BatchHandle, BatchRefused, BatchTcpWorker,
    ClientFailure, ClientStatus, IngressClientOwner,
};
use statevec_frame::ClientStreamBatch;
use statevec_frame::stream_batch_protocol::EntryBinding;
use std::future::{Future, poll_fn};
use std::io;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{
    Arc, Mutex, MutexGuard,
    atomic::{AtomicBool, Ordering},
};
use std::task::{Poll, Waker};
use std::thread::JoinHandle;
use std::time::Duration;

// Physical wake-error observation ceiling, not a request/retry timeout.
const WAIT_CEILING: Duration = Duration::from_millis(250);

/// One exclusive presentation handle. The thread drives the existing owner;
/// synchronous calls serialize through a short local exclusion lock. Terminal values
/// remain in owner slots until poll_event/next_event consumes them. Async is a
/// Future view of this same background driver, not a second runtime/reducer.
///
/// Explicit shutdown joins physical retirement and returns the actual owner,
/// including unconsumed terminals. Drop requests Stop and joins, but (like
/// dropping a memory-only owner) discards any unconsumed application custody.
#[must_use = "retain the session, or shutdown to recover its terminal custody"]
pub struct BatchBackground {
    client: Arc<Mutex<IngressClientOwner>>,
    thread: Option<JoinHandle<()>>,
    stop: Arc<AtomicBool>,
    finished: Arc<AtomicBool>,
    notify: Arc<dyn Fn() -> io::Result<()> + Send + Sync>,
    error: Arc<Mutex<Option<ClientFailure>>>,
    waiter: Arc<Mutex<Option<Waker>>>,
    // Scheduling cut only: the real worker still owns Stop and completion.
    #[cfg(test)]
    pub(crate) after_empty: Option<Box<dyn FnOnce(&AtomicBool) + Send>>,
    #[cfg(test)]
    pub(crate) after_no_error: Option<Box<dyn FnOnce(&AtomicBool) + Send>>,
    #[cfg(test)]
    pub(crate) on_contention: Option<Box<dyn Fn() + Send + Sync>>,
}

impl std::fmt::Debug for BatchBackground {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never inspect a potentially interrupted owner to render a failure.
        f.debug_struct("BatchBackground")
            .field("finished", &self.finished.load(Ordering::Acquire))
            .field("owner_poisoned", &self.client.is_poisoned())
            .finish_non_exhaustive()
    }
}

impl BatchBackground {
    /// Start an unbound batch owner on real nonblocking TCP. OS construction
    /// failure returns the complete original owner; no fresh owner replaces it.
    pub fn spawn(client: IngressClientOwner) -> Result<Self, (ClientFailure, IngressClientOwner)> {
        let io = match BatchTcpWorker::new() {
            Ok(io) => io,
            Err(source) => return Err((ClientFailure::BatchWait { source }, client)),
        };
        let notify = io.notifier();
        Self::spawn_with_io(client, io, notify)
    }

    /// Physical substitution for deterministic tests/custom transports. The
    /// notifier must wake the matching wait, including publication before wait;
    /// wait must return within its supplied ceiling even if notification fails.
    /// Neither callback owns connection/retry/deadline or terminal decisions.
    pub fn spawn_with_io<P>(
        client: IngressClientOwner,
        io: P,
        notify: Arc<dyn Fn() -> io::Result<()> + Send + Sync>,
    ) -> Result<Self, (ClientFailure, IngressClientOwner)>
    where
        P: BatchConnectIo + Send + 'static,
        P::Stream: Send,
    {
        if let Err(cause) = client.check_driver_assembly() {
            return Err((cause, client));
        }
        let client = Arc::new(Mutex::new(client));
        let stop = Arc::new(AtomicBool::new(false));
        let finished = Arc::new(AtomicBool::new(false));
        let error = Arc::new(Mutex::new(None));
        let waiter = Arc::new(Mutex::new(None));
        let (owned, stopping, done, errors, waiting) =
            (client.clone(), stop.clone(), finished.clone(), error.clone(), waiter.clone());
        let spawned = std::thread::Builder::new().name("sv-ingress-client".into()).spawn(move || {
            let mut driver = BatchDriverWorker::new(io);
            let run = catch_unwind(AssertUnwindSafe(|| {
                loop {
                    let (runnable, timeout, stopped) = {
                        let mut client = match owned.lock() {
                            Ok(client) => client,
                            Err(_) => {
                                record(&errors, ClientFailure::DriverPanicked);
                                break;
                            }
                        };
                        let stopping = stopping.load(Ordering::Acquire);
                        // An idempotent Stop input, not wrapper-owned termination.
                        if stopping && let Err(cause) = driver.stop(&mut client) {
                            record(&errors, cause);
                        }
                        let runnable = match driver.step(&mut client) {
                            Ok(BatchDriverStep::Diagnostic { cause }) => {
                                record(&errors, cause);
                                true
                            }
                            Ok(value) => value.is_runnable(),
                            Err(cause) => {
                                record(&errors, cause);
                                false
                            }
                        };
                        let (runnable, timeout) = if runnable {
                            (true, Duration::ZERO)
                        } else {
                            match driver.prepare_wait(&mut client) {
                                Ok((BatchDriverStep::Diagnostic { cause }, _)) => {
                                    record(&errors, cause);
                                    // Owner-consumed fault may have made Close
                                    // immediately due. Do not park on a diagnostic.
                                    (true, Duration::ZERO)
                                }
                                Ok((progress, timeout)) => (
                                    progress.is_runnable(),
                                    timeout.map_or(WAIT_CEILING, |timeout| timeout.min(WAIT_CEILING)),
                                ),
                                Err(cause) => {
                                    record(&errors, cause);
                                    // Rejected drive/assembly, not an owner
                                    // decision creating a due Close. Bound the
                                    // wait even for persistent invariant errors.
                                    (false, WAIT_CEILING)
                                }
                            }
                        };
                        // Automatic lifetime/clock termination is owned by the same
                        // session. Do not keep a retired background alive waiting
                        // for an unrelated explicit request_stop.
                        let stopped = matches!(client.status(), ClientStatus::Batch { stopped: true, .. });
                        (runnable, timeout, stopped && driver.is_quiescent())
                    };
                    wake_waiter(&waiting, &errors);
                    if stopped {
                        break;
                    }
                    if runnable {
                        std::thread::yield_now();
                    } else if let Err(source) = driver.wait_io(Some(timeout)) {
                        record(&errors, ClientFailure::BatchWait { source });
                        // A failed poller is reported and parked at the physical
                        // ceiling; it cannot create a hot retry loop or a result.
                        std::thread::park_timeout(WAIT_CEILING);
                    }
                }
            }));
            if run.is_err() {
                record(&errors, ClientFailure::DriverPanicked);
                // A physical wait panic leaves the owner intact; a panic under
                // its lock may have interrupted a transition. Never resume the
                // latter just because catch_unwind returned. Drop the driver
                // in either case to retire sockets without classifying results.
                let _cleanup = catch_unwind(AssertUnwindSafe(|| {
                    let Ok(mut client) = owned.lock() else { return };
                    if let Err(cause) = driver.stop(&mut client) {
                        record(&errors, cause);
                    }
                    match driver.step(&mut client) {
                        Ok(BatchDriverStep::Diagnostic { cause }) | Err(cause) => record(&errors, cause),
                        Ok(_) => (),
                    }
                }));
            }
            drop(driver);
            done.store(true, Ordering::Release);
            wake_waiter(&waiting, &errors);
        });
        match spawned {
            Ok(thread) => Ok(Self {
                client,
                thread: Some(thread),
                stop,
                finished,
                notify,
                error,
                waiter,
                #[cfg(test)]
                after_empty: None,
                #[cfg(test)]
                after_no_error: None,
                #[cfg(test)]
                on_contention: None,
            }),
            Err(source) => Err((ClientFailure::DriverSpawn { source }, unwrap_client(client))),
        }
    }

    /// Serializes local owner access, never waits for bytes or a server reply.
    /// The exclusion wait has no hard wall-time bound; physical callbacks must
    /// remain nonblocking and socket readiness waits occur outside this lock.
    /// Time failure can stop the session even when this input is refused; the
    /// driver retires its socket while prior terminal custody remains consumable.
    pub fn try_submit(&self, batch: ClientStreamBatch) -> Result<BatchHandle, BatchRefused> {
        let result = match self.client() {
            Ok(mut client) => client.try_submit(batch),
            Err(reason) => Err(BatchRefused { reason, batch, committed: None }),
        };
        self.wake(); // Acceptance-time expiry can also owe Close after refusal.
        result
    }

    /// Import the original uncertain intent. As with try_submit, time failure
    /// stops the session and preserves prior terminal custody for consumption.
    pub fn try_import(
        &self,
        batch: ClientStreamBatch,
        committed: Option<EntryBinding>,
    ) -> Result<BatchHandle, BatchRefused> {
        let result = match self.client() {
            Ok(mut client) => client.try_import(batch, committed),
            Err(reason) => Err(BatchRefused { reason, batch, committed }),
        };
        self.wake();
        result
    }

    pub fn status(&self) -> Result<ClientStatus, ClientFailure> {
        Ok(self.client()?.status())
    }

    pub fn poll_event(&self) -> Result<Option<BatchEvent>, ClientFailure> {
        // Only an intact owner may consume a terminal, including after Stop.
        let event = self.client()?.poll_event();
        if event.is_some() {
            self.wake();
        }
        Ok(event)
    }

    #[cfg(test)]
    pub(crate) fn driver_finished(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }

    fn poll_async_event(&self) -> Poll<Result<Option<BatchEvent>, ClientFailure>> {
        let event = match self.client.try_lock() {
            Ok(mut client) => client.poll_event(),
            Err(std::sync::TryLockError::WouldBlock) => return Poll::Pending,
            Err(std::sync::TryLockError::Poisoned(_)) => return Poll::Ready(Err(ClientFailure::DriverPanicked)),
        };
        if event.is_some() {
            self.wake();
        }
        Poll::Ready(Ok(event))
    }

    /// Take the first coalesced physical/driver diagnostic. Presentation panics
    /// take precedence, with DriverPanicked strongest. Operational I/O errors
    /// are diagnostics; the owner alone decides recovery and batch terminals.
    /// An accepted batch is never refused because a later notification failed.
    pub fn take_driver_error(&self) -> Option<ClientFailure> {
        lock(&self.error).take()
    }

    fn take_presentation_error(&self) -> Option<ClientFailure> {
        let mut error = lock(&self.error);
        if matches!(error.as_ref(), Some(ClientFailure::DriverPanicked | ClientFailure::WaiterPanicked)) {
            error.take()
        } else {
            None
        }
    }

    /// Register before checking slots. Dropping this future consumes nothing,
    /// does not stop the driver, and does not release any accepted capacity.
    /// Err reports only DriverPanicked or WaiterPanicked. Recoverable connection
    /// faults remain available through take_driver_error while this future waits
    /// for owner events. Owner fail-stop causes travel with the retained terminal;
    /// all consumable terminals precede Ok(None), which marks driver completion.
    pub fn next_event(&mut self) -> impl Future<Output = Result<Option<BatchEvent>, ClientFailure>> + '_ {
        poll_fn(|cx| {
            let waker = match catch_unwind(AssertUnwindSafe(|| cx.waker().clone())) {
                Ok(waker) => waker,
                Err(_) => return Poll::Ready(Err(ClientFailure::WaiterPanicked)),
            };
            let previous = lock(&self.waiter).replace(waker);
            if catch_unwind(AssertUnwindSafe(|| drop(previous))).is_err() {
                return Poll::Ready(Err(ClientFailure::WaiterPanicked));
            }
            let result = match self.poll_async_event() {
                Poll::Ready(Ok(Some(event))) => Some(Ok(Some(event))),
                Poll::Ready(Ok(None)) => {
                    #[cfg(test)]
                    if let Some(cut) = self.after_empty.take() {
                        cut(&self.finished);
                    }
                    self.take_presentation_error().map(Err).or_else(|| {
                        #[cfg(test)]
                        if let Some(cut) = self.after_no_error.take() {
                            cut(&self.finished);
                        }
                        if !self.finished.load(Ordering::Acquire) {
                            return None;
                        }
                        // Stop may publish terminals between the empty read
                        // and finish. Acquire certifies no more publication,
                        // not consumption: observe the slots again before EOF.
                        match self.poll_async_event() {
                            Poll::Pending => None,
                            // The first error read may precede the thread's
                            // final publication. Acquire above makes that
                            // diagnostic visible too; only an empty owner can
                            // reach EOF, after its presentation error is read.
                            Poll::Ready(Ok(None)) => Some(self.take_presentation_error().map_or(Ok(None), Err)),
                            Poll::Ready(result) => Some(result),
                        }
                    })
                }
                Poll::Pending => None,
                Poll::Ready(Err(cause)) => Some(Err(cause)),
            };
            match result {
                Some(result) => {
                    let retired = lock(&self.waiter).take();
                    if catch_unwind(AssertUnwindSafe(|| drop(retired))).is_err() {
                        record(&self.error, ClientFailure::WaiterPanicked);
                    }
                    Poll::Ready(result)
                }
                None => Poll::Pending,
            }
        })
    }

    /// Bounded stop notification. Completion/physical disposal is joined by
    /// shutdown; this flag is only delivery of the existing Stop input.
    pub fn request_stop(&self) {
        self.stop.store(true, Ordering::Release);
        self.wake();
    }

    /// Join without requiring the caller to consume terminals first. Return the
    /// SAME intact owner for draining, including all monotonic fields. If a
    /// panic interrupted an owner-locked turn, Err retains this joined wrapper
    /// and its quarantined owner. There is no unpoison/reuse escape hatch;
    /// dropping that wrapper has the documented memory-only crash semantics.
    pub fn shutdown(mut self) -> Result<(IngressClientOwner, Option<ClientFailure>), Self> {
        self.join();
        if self.client.is_poisoned() {
            return Err(self);
        }
        let error = self.take_driver_error();
        // No clone/reconstruction of owner facts. The replacement Arc is only
        // a temporary reference until self's Drop has observed thread=None.
        let client = self.client.clone();
        drop(self);
        Ok((unwrap_client(client), error))
    }

    fn client(&self) -> Result<MutexGuard<'_, IngressClientOwner>, ClientFailure> {
        match self.client.try_lock() {
            Ok(client) => Ok(client),
            Err(std::sync::TryLockError::WouldBlock) => {
                #[cfg(test)]
                if let Some(cut) = &self.on_contention {
                    cut();
                }
                self.client.lock().map_err(|_| ClientFailure::DriverPanicked)
            }
            Err(std::sync::TryLockError::Poisoned(_)) => Err(ClientFailure::DriverPanicked),
        }
    }
    fn wake(&self) {
        match catch_unwind(AssertUnwindSafe(|| (self.notify)())) {
            Ok(Ok(())) => (),
            Ok(Err(source)) => record(&self.error, ClientFailure::BatchWait { source }),
            Err(_) => record(&self.error, ClientFailure::DriverPanicked),
        }
        if let Some(thread) = &self.thread {
            thread.thread().unpark();
        }
    }
    fn join(&mut self) {
        if let Some(thread) = self.thread.take() {
            self.request_stop();
            thread.thread().unpark();
            if thread.join().is_err() {
                record(&self.error, ClientFailure::DriverPanicked);
            }
        }
    }
}

impl Drop for BatchBackground {
    fn drop(&mut self) {
        self.join();
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}
fn unwrap_client(client: Arc<Mutex<IngressClientOwner>>) -> IngressClientOwner {
    match Arc::try_unwrap(client) {
        Ok(client) => client.into_inner().expect("only an intact joined owner can be extracted"),
        Err(_) => unreachable!("only joined thread and exclusive presentation own the client"),
    }
}
fn record(errors: &Mutex<Option<ClientFailure>>, cause: ClientFailure) {
    let mut error = lock(errors);
    if matches!(cause, ClientFailure::DriverPanicked)
        || (matches!(cause, ClientFailure::WaiterPanicked) && !matches!(*error, Some(ClientFailure::DriverPanicked)))
    {
        *error = Some(cause);
    } else {
        error.get_or_insert(cause);
    }
}
fn wake_waiter(waiter: &Mutex<Option<Waker>>, errors: &Mutex<Option<ClientFailure>>) {
    let waker = lock(waiter).take();
    if let Some(waker) = waker
        && catch_unwind(AssertUnwindSafe(|| waker.wake())).is_err()
    {
        record(errors, ClientFailure::WaiterPanicked);
    }
}
