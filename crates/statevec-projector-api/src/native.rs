//! Native runtime ports, separate from the configured-plugin contracts.
//! Implementations retain source provenance and admission authority.
use crate::{
    ConsumeOffset, ProjectionContext, ProjectionPosition, ProjectorCheckpointRejection,
    ProjectorCheckpointStatus, TransactionProjection,
    TransactionResult,
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

/// Immutable evidence supplied by a committed source. Implementations determine
/// provenance; the production implementation cannot be constructed from metadata.
/// Copying a cut does not duplicate frame custody or acknowledge a checkpoint.
pub trait CheckpointCut: Copy {
    type Publication: CheckpointPublication<Cut = Self>;

    fn position(&self) -> ProjectionPosition;

    /// A continuous source supplies its bound namespace. Native ReplayOnly has
    /// no durable consumer offset and returns None.
    fn offset(&self) -> Option<ConsumeOffset>;

    /// Call only after business effects and the complete offset are durable.
    /// This is the application's assertion, not host file I/O or an automatic ACK.
    fn checkpointed(self) -> Self::Publication;
}

/// Store publication bound to its source's concrete cut token.
pub trait CheckpointPublication: Copy {
    type Cut: CheckpointCut<Publication = Self>;

    fn cut(&self) -> Self::Cut;
    fn offset(&self) -> Option<ConsumeOffset>;
}

/// Read-only member output. Raw engine output alone is not a committed frame.
pub trait CommittedTransaction {
    fn result(&self) -> TransactionResult<'_>;
    fn output(&self) -> &TransactionProjection;
}

/// One complete committed entry, including empty membership/no-op barriers.
/// The production frame is move-only. Consumers cannot skip part of a frame
/// or reconstruct its cut token from the readable position.
pub trait CommittedFrame {
    type Cut: CheckpointCut;
    type Transaction: CommittedTransaction;

    fn checkpoint_cut(&self) -> Self::Cut;
    fn peer_id(&self) -> u64;
    fn context(&self) -> ProjectionContext;
    fn transactions(&self) -> &[Self::Transaction];
}

/// Bounded delivery. Dequeue/disconnection notify the runtime; they do not
/// acknowledge durable consumption. Empty and disconnected remain distinct.
pub trait ProjectionReceiver {
    type Frame: CommittedFrame;

    fn try_recv(&self) -> Result<Box<Self::Frame>, mpsc::TryRecvError>;
    fn recv_timeout(&self, timeout: Duration) -> Result<Box<Self::Frame>, mpsc::RecvTimeoutError>;
}

/// Nonblocking checkpoint offers to the existing runtime. The associated
/// failure preserves the host's exact typed cause and unqueued request custody;
/// these methods neither retry nor choose a shutdown policy.
pub trait ProjectorControl {
    type Cut: CheckpointCut;
    type RequestFailure<T>;

    fn enqueue_projector_checkpoint(
        &self,
        publication: <Self::Cut as CheckpointCut>::Publication,
    ) -> Result<
        mpsc::Receiver<Result<(), ProjectorCheckpointRejection>>,
        Self::RequestFailure<<Self::Cut as CheckpointCut>::Publication>,
    >;

    fn enqueue_projector_checkpoint_status(
        &self,
    ) -> Result<mpsc::Receiver<ProjectorCheckpointStatus>, Self::RequestFailure<()>>;
}
