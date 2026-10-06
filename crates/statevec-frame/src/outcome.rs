// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

//! Transport-independent leader ingress values.
//!
//! This module defines shared leader ingress observations. It does not depend on
//! raft-rs, concrete transports, runtime engines, or domain command types.

use crate::{TxChainHash, TxId, TxSeq};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Typed digest of one exact encoded Raft command entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EntryDigest([u8; 32]);

impl EntryDigest {
    /// Computes the digest from the exact entry bytes accepted by raft-rs.
    pub fn from_encoded_entry(encoded_entry: &[u8]) -> Self {
        Self(Sha256::digest(encoded_entry).into())
    }

    /// Rebinds bytes decoded from an untrusted wire frame.
    ///
    /// This does not grant proposal authority; the receiving boundary must
    /// compare it with locally owned entry evidence before acting on it.
    pub const fn from_untrusted_wire(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Returns the canonical digest bytes for transport encoding.
    pub const fn bytes(self) -> [u8; 32] {
        self.0
    }
}

/// Evidence returned to synchronous RPC commit-ack waiters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitOutcome {
    /// Node that applied the transaction and published the outcome.
    pub applied_by: u64,
    /// Canonical transaction sequence.
    pub tx_seq: TxSeq,
    /// Canonical transaction id.
    pub tx_id: TxId,
    /// Chain hash after this transaction.
    pub tx_chain_hash: TxChainHash,
    /// Raft entry index that contains this transaction.
    pub raft_index: u64,
    /// Raft term observed for the applied raft entry range.
    pub raft_term: u64,
    /// Runtime/system status code.
    pub sys_status_code: u16,
    /// Domain/plugin status code.
    pub biz_status_code: u16,
    /// Canonical total event count emitted by this transaction.
    pub emitted_event_count: u32,
    /// Capped inline response event projection.
    pub inline_events: Vec<InlineResponseEvent>,
    /// Whether inline-eligible events were removed by response caps.
    pub inline_events_truncated: bool,
}

impl CommitOutcome {
    /// Current wire shape, excluding the outer TCP response frame.
    pub const FIXED_ENCODED_BYTES: usize = 4 * 8 + 2 * 32 + 2 * 2 + 2 * 4 + 1;
    pub const INLINE_EVENT_HEADER_BYTES: usize = 8 + 4 + 2 + 4;

    pub fn encoded_len(&self) -> usize {
        Self::FIXED_ENCODED_BYTES
            + self
                .inline_events
                .iter()
                .map(|event| Self::INLINE_EVENT_HEADER_BYTES + event.payload.len())
                .sum::<usize>()
    }

    /// Returns the durable commit evidence subset.
    pub fn commit_evidence(&self) -> CommitEvidence {
        CommitEvidence {
            tx_seq: self.tx_seq,
            tx_chain_hash: self.tx_chain_hash,
            raft_index: self.raft_index,
            raft_term: self.raft_term,
            applied_by: self.applied_by,
        }
    }
}

/// Inline event projection in a commit-ack response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InlineResponseEvent {
    /// Canonical transaction sequence.
    pub tx_seq: TxSeq,
    /// Event sequence in the canonical transaction event stream.
    pub event_seq: u32,
    /// Schema event kind.
    pub event_kind: u16,
    /// Encoded event payload bytes.
    pub payload: Vec<u8>,
}

/// Evidence that a submitted request has been committed and applied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitEvidence {
    /// Canonical transaction sequence.
    pub tx_seq: TxSeq,
    /// Chain hash after this transaction.
    pub tx_chain_hash: TxChainHash,
    /// Raft entry index that contains this transaction.
    pub raft_index: u64,
    /// Raft term observed for the applied raft entry range.
    pub raft_term: u64,
    /// Node that applied the transaction and published the evidence.
    pub applied_by: u64,
}

#[cfg(test)]
#[path = "ut_raft_ingress.rs"]
mod ut_raft_ingress;
