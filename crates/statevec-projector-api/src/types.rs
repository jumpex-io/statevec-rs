use statevec_frame::{Digest32, TxChainHash, TxSeq};
use statevec_model::{RecordKey, SchemaIdentity};

/// Read-only identity of the native complete AppliedBatch, including a validated
/// compacted base. Constructing these bytes never manufactures a live token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppliedBatchIdentity(pub [u8; 32]);

/// Actual execution progress, including the genesis chain at sequence zero.
/// These inspectable bytes do not grant publication or recovery authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppliedTxProgress {
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
/// live token token. A loaded position must be verified by runtime restore/replay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourcePosition {
    pub cluster_identity: Digest32,
    pub genesis_identity: Digest32,
    pub execution_profile_identity: Digest32,
    pub manifest: ProjectionManifest,
    pub raft_index: u64,
    pub raft_term: u64,
    pub applied_batch_identity: AppliedBatchIdentity,
    pub frontier: AppliedTxProgress,
}

/// Actual result of a member in its containing committed entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TxResult<'a> {
    pub tx_seq: u64,
    pub ordinal: u32,
    pub tx_id: Digest32,
    pub tx_chain_hash: Digest32,
    /// Preserve the producing profile alongside its exact digest bytes.
    pub result_digest_profile: u8,
    pub result_digest: &'a [u8],
    pub system_code: u16,
    pub business_code: u16,
}

/// Complete transaction output. Events retain their original ordinals; changes
/// are in record-key order. Apply the containing entry atomically before
/// coalescing current-state rows. Historical views preserve transaction/event
/// order. Rejected members have a present, empty output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TxDelta {
    pub schema_identity: SchemaIdentity,
    /// Exact replicated reference time; zero and decreasing values are valid.
    pub ref_tx_time_ns: u64,
    pub events: Vec<ProjectedEvent>,
    pub record_deltas: Vec<RecordDelta>,
}

/// Complete event, including events absent from the bounded client response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectedEvent {
    pub event_seq: u32,
    pub event_kind: u16,
    pub payload: Vec<u8>,
}

/// Complete schema data at this transaction's boundary, without allocator
/// headers. Repeated writes expose one net change; later transactions see
/// earlier successful members, not the base of the containing Raft entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordDelta {
    Insert { key: RecordKey, revision: u64, after: Vec<u8> },
    Update { key: RecordKey, revision: u64, before: Vec<u8>, after: Vec<u8> },
    Delete { key: RecordKey, revision: u64, before: Vec<u8> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum HistoryMode { Continuous, LatestState }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SupportedHistoryModes { Continuous, LatestState, Both }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum StartInstruction { DeliverEntries, Rebuild { target: u64 } }

/// Runtime disposition of a checkpoint publication. It never changes store durability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ProjectorControlRejection {
    NotAttached,
    CheckpointsDisabled,
    RestoreNotVerified,
    OwnerNotRunning,
    IdentityMismatch,
    BeyondApplied { applied: u64, projector: u64 },
    OffsetRegressed,
    OffsetConflict,
    WrongHistoryMode,
    ProjectorIdentityMismatch,
    NotRebuilding,
    RebuildNotComplete,
    RebuildMismatch,
}

/// Pull-only scheduling sample. It does not authorize checkpoint selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ProjectorStatus {
    Disabled,
    ReplayOnly,
    Recovering { projector_raft_index: u64 },
    Active { durable_raft_index: u64, engine_checkpoint_blocked_at: Option<u64> },
    Rebuilding { target_raft_index: u64 },
    LatestStateActive { baseline_raft_index: u64 },
}
