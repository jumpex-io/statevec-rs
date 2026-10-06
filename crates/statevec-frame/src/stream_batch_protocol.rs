//! Whole-batch protocol values shared by the Raft owner and stateless wire codecs.
//! No value here grants commitment, admission or custody authority.
use crate::applied_binding::AppliedStreamBatchBindingV1;
use crate::outcome::{CommitOutcome, EntryDigest};
use crate::{ClientStreamBatch, ClientStreamBatchCommitment, Digest32};
use std::num::{NonZeroU64, NonZeroU128};

/// Product recovery window: client lifetimes may not exceed this duration, and
/// whole-batch services retain live results for at least this long after apply.
/// This is local delivery policy, not a replicated expiration or wire field.
pub const MAX_BATCH_LIFETIME_MILLIS: u64 = 24 * 60 * 60 * 1000;

pub const REPLY_OBSERVATION_BYTES: usize = 34;
/// Complete v14 request framing and context, before the canonical intent.
pub const REQUEST_PREFIX_BYTES: usize = 16 + 120;
/// V13 complete fixed envelope, including framing, context, intent and observation.
pub const REPLY_BASE_BYTES: usize =
    16 + 120 + crate::CLIENT_STREAM_BATCH_COMMITMENT_BYTES + REPLY_OBSERVATION_BYTES + 1;
pub const ENTRY_BINDING_BYTES: usize = 48;
/// Safe complete response reservation before executing the batch. The wire
/// omits parent entry/event coordinates from CommitOutcome; its old bound plus
/// one member tag therefore bounds every projected outcome (or unavailable).
pub fn completed_reply_byte_bound(count: u32, outcome_bound: u32) -> Option<u64> {
    u64::from(count)
        .checked_mul(u64::from(outcome_bound) + 1)
        .and_then(|members| members.checked_add((REPLY_BASE_BYTES + ENTRY_BINDING_BYTES) as u64))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidReplyObservation;
impl std::fmt::Display for InvalidReplyObservation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("invalid owner reply observation")
    }
}
impl std::error::Error for InvalidReplyObservation {}

/// Values echoed by future replies. A live session keeps its incarnation across
/// reconnects; neither these values nor a parsed request authorizes completion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestContext {
    pub cluster_identity: Digest32,
    pub genesis_identity: Digest32,
    pub execution_profile: Digest32,
    pub session_incarnation: NonZeroU128,
    pub request_id: NonZeroU64,
}

/// Both operations retain the complete original intent. Querying history is not
/// a submission, and decoding either variant reserves no owner/client capacity.
#[derive(Debug, PartialEq, Eq)]
pub enum Request {
    Submit { context: RequestContext, batch: ClientStreamBatch },
    Reconcile { context: RequestContext, batch: ClientStreamBatch },
}

impl Request {
    pub fn context(&self) -> &RequestContext {
        match self {
            Self::Submit { context, .. } | Self::Reconcile { context, .. } => context,
        }
    }

    pub fn batch(&self) -> &ClientStreamBatch {
        match self {
            Self::Submit { batch, .. } | Self::Reconcile { batch, .. } => batch,
        }
    }
}

/// A same-owner-turn observation, not a commit frontier or native permission.
/// Its history context is the reply's checked cluster/genesis/profile binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReplyObservation {
    current_term: u64,
    applied_index: u64,
    applied_term: u64,
    leader_hint: Option<NonZeroU64>,
    unavailable_reason: Option<IngressUnavailableReason>,
}

impl ReplyObservation {
    pub fn new(
        current_term: u64,
        applied_index: u64,
        applied_term: u64,
        leader_hint: Option<NonZeroU64>,
        unavailable_reason: Option<IngressUnavailableReason>,
    ) -> Result<Self, InvalidReplyObservation> {
        if (applied_index == 0) != (applied_term == 0)
            || applied_term > current_term
            || (current_term == 0 && leader_hint.is_some())
        {
            return Err(InvalidReplyObservation);
        }
        Ok(Self { current_term, applied_index, applied_term, leader_hint, unavailable_reason })
    }
    pub const fn current_term(self) -> u64 {
        self.current_term
    }
    pub const fn applied_index(self) -> u64 {
        self.applied_index
    }
    pub const fn applied_term(self) -> u64 {
        self.applied_term
    }
    pub const fn leader_hint(self) -> Option<NonZeroU64> {
        self.leader_hint
    }
    pub const fn unavailable_reason(self) -> Option<IngressUnavailableReason> {
        self.unavailable_reason
    }
}

/// Read-only reason captured at the branch which could not serve this attempt.
/// Neither a retry permit nor evidence that an earlier intent did not execute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum IngressUnavailableReason {
    NotLeader = 1,
    LeaderNotReady = 2,
    EngineOccupied = 3,
    ReadyBacklog = 4,
    SnapshotInstalling = 5,
    HistoryScanLimited = 6,
    Draining = 7,
    Failed = 8,
}

/// A decoded observation cannot join this retained request. This is separate
/// from framing failure: the client owner classifies stale versus current
/// operations without letting a decoder retire custody or choose a retry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplyRequestFailure {
    ContextMismatch,
    IntentMismatch,
    UnexpectedSubmitDisposition,
    UnexpectedReconcileDisposition,
}

impl std::fmt::Display for ReplyRequestFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "whole-batch reply/request join: {self:?}")
    }
}
impl std::error::Error for ReplyRequestFailure {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReplyBinding {
    pub context: RequestContext,
    pub intent: ClientStreamBatchCommitment,
}

impl ReplyBinding {
    pub fn for_request(request: &Request) -> Self {
        Self { context: *request.context(), intent: request.batch().commitment_v1() }
    }

