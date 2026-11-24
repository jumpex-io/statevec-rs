use super::{
    BATCH_CLIENT_LIFETIME_DEFAULT_MS, BATCH_CLIENT_OPERATION_DEFAULT_MS, BATCH_CLIENT_RETRY_DEFAULT_MS,
    BatchRetryConfig, Digest32, InputRef, ReadSession,
};
use std::net::SocketAddr;
use std::num::NonZeroU128;

/// The deployment to which an application binds its saved batch and stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeploymentIdentity {
    cluster_identity: Digest32,
    genesis_identity: Digest32,
    execution_lineage: Digest32,
}

impl DeploymentIdentity {
    pub const fn new(cluster_identity: Digest32, genesis_identity: Digest32, execution_lineage: Digest32) -> Self {
        Self { cluster_identity, genesis_identity, execution_lineage }
    }

    pub const fn cluster_identity(self) -> Digest32 {
        self.cluster_identity
    }

    pub const fn genesis_identity(self) -> Digest32 {
        self.genesis_identity
    }

    pub const fn execution_lineage(self) -> Digest32 {
        self.execution_lineage
    }
}

/// Parameters checked when a batch session opens.
#[derive(Debug)]
pub struct BatchClientSettings {
    pub(super) identity: DeploymentIdentity,
    pub(super) session_incarnation: NonZeroU128,
    pub(super) stream: InputRef,
    pub(super) endpoints: Vec<(u64, SocketAddr)>,
    pub(super) maximum_commands: u32,
    pub(super) maximum_frame_bytes: u32,
    pub(super) maximum_outcome_bytes: u32,
    pub(super) unavailable_reply_limit: u32,
    pub(super) retry: BatchRetryConfig,
    pub(super) operation_millis: u64,
    pub(super) lifetime_millis: u64,
}

impl BatchClientSettings {
    /// Uses the shipping retry cadence, operation deadline and batch lifetime.
    /// The four limits are the command count per batch, frame size in bytes,
    /// outcome size in bytes, and number of unavailable replies per peer, in that order.
    pub fn new(
        identity: DeploymentIdentity,
        session_incarnation: NonZeroU128,
        stream: InputRef,
        endpoints: Vec<(u64, SocketAddr)>,
        maximum_commands: u32,
        maximum_frame_bytes: u32,
        maximum_outcome_bytes: u32,
        unavailable_reply_limit: u32,
    ) -> Self {
        Self {
            identity,
            session_incarnation,
            stream,
            endpoints,
            maximum_commands,
            maximum_frame_bytes,
            maximum_outcome_bytes,
            unavailable_reply_limit,
            retry: BatchRetryConfig::from(BATCH_CLIENT_RETRY_DEFAULT_MS),
            operation_millis: BATCH_CLIENT_OPERATION_DEFAULT_MS,
            lifetime_millis: BATCH_CLIENT_LIFETIME_DEFAULT_MS,
        }
    }

    /// Selects the retry, handover and routing-discovery timing for this deployment.
    pub fn with_retry(mut self, retry: impl Into<BatchRetryConfig>) -> Self {
        self.retry = retry.into();
        self
    }

    pub fn with_operation_millis(mut self, millis: u64) -> Self {
        self.operation_millis = millis;
        self
    }

    pub fn with_lifetime_millis(mut self, millis: u64) -> Self {
        self.lifetime_millis = millis;
        self
    }
}

/// Parameters checked when a direct-read session opens.
#[derive(Debug)]
pub struct ReadClientSettings {
    pub(super) identity: DeploymentIdentity,
    pub(super) session_incarnation: NonZeroU128,
    pub(super) endpoints: Vec<(u64, SocketAddr)>,
    pub(super) maximum_frame_bytes: u32,
    pub(super) session: ReadSession,
    pub(super) retry_millis: u64,
    pub(super) operation_millis: u64,
}

impl ReadClientSettings {
    pub fn new(
        identity: DeploymentIdentity,
        session_incarnation: NonZeroU128,
        endpoints: Vec<(u64, SocketAddr)>,
        maximum_frame_bytes: u32,
        session: ReadSession,
    ) -> Self {
        Self {
            identity,
            session_incarnation,
            endpoints,
            maximum_frame_bytes,
            session,
            retry_millis: BATCH_CLIENT_RETRY_DEFAULT_MS,
            operation_millis: BATCH_CLIENT_OPERATION_DEFAULT_MS,
        }
    }

    pub fn with_retry_millis(mut self, millis: u64) -> Self {
        self.retry_millis = millis;
        self
    }

    pub fn with_operation_millis(mut self, millis: u64) -> Self {
        self.operation_millis = millis;
        self
    }
}
