use statevec_frame::{Digest32, TxChainHash, TxSeq, outcome::EntryDigest};
use statevec_model::{RecordKey, SchemaIdentity};
use std::num::NonZeroU64;

/// Actual execution progress, including the genesis chain at sequence zero.
/// These inspectable bytes do not grant publication or recovery authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProjectionFrontier {
    pub applied_tx_seq: TxSeq,
    pub tx_chain_hash: TxChainHash,
}

/// Coverage of the source output. All registered record kinds and events are included.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectionManifest {
    AllRegisteredSchemaV1,
}

/// Diagnostic origin of delivery. Consumers must handle replay idempotently;
/// this flag does not determine whether their own state already covers an entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectionContext {
    Live,
    Replay,
}

/// Read-only source identity and position. This is persistable metadata, not a
/// live cut token. A loaded position must be verified by runtime restore/replay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProjectionPosition {
    pub cluster_identity: Digest32,
    pub genesis_identity: Digest32,
    pub execution_profile_identity: Digest32,
    pub manifest: ProjectionManifest,
    pub raft_index: u64,
    pub raft_term: u64,
    pub entry_digest: EntryDigest,
    pub frontier: ProjectionFrontier,
}

/// Actual result of a member in its containing committed frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransactionResult<'a> {
    pub tx_seq: u64,
    pub ordinal: u32,
    pub tx_id: Digest32,
    pub tx_chain_hash: Digest32,
    /// Preserve the producing profile alongside its exact digest bytes.
    pub result_digest_profile: u8,
    pub result_digest: &'a [u8],
    pub sys_result_code: u16,
    pub biz_code: u16,
}

/// Complete transaction output. Events retain their original ordinals; changes
/// are in record-key order. Apply the containing frame atomically before
/// coalescing current-state rows. Historical views preserve transaction/event
/// order. Rejected members have a present, empty output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransactionProjection {
    pub schema_identity: SchemaIdentity,
    /// Exact replicated reference time; zero and decreasing values are valid.
    pub ref_tx_time_ns: u64,
    pub events: Vec<ProjectedEvent>,
    pub record_changes: Vec<ProjectedRecordChange>,
}

/// Complete event, including events absent from the bounded client response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectedEvent {
    pub event_seq: u32,
    pub event_kind: u16,
    pub payload: Vec<u8>,
}

/// Complete schema data after-image or explicit tombstone, without allocator
/// headers. Insert and update remain distinct so missing prior rows are visible.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectedRecordChange {
    Insert { key: RecordKey, revision: u64, after: Vec<u8> },
    Update { key: RecordKey, revision: u64, after: Vec<u8> },
    Delete { key: RecordKey, revision: u64 },
}

/// Untrusted metadata decoded from a projector's durable selection. Runtime
/// restore/replay must join this exact cut before it can cover an engine checkpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProjectorCheckpointRestoreObservation {
    pub generation: NonZeroU64,
    pub manifest_digest: Digest32,
    pub cluster_identity: Digest32,
    pub genesis_identity: Digest32,
    pub execution_profile_identity: Digest32,
    pub manifest: ProjectionManifest,
    pub raft_index: u64,
    pub raft_term: u64,
    pub entry_digest: EntryDigest,
    pub frontier: ProjectionFrontier,
}

/// Requested recovery mode; the runtime checks it against its actual startup base.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectorRecovery {
    /// Replay retained history with automatic engine checkpoints disabled.
    ReplayOnly,
    /// Empty application state; the runtime requires a genesis engine base.
    Fresh,
    Restore(ProjectorCheckpointRestoreObservation),
}

/// Runtime disposition of a checkpoint publication. It never changes store durability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectorCheckpointRejection {
    NotAttached,
    CheckpointsDisabled,
    RestoreNotVerified,
    OwnerNotRunning,
    IdentityMismatch,
    BeyondApplied { applied: u64, projector: u64 },
    GenerationRegressed,
    GenerationConflict,
    CutRegressed,
}

/// Pull-only scheduling sample. It does not authorize checkpoint selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectorCheckpointStatus {
    Disabled,
    ReplayOnly,
    Recovering { projector_raft_index: u64 },
    Active { durable_raft_index: u64, waiting_for_projector: Option<u64> },
}
