use std::{error::Error, fmt, time::Duration};

/// Process-origin milliseconds, never a persisted/wall-clock timestamp.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(transparent)]
pub struct MonotonicMillis(u64);

impl MonotonicMillis {
    pub const fn new(milliseconds: u64) -> Self {
        Self(milliseconds)
    }
    pub const fn get(self) -> u64 {
        self.0
    }
    pub fn checked_add(self, milliseconds: u64) -> Result<Self, ClockReadFailure> {
        self.0.checked_add(milliseconds).map(Self).ok_or(ClockReadFailure::OutOfRange)
    }
}

impl TryFrom<Duration> for MonotonicMillis {
    type Error = ClockReadFailure;
    fn try_from(elapsed: Duration) -> Result<Self, Self::Error> {
        u64::try_from(elapsed.as_millis())
            .map(Self)
            .map_err(|_| ClockReadFailure::OutOfRange)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use]
pub enum ClockReadFailure {
    OutOfRange,
}

impl fmt::Display for ClockReadFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("monotonic milliseconds out of range")
    }
}
impl Error for ClockReadFailure {}
