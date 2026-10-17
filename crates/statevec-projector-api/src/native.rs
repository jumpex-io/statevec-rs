//! Native runtime ports, separate from the configured-plugin contracts.
//! Implementations retain source identity and admission authority.
use crate::{
    ConsumeOffset, ProjectionContext, SourcePosition, ProjectorControlRejection,
    ProjectorStatus, TxDelta,
    TxResult,
};
use std::{sync::mpsc, time::Duration};

/// Native attachment request; ReplayOnly is not a configured plugin mode.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum AttachmentRecovery {
    ReplayOnly,
    Continuous { offset: Option<ConsumeOffset> },
    LatestState,
}

/// Immutable token supplied by an applied source. The production implementation
/// cannot be constructed from metadata. Copying a token does not duplicate entry
/// ownership or acknowledge durability.
pub trait NativeEntryToken: Copy {
    type Report: NativeDurableOffsetReport<Token = Self>;

    fn position(&self) -> SourcePosition;

    /// A continuous source supplies its bound namespace. Native ReplayOnly has
    /// no durable consumer offset and returns None.
    fn offset(&self) -> Option<ConsumeOffset>;

    /// Call only after business effects and the complete offset are durable.
    /// This is the application's assertion, not host file I/O or an automatic ACK.
    /// Unlike the plugin token, this native declaration does not check the mode;
    /// the owner decides whether the report is admissible.
    fn declare_durable(self) -> Self::Report;
}

/// Durability declaration bound to its source's concrete entry token.
pub trait NativeDurableOffsetReport: Copy {
    type Token: NativeEntryToken<Report = Self>;

    fn token(&self) -> Self::Token;
    fn offset(&self) -> Option<ConsumeOffset>;
}

/// Read-only Tx output. Raw engine output alone is not an applied Raft entry.
pub trait AppliedTx {
    fn result(&self) -> TxResult<'_>;
    fn output(&self) -> &TxDelta;
}

/// One complete committed entry, including empty membership/no-op barriers.
/// The production entry is move-only. Consumers cannot skip part of an entry
/// or reconstruct its token from the readable position.
pub trait AppliedRaftEntry {
    type Token: NativeEntryToken;
    type Tx: AppliedTx;

    fn token(&self) -> Self::Token;
    fn peer_id(&self) -> u64;
    fn context(&self) -> ProjectionContext;
    fn txs(&self) -> &[Self::Tx];
}

/// Bounded delivery. Dequeue/disconnection notify the runtime; they do not
/// acknowledge durable consumption. Empty and disconnected remain distinct.
pub trait ProjectionReceiver {
    type Entry: AppliedRaftEntry;

    fn try_recv(&self) -> Result<Box<Self::Entry>, mpsc::TryRecvError>;
    fn recv_timeout(&self, timeout: Duration) -> Result<Box<Self::Entry>, mpsc::RecvTimeoutError>;
}

/// Nonblocking control requests to the existing runtime. The associated
/// error preserves the host's exact typed cause and unqueued request ownership;
/// these methods neither retry nor choose a shutdown policy.
pub trait ProjectorControl {
    type Token: NativeEntryToken;
    type RequestError<T>;

    fn enqueue_durable_offset(
        &self,
        publication: <Self::Token as NativeEntryToken>::Report,
    ) -> Result<
        mpsc::Receiver<Result<(), ProjectorControlRejection>>,
        Self::RequestError<<Self::Token as NativeEntryToken>::Report>,
    >;

    fn enqueue_status(
        &self,
    ) -> Result<mpsc::Receiver<ProjectorStatus>, Self::RequestError<()>>;
}
