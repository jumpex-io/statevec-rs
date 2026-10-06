//! Exact complete-batch application evidence. Construction and decoding check
//! structure only; the Raft owner's complete apply join is the sole publisher.

use crate::outcome::EntryDigest;
use crate::{ClientStreamBatchCommitment, ClientStreamBatchFailure, Digest32};

pub const APPLIED_STREAM_BATCH_BINDING_BYTES: usize = 144;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppliedStreamBatchBindingFailure {
    InvalidLength,
    Intent { source: ClientStreamBatchFailure },
    InvalidEntryBoundary,
    InvalidEngineFrontier,
}

impl std::fmt::Display for AppliedStreamBatchBindingFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "applied stream batch binding: {self:?}")
    }
}

impl std::error::Error for AppliedStreamBatchBindingFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Intent { source } => Some(source),
            _ => None,
        }
    }
}

/// One last complete batch, not a historical result cache or an apply permit.
/// Lineage, profile, enclosing applied boundary and actual engine frontier are
/// checked by the consumer; these bytes alone prove none of those joins.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppliedStreamBatchBindingV1 {
    intent: ClientStreamBatchCommitment,
    raft_index: u64,
    raft_term: u64,
    entry_digest: EntryDigest,
    engine_end_tx_seq: u64,
    engine_end_chain: Digest32,
}

impl AppliedStreamBatchBindingV1 {
    pub fn new(
        intent: ClientStreamBatchCommitment,
        raft_index: u64,
        raft_term: u64,
        entry_digest: EntryDigest,
        engine_end_tx_seq: u64,
        engine_end_chain: Digest32,
    ) -> Result<Self, AppliedStreamBatchBindingFailure> {
        if raft_index == 0 || raft_term == 0 {
            return Err(AppliedStreamBatchBindingFailure::InvalidEntryBoundary);
        }
        // Every member has a nonzero, nonterminal tx_seq; count contiguous
        // members ending here must have room for their entire starting range.
        if engine_end_tx_seq < u64::from(intent.count()) || engine_end_tx_seq == u64::MAX {
            return Err(AppliedStreamBatchBindingFailure::InvalidEngineFrontier);
        }
        Ok(Self { intent, raft_index, raft_term, entry_digest, engine_end_tx_seq, engine_end_chain })
    }

    pub const fn intent(self) -> ClientStreamBatchCommitment {
        self.intent
    }
    pub const fn raft_index(self) -> u64 {
        self.raft_index
    }
    pub const fn raft_term(self) -> u64 {
        self.raft_term
    }
    pub const fn entry_digest(self) -> EntryDigest {
        self.entry_digest
    }
    pub const fn engine_end_tx_seq(self) -> u64 {
        self.engine_end_tx_seq
    }
    pub const fn engine_end_chain(self) -> Digest32 {
        self.engine_end_chain
    }

    /// Fixed-width V1 payload. The enclosing record/wire revision selects this
    /// interpretation explicitly; an old last-command payload is never padded.
    pub fn encode_v1(self) -> [u8; APPLIED_STREAM_BATCH_BINDING_BYTES] {
        let mut bytes = [0; APPLIED_STREAM_BATCH_BINDING_BYTES];
        bytes[..56].copy_from_slice(&self.intent.encode_v1());
        bytes[56..64].copy_from_slice(&self.raft_index.to_le_bytes());
        bytes[64..72].copy_from_slice(&self.raft_term.to_le_bytes());
        bytes[72..104].copy_from_slice(&self.entry_digest.bytes());
        bytes[104..112].copy_from_slice(&self.engine_end_tx_seq.to_le_bytes());
        bytes[112..144].copy_from_slice(&self.engine_end_chain.bytes());
        bytes
    }

    /// Allocation-free structural check, including the original count bound.
    pub fn decode_v1(bytes: &[u8], maximum_commands: usize) -> Result<Self, AppliedStreamBatchBindingFailure> {
        let bytes: &[u8; APPLIED_STREAM_BATCH_BINDING_BYTES] =
            bytes.try_into().map_err(|_| AppliedStreamBatchBindingFailure::InvalidLength)?;
        let intent = ClientStreamBatchCommitment::decode_v1(&bytes[..56], maximum_commands)
            .map_err(|source| AppliedStreamBatchBindingFailure::Intent { source })?;
        Self::new(
            intent,
            u64::from_le_bytes(bytes[56..64].try_into().expect("fixed index")),
            u64::from_le_bytes(bytes[64..72].try_into().expect("fixed term")),
            EntryDigest::from_untrusted_wire(bytes[72..104].try_into().expect("fixed digest")),
            u64::from_le_bytes(bytes[104..112].try_into().expect("fixed sequence")),
            Digest32::new(bytes[112..144].try_into().expect("fixed chain")),
        )
    }
}
