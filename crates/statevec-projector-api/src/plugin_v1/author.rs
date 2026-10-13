//! Safe configured authoring. Source observations do not mint publication cuts.
use super::{ConfigureFailure, DeliveryLimits, ProjectorFailure, ProjectorIo};
use crate::{Digest32, ProjectionFrontier, ProjectionManifest, ProjectorRecovery, ProjectorRecoveryMode, ProjectorRecoverySupport};
use std::{num::NonZeroU64, path::Path};

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
pub struct ProjectorCheckpointPolicy {
    pub interval_indices: NonZeroU64,
    pub min_selected_duration_ms: NonZeroU64,
}

pub struct ProjectorPrepare<'a> {
    pub identity: ProjectionIdentity,
    pub peer_id: u64,
    pub cluster_id: &'a str,
    pub projector_id: &'a str,
    pub schema_idl: &'a [u8],
    pub learner_only: bool,
    pub external_retention: bool,
    pub checkpoint_policy: Option<ProjectorCheckpointPolicy>,
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
    fn run(self, io: ProjectorIo<'_>) -> Result<(), ProjectorFailure>;
}
