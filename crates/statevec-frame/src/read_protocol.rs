//! Business read values shared by the node, its read owners and the read wire.
//! A read is a volatile lookup at one applied position. No value here grants
//! commitment, admission, linearizability or durable custody.

/// Request and answer bounds of one read origin. Each origin defines its value
/// once, beside the boundary that publishes that origin's entry point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadLimits {
    /// Keys in one request. A request holds at least one key.
    pub max_keys: usize,
    /// Encoded unique-key bytes of one key.
    pub max_key_bytes: usize,
    /// Total record bytes of one answer.
    pub max_record_bytes: usize,
}

/// Direct client reads over ingress. Measured before the release freeze.
pub const CLIENT_READ_LIMITS: ReadLimits = ReadLimits { max_keys: 256, max_key_bytes: 1024, max_record_bytes: 1 << 20 };

/// Direct client reads on one node, from offer to answer. Each holds one credit
/// in its reply route until the route publishes or is dropped.
pub const CLIENT_READ_CREDITS: usize = 16;

/// One requested record. A `SysId` key finds a record only when the live
/// record has that kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadKey {
    UniqueKey { kind: u16, uk_id: u8, key: Vec<u8> },
    SysId { kind: u16, sys_id: u64 },
}

/// The serving node's role when its owner answered. A learner is a non-voter
/// in the node's current membership, whatever its Raft state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadRole {
    Leader,
    Follower,
    /// Campaigning, including a pre-vote.
    Candidate,
    Learner,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadObservationError {
    AppliedTermAboveCurrent,
    AppliedAboveCommitted,
    /// Applied index and applied term are zero together, or neither is.
    AppliedEntry,
    /// A leader hint needs a nonzero current term.
    LeaderHint,
}

/// The serving node's Raft view when its owner answered or stamped a read.
/// An answer's applied entry is its position P. A projection for the
/// requester, never authority for another request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadObservation {
    current_term: u64,
    applied_index: u64,
    applied_term: u64,
    committed_index: u64,
    leader_hint: Option<std::num::NonZeroU64>,
    role: ReadRole,
}

impl ReadObservation {
    pub fn new(
        current_term: u64,
        applied_index: u64,
        applied_term: u64,
        committed_index: u64,
        leader_hint: Option<std::num::NonZeroU64>,
        role: ReadRole,
    ) -> Result<Self, ReadObservationError> {
        if applied_term > current_term {
            return Err(ReadObservationError::AppliedTermAboveCurrent);
        }
        if applied_index > committed_index {
            return Err(ReadObservationError::AppliedAboveCommitted);
        }
        if (applied_index == 0) != (applied_term == 0) {
            return Err(ReadObservationError::AppliedEntry);
        }
        if leader_hint.is_some() && current_term == 0 {
            return Err(ReadObservationError::LeaderHint);
        }
        Ok(Self { current_term, applied_index, applied_term, committed_index, leader_hint, role })
    }

    pub fn current_term(&self) -> u64 {
        self.current_term
    }

    pub fn applied_index(&self) -> u64 {
        self.applied_index
    }

    pub fn applied_term(&self) -> u64 {
        self.applied_term
    }

    pub fn committed_index(&self) -> u64 {
        self.committed_index
    }

    pub fn leader_hint(&self) -> Option<std::num::NonZeroU64> {
        self.leader_hint
    }

    pub fn role(&self) -> ReadRole {
        self.role
    }
}

/// Why a read was not answered. A refusal is the read's complete outcome: no
/// record of it is returned, and the request may be sent again unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ReadRefusal {
    /// The request's cluster, genesis or execution lineage is not this
    /// node's. Answered without an observation.
    IncompatibleContext,
    /// The serving owner has stopped after a failure.
    Fatal,
    /// The serving owner is shutting down.
    Draining,
    /// This node does not serve client reads.
    ReadsDisabled,
    /// This node is not a voter in its current membership.
    NotVoter,
    /// This node is the leader and does not serve leader reads.
    LeaderReadsDisabled,
    /// Initial replay has not reached the durable commit recorded at open.
    RecoveryPending,
    /// A snapshot installation is in progress.
    SnapshotInstalling,
    /// The local Projector's latest-state rebuild barrier holds.
    ProjectorRebuilding,
    /// No Projector that serves engine queries is attached.
    Unsupported,
    /// The request exceeds its origin's bounds.
    RequestTooLarge,
    /// The origin's reads in flight are at their bound.
    Busy,
    /// The serving engine has applied less than the request's minimum.
    NotYetVisible { applied_tx_seq: u64 },
    /// A key names a record kind this engine's schema does not define.
    UnknownKind,
    /// A key names a unique key its record kind does not define.
    UnknownUniqueKey,
    /// The answer would exceed its origin's record-byte bound.
    ReplyTooLarge,
    /// No live engine can serve the read now.
    Unavailable,
    /// A refusal code this SDK does not recognize, with its opaque argument.
    Unrecognized { observed: u8, argument: u64 },
}
