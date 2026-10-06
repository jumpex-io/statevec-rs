// Mechanical byte and socket I/O. No admission, result, endpoint selection or retry policy.
use super::ClientFailure;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

include!("batch_io.rs");
include!("batch_driver.rs");
include!("batch_tcp.rs");

/// Default OS source for bounded clients and their drivers. Virtual execution
/// injects the source at construction; the owner samples it for every timed event.
pub fn batch_monotonic_now() -> Result<super::MonotonicMillis, super::ClockReadFailure> {
    static ORIGIN: OnceLock<Instant> = OnceLock::new();
    super::MonotonicMillis::try_from(ORIGIN.get_or_init(Instant::now).elapsed())
}
