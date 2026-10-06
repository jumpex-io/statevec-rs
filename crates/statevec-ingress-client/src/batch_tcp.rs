// OS resources only. No connect timeout, route selection or retry policy.
use polling::{Event as SocketEvent, Events, Poller};
use socket2::{Domain, Protocol, Socket, Type};

pub struct BatchTcpWorker {
    poller: std::sync::Arc<Poller>,
}

impl BatchTcpWorker {
    pub fn new() -> io::Result<Self> {
        Ok(Self { poller: std::sync::Arc::new(Poller::new()?) })
    }

    /// Coalesced physical wake, including notification before wait registers.
    /// The driver observes errors; the bounded OS wait also prevents a failed
    /// notification from permanently stranding stop or newly admitted work.
    pub fn notifier(&self) -> std::sync::Arc<dyn Fn() -> io::Result<()> + Send + Sync> {
        let poller = self.poller.clone();
        std::sync::Arc::new(move || poller.notify())
    }
}

impl BatchConnectIo for BatchTcpWorker {
    type Stream = TcpStream;

    fn monotonic_now(&self) -> std::time::Instant {
        std::time::Instant::now()
    }

    fn pause_after_wait_error(&mut self, remaining: Duration) {
        std::thread::sleep(remaining);
    }

    fn connect(&mut self, address: SocketAddr) -> io::Result<TcpStream> {
        let socket = Socket::new(Domain::for_address(address), Type::STREAM, Some(Protocol::TCP))?;
        socket.set_nonblocking(true)?;
        socket.set_tcp_nodelay(true)?;
        match socket.connect(&address.into()) {
            Ok(()) => (),
            Err(source) if source.kind() == io::ErrorKind::WouldBlock => (),
            #[cfg(unix)]
            Err(source) if source.raw_os_error() == Some(libc::EINPROGRESS) => (),
            Err(source) => return Err(source),
        }
        Ok(socket.into())
    }

    fn connected(&mut self, stream: &mut TcpStream) -> io::Result<Poll<()>> {
        if let Some(source) = stream.take_error()? {
            return Err(source);
        }
        match stream.peer_addr() {
            Ok(_) => Ok(Poll::Ready(())),
            Err(source) if matches!(source.kind(), io::ErrorKind::NotConnected | io::ErrorKind::WouldBlock) => {
                Ok(Poll::Pending)
            }
            Err(source) => Err(source),
        }
    }

    fn wait(
        &mut self,
        stream: Option<&TcpStream>,
        interests: (bool, bool),
        timeout: Option<Duration>,
    ) -> io::Result<(bool, bool)> {
        let mut events = Events::with_capacity(std::num::NonZeroUsize::new(1).expect("one socket"));
        let stream = stream.filter(|_| interests.0 || interests.1);
        if let Some(stream) = stream {
            // SAFETY: this borrowed stream cannot close or move while wait is
            // running. Delete below on both success and error before returning.
            unsafe { self.poller.add(stream, SocketEvent::new(0, interests.0, interests.1)) }?;
        }
        let result = self.poller.wait(&mut events, timeout);
        let deleted = stream.map_or(Ok(()), |stream| self.poller.delete(stream));
        result?;
        deleted?;
        let mut ready = (false, false);
        for event in events.iter() {
            ready.0 |= event.readable;
            ready.1 |= event.writable;
        }
        Ok(ready)
    }
}
