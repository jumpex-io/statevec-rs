//! Plugin implementation contracts: configure, prepare, then one worker run.
use super::{ConfigureError, DeliveryLimits, ProjectorError, ProjectorIo};
use crate::{Digest32, AppliedTxProgress, ProjectionManifest, HistoryMode, SupportedHistoryModes};
use std::num::NonZeroU64;
use std::path::Path;

/// Reconstruction request, distinct from capabilities and the host's startup instruction.
/// Native-only ReplayOnly cannot be constructed here:
/// ```compile_fail,E0599
/// use statevec_projector_api::plugin::StartRequest;
/// let recovery = StartRequest::ReplayOnly;
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum StartRequest {
    Continuous { offset: Option<crate::ConsumeOffset> },
    LatestState,
}

#[non_exhaustive]
pub struct ProjectorConfig<'a> {
    pub bytes: &'a [u8],
    pub base_dir: &'a Path,
}

impl<'a> ProjectorConfig<'a> {
    /// Construct the configuration input for a direct call to [ProjectorModule::configure].
    pub fn new(bytes: &'a [u8], base_dir: &'a Path) -> Self {
        Self { bytes, base_dir }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceIdentity {
    pub cluster_identity: Digest32,
    pub genesis_identity: Digest32,
    pub execution_lineage_identity: Digest32,
    pub manifest: ProjectionManifest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct EngineCheckpointPolicy {
    pub interval_indices: NonZeroU64,
    /// Lower bound between adopting a selected engine checkpoint and capturing
    /// its successor. This is not the application's publication interval.
    pub min_capture_residence_ms: NonZeroU64,
}

impl EngineCheckpointPolicy {
    pub const fn new(interval_indices: NonZeroU64, min_capture_residence_ms: NonZeroU64) -> Self {
        Self { interval_indices, min_capture_residence_ms }
    }
}

/// The host has already admitted a learner with External retention. These
/// eligibility facts are validated at the wire boundary, not writable flags.
#[non_exhaustive]
pub struct ProjectorPrepare<'a> {
    pub identity: SourceIdentity,
    pub peer_id: u64,
    pub cluster_id: &'a str,
    pub projector_id: &'a str,
    pub schema_idl: &'a [u8],
    pub checkpoint_policy: Option<EngineCheckpointPolicy>,
    pub history_mode: HistoryMode,
}

impl<'a> ProjectorPrepare<'a> {
    /// Construct the borrowed input for a direct call to [ConfiguredProjector::prepare].
    /// The host validates attachment and wire input separately.
    pub fn new(
        identity: SourceIdentity,
        peer_id: u64,
        cluster_id: &'a str,
        projector_id: &'a str,
        schema_idl: &'a [u8],
        checkpoint_policy: Option<EngineCheckpointPolicy>,
        history_mode: HistoryMode,
    ) -> Self {
        Self { identity, peer_id, cluster_id, projector_id, schema_idl, checkpoint_policy, history_mode }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StartupPosition {
    pub identity: SourceIdentity,
    pub peer_id: u64,
    pub raft_index: u64,
    pub raft_term: u64,
    pub frontier: AppliedTxProgress,
}

/// Opt-in decoder contract, independent of continuous/latest history support.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaCompatibility {
    Exact,
    /// Preserve known fields and handle unknown kinds explicitly. This does
    /// not promise automatic migration of the application's own model store.
    Additive,
}

pub trait ProjectorModule: Send + Sync + 'static {
    const SUPPORTED_HISTORY_MODES: SupportedHistoryModes;
    const SCHEMA_COMPATIBILITY: SchemaCompatibility = SchemaCompatibility::Exact;
    /// The module's own decoder schema, never a copy of the host's active Core
    /// schema. Generated registry IDL may be retained in a module OnceLock.
    /// The host copies and validates this bounded immutable span at discovery.
    fn decoder_idl() -> &'static [u8];
    type Configured: ConfiguredProjector;
    /// Parse immutable configuration only; do not open stores/start services.
    fn configure(&self, config: ProjectorConfig<'_>) -> Result<Self::Configured, ConfigureError>;
}
pub trait ConfiguredProjector: Send + Sync + 'static {
    type Prepared: PreparedProjector;
    /// Open/verify the model but do not start service threads. Borrowed metadata
    /// ends with this call; retain owned copies only where the model needs them.
    fn prepare(&self, context: ProjectorPrepare<'_>) -> Result<Self::Prepared, ProjectorError>;
}
pub trait PreparedProjector: Send + 'static {
    fn start_request(&self) -> StartRequest;
    fn delivery_limits(&self) -> DeliveryLimits;
    /// The SDK owns the source and joins every scoped service on all exits.
    /// Runs on the SDK worker, not the engine thread. Borrowing services must
    /// use `io.split().services`; the SDK stops acquisition, joins them, then releases
    /// remaining borrows/replies. Application-owned threads must also be joined
    /// before returning. Returning does not by itself prove host retirement.
    fn run(self, io: ProjectorIo<'_>) -> Result<(), ProjectorError>;
}
