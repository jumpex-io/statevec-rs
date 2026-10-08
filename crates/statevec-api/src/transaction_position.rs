//! The replicated position of the executing transaction.

/// The host has not supplied the executing transaction's replicated position.
///
/// Native execution assigns every transaction, including deterministic
/// refusals, a unique position that replay reproduces. Legacy plugin hosts and
/// direct test-host calls outside a transaction report this error; callers must
/// not substitute a command or local counter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TxPositionUnavailable;

impl std::fmt::Display for TxPositionUnavailable {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("host does not provide the replicated transaction position")
    }
}

impl std::error::Error for TxPositionUnavailable {}