    /// Identity only. Consumers use Reply::check_request for the request-kind
    /// relation, then the client owner checks currentness and its lifecycle.
    pub fn check_request(self, request: &Request) -> Result<(), ReplyRequestFailure> {
        if self.context != *request.context() {
            return Err(ReplyRequestFailure::ContextMismatch);
        }
        if self.intent != request.batch().commitment_v1() {
            return Err(ReplyRequestFailure::IntentMismatch);
        }
        Ok(())
    }
}

/// Remote declaration of one exact entry, never native commitment authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntryBinding {
    pub index: NonZeroU64,
    pub term: NonZeroU64,
    pub digest: EntryDigest,
}

/// Refuses this attempt, not a statement that an earlier uncertain attempt
/// never committed. Business rejections belong in Completed outcomes instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchSubmitRefusal {
    NotReady,
    Busy,
    TooLarge,
    InvalidInput,
    Unauthorized,
    ShuttingDown,
    IncompatibleContext,
    /// This operation failed internally; not proof of non-commitment and not
    /// advice to retry unchanged input. Business rejection is an outcome.
    InternalFailure,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemberResult {
    Outcome(CommitOutcome),
    /// Applied, but no business outcome is available. Its identity derives from
    /// its position in the complete batch; this is not success or retry advice.
    Unavailable {
        tx_seq: NonZeroU64,
    },
}

impl MemberResult {
    pub fn tx_seq(&self) -> u64 {
        match self {
            Self::Outcome(value) => value.tx_seq,
            Self::Unavailable { tx_seq } => tx_seq.get(),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum ReplyDisposition {
    /// The complete intent was admitted to one volatile proposal. This is not
    /// a follower validation ACK, durable commitment or business completion.
    IngressAdmitted(EntryBinding),
    PendingKnown(EntryBinding),
    CommittedAwaitingApply(EntryBinding),
    Unknown,
    /// Applied coverage without a fabricated entry binding or historical result.
    ProcessedResultUnavailable {
        next_sequence: u64,
    },
    /// The latest durable binding proves exact intent application, but the
    /// optional result payload is absent/expired. Its intent equals this echo.
    ExactAppliedResultUnavailable(AppliedStreamBatchBindingV1),
    RetryableNext,
    Conflict,
    /// Admission gap hint, never permission to trim or resequence the intent.
    StreamGapRejected {
        expected_seq: u64,
    },
    Refused(BatchSubmitRefusal),
    /// Live or retained results for either Submit or Reconcile: exactly one
    /// ordered result per member, all from this same entry. A partial
    /// vector, inconsistent entry or noncontiguous engine sequence is invalid.
    Completed {
        entry: EntryBinding,
        results: Vec<MemberResult>,
    },
}

#[derive(Debug, PartialEq, Eq)]
pub struct Reply {
    pub binding: ReplyBinding,
    /// Absent only for refusal without checked history (for example a foreign
    /// context). None never grants currentness or contradicts prior commitment.
    pub observation: Option<ReplyObservation>,
    pub disposition: ReplyDisposition,
}

impl Reply {
    /// Check a decoded reply against the exact retained wire operation, not
    /// merely the current batch. Submit and Reconcile have distinct request IDs.
    /// Success is only a static relation, not commitment, currentness or terminal
    /// delivery authority. Duplicate/late observations remain client-owner work.
    pub fn check_request(&self, request: &Request) -> Result<(), ReplyRequestFailure> {
        self.binding.check_request(request)?;
        self.check_kind(matches!(request, Request::Submit { .. }))
    }

    /// The same static join for an owner retaining batch custody separately
    /// from its original Submit correlation. No authority is minted here.
    pub fn check_submit(
        &self,
        context: RequestContext,
        intent: ClientStreamBatchCommitment,
    ) -> Result<(), ReplyRequestFailure> {
        self.check_retained(context, intent, true)
    }

    /// Join a current query while the client still owns its original batch.
    pub fn check_reconcile(
        &self,
        context: RequestContext,
        intent: ClientStreamBatchCommitment,
    ) -> Result<(), ReplyRequestFailure> {
        self.check_retained(context, intent, false)
    }

    fn check_retained(
        &self,
        context: RequestContext,
        intent: ClientStreamBatchCommitment,
        submit: bool,
    ) -> Result<(), ReplyRequestFailure> {
        if self.binding.context != context {
            return Err(ReplyRequestFailure::ContextMismatch);
        }
        if self.binding.intent != intent {
            return Err(ReplyRequestFailure::IntentMismatch);
        }
        self.check_kind(submit)
    }

    fn check_kind(&self, submit: bool) -> Result<(), ReplyRequestFailure> {
        let legal = match &self.disposition {
            ReplyDisposition::IngressAdmitted(_) | ReplyDisposition::StreamGapRejected { .. } => submit,
            ReplyDisposition::Unknown | ReplyDisposition::RetryableNext => !submit,
            ReplyDisposition::Completed { .. }
            | ReplyDisposition::PendingKnown(_)
            | ReplyDisposition::CommittedAwaitingApply(_)
            | ReplyDisposition::ProcessedResultUnavailable { .. }
            | ReplyDisposition::ExactAppliedResultUnavailable(_)
            | ReplyDisposition::Conflict
            | ReplyDisposition::Refused(_) => true,
        };
        if legal {
            Ok(())
        } else {
            Err(if submit {
                ReplyRequestFailure::UnexpectedSubmitDisposition
            } else {
                ReplyRequestFailure::UnexpectedReconcileDisposition
            })
        }
    }
}
