//! Plugin implementation contracts: configure, prepare, then one worker run.
use super::{ConfigureFailure, DeliveryLimits, ProjectorFailure, ProjectorIo};
use crate::{Digest32, ProjectionFrontier, ProjectionManifest, ProjectorRecoveryMode, ProjectorRecoverySupport};
use std::{num::NonZeroU64, path::Path};

/// Reconstruction request, distinct from capabilities and the host's startup instruction.
/// Native-only ReplayOnly cannot be constructed here:
/// ```compile_fail,E0599
/// use statevec_projector_api::plugin_v2::ProjectorRecovery;
/// let recovery = ProjectorRecovery::ReplayOnly;
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ProjectorRecovery {
    ContinuousHistory { offset: Option<crate::ConsumeOffset> },
    LatestStateOnly,
}

pub struct ProjectorConfig<'a> {
    pub bytes: &'a [u8],
    pub base_dir: &'a Path,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProjectionIdentity {
    pub cluster_identity: Digest32,
    pub genesis_identity: Digest32,
    pub execution_profile_identity: Digest32,
    pub manifest: ProjectionManifest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EngineCheckpointPolicy {
    pub interval_indices: NonZeroU64,
    /// Lower bound between adopting a selected engine checkpoint and capturing
    /// its successor. This is not the application's publication interval.
    pub min_capture_residence_ms: NonZeroU64,
}

/// The host has already admitted a learner with External retention. These
/// eligibility facts are validated at the wire boundary, not writable flags.
pub struct ProjectorPrepare<'a> {
    pub identity: ProjectionIdentity,
    pub peer_id: u64,
    pub cluster_id: &'a str,
    pub projector_id: &'a str,
    pub schema_idl: &'a [u8],
    pub checkpoint_policy: Option<EngineCheckpointPolicy>,
    pub recovery_mode: ProjectorRecoveryMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProjectorStartupBase {
    pub identity: ProjectionIdentity,
    pub peer_id: u64,
    pub raft_index: u64,
    pub raft_term: u64,
    pub frontier: ProjectionFrontier,
}

pub trait ProjectorModule: Send + Sync + 'static {
    const RECOVERY_SUPPORT: ProjectorRecoverySupport;
    type Configured: ConfiguredProjector;
    /// Parse immutable configuration only; do not open stores/start services.
    fn configure(&self, config: ProjectorConfig<'_>) -> Result<Self::Configured, ConfigureFailure>;
}
pub trait ConfiguredProjector: Send + Sync + 'static {
    type Prepared: PreparedProjector;
    /// Open/verify the model but do not start service threads. Borrowed metadata
    /// ends with this call; retain owned copies only where the model needs them.
    fn prepare(&self, context: ProjectorPrepare<'_>) -> Result<Self::Prepared, ProjectorFailure>;
}
pub trait PreparedProjector: Send + 'static {
    fn recovery(&self) -> ProjectorRecovery;
    fn delivery_limits(&self) -> DeliveryLimits;
    /// The SDK owns source custody and joins every scoped service on all exits.
    /// Runs on the SDK worker, not the engine thread. Borrowing services must
    /// use `io.split().services`; the SDK stops acquisition, joins them, then releases
    /// remaining loans/replies. Application-owned threads must also be joined
    /// before returning. Returning does not by itself prove host retirement.
    fn run(self, io: ProjectorIo<'_>) -> Result<(), ProjectorFailure>;
}
