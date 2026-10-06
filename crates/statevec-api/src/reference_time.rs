//! Missing capability is distinct from the valid Unix epoch value zero.

/// The host has not supplied a replicated transaction reference time.
///
/// Native execution must provide it. Legacy plugin hosts and unconfigured test
/// hosts report this error; callers must not substitute command or local time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReferenceTimeUnavailable;

impl std::fmt::Display for ReferenceTimeUnavailable {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("host does not provide replicated transaction reference time")
    }
}

impl std::error::Error for ReferenceTimeUnavailable {}
