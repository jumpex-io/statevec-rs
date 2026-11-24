// OS resources only. No connect timeout, route selection or retry policy.
use polling::{Event as SocketEvent, Events, Poller};
use socket2::{Domain, Protocol, Socket, TcpKeepalive, Type};

#[derive(Debug)]
enum ClientSocketOption {
    Keepalive,
    #[cfg(target_os = "linux")]
    UserTimeout,
}

#[derive(Debug)]
struct ClientSocketConfigurationFailure {
    option: ClientSocketOption,
    source: io::Error,
}

impl std::fmt::Display for ClientSocketConfigurationFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "client socket {:?}: {}", self.option, self.source)
    }
}

impl std::error::Error for ClientSocketConfigurationFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> { Some(&self.source) }
}

fn configure_client_liveness(socket: &Socket, user_timeout: Duration) -> io::Result<()> {
    let failure = |option, source: io::Error| {
        io::Error::new(source.kind(), ClientSocketConfigurationFailure { option, source })
    };
    let keepalive = TcpKeepalive::new()
        .with_time(Duration::from_secs(5))
        .with_interval(Duration::from_secs(1))
        .with_retries(3);
    socket.set_tcp_keepalive(&keepalive)
        .map_err(|source| failure(ClientSocketOption::Keepalive, source))?;
    #[cfg(target_os = "linux")]
    socket.set_tcp_user_timeout(Some(user_timeout))
        .map_err(|source| failure(ClientSocketOption::UserTimeout, source))?;
    #[cfg(not(target_os = "linux"))]
    let _ = user_timeout;
    Ok(())
}

pub struct BatchTcpWorker {
    poller: std::sync::Arc<Poller>,
    tcp_user_timeout: Duration,
}

impl BatchTcpWorker {
    pub fn new() -> io::Result<Self> {
        Self::with_tcp_user_timeout(Duration::from_secs(8))
    }

    /// Linux physical unacknowledged/zero-window budget, independent of the
    /// owner's operation deadline. At least 5s + 3 * 1s permits three idle probes.
    /// Whole milliseconds must fit Linux's signed socket-option integer.
    /// Other platforms retain keepalive without this Linux-only timeout.
    pub fn with_tcp_user_timeout(timeout: Duration) -> io::Result<Self> {
        if timeout < Duration::from_secs(8)
            || timeout.as_millis() > i32::MAX as u128
            || timeout.subsec_nanos() % 1_000_000 != 0
        {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        Ok(Self { poller: std::sync::Arc::new(Poller::new()?), tcp_user_timeout: timeout })
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
        configure_client_liveness(&socket, self.tcp_user_timeout)?;
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
        self.wait_pair([stream.map(|socket| (socket, interests)), None], timeout).map(|[ready, _]| ready)
    }

    fn wait_pair(
        &mut self,
        sockets: [Option<(&TcpStream, (bool, bool))>; 2],
        timeout: Option<Duration>,
    ) -> io::Result<[(bool, bool); 2]> {
        let sockets = sockets.map(|socket| socket.filter(|(_, interests)| interests.0 || interests.1));
        let capacity = if sockets.iter().all(Option::is_some) { 2 } else { 1 };
        let mut events = Events::with_capacity(std::num::NonZeroUsize::new(capacity).expect("one or two sockets"));
        let mut registered = [None, None];
        let result = (|| {
            for (key, socket) in sockets.into_iter().enumerate() {
                if let Some((socket, (readable, writable))) = socket {
                    // SAFETY: the borrowed sockets outlive these scoped
                    // registrations, including the cleanup after a failed add.
                    unsafe { self.poller.add(socket, SocketEvent::new(key, readable, writable)) }?;
                    registered[key] = Some(socket);
                }
            }
            self.poller.wait(&mut events, timeout)
        })();
        let [primary, probe] = registered.map(|socket| socket.map_or(Ok(()), |socket| self.poller.delete(socket)));
        result?;
        primary?;
        probe?;
        let mut ready = [(false, false); 2];
        for event in events.iter() {
            if let Some((readable, writable)) = ready.get_mut(event.key) {
                *readable |= event.readable;
                *writable |= event.writable;
            }
        }
        Ok(ready)
    }
}
