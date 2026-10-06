//! Whole-range observations only. Decoding or correlating a reply does not mint
//! a core commit capability, settle client custody or authorize another submit.

use super::{FrameFailure, HEADER_BYTES, VERSION, append_context, check_bytes, decode_header, take, take_context};
use statevec_frame::applied_binding::AppliedStreamBatchBindingV1;
use statevec_frame::outcome::EntryDigest;
use statevec_frame::{CLIENT_STREAM_BATCH_COMMITMENT_BYTES, ClientStreamBatchCommitment};
use std::num::NonZeroU64;

#[path = "stream_batch_outcome.rs"]
mod outcome;

const REPLY: u16 = 3;
use statevec_frame::stream_batch_protocol::{
    ENTRY_BINDING_BYTES as ENTRY_BYTES, REPLY_BASE_BYTES as BASE_BYTES, REPLY_OBSERVATION_BYTES as OBSERVATION_BYTES,
};

pub use statevec_frame::stream_batch_protocol::{
    BatchSubmitRefusal, EntryBinding, IngressUnavailableReason, MemberResult, Reply, ReplyBinding, ReplyDisposition,
    ReplyObservation, ReplyRequestFailure,
};

pub fn encode_reply(
    reply: &Reply,
    maximum_frame_bytes: usize,
    maximum_commands: usize,
) -> Result<Vec<u8>, FrameFailure> {
    check_intent(reply.binding.intent, maximum_commands)?;
    if reply.observation.is_none() && !matches!(reply.disposition, ReplyDisposition::Refused(_)) {
        return Err(FrameFailure::MissingReplyObservation);
    }
    check_unavailability(
        reply.observation,
        matches!(reply.disposition, ReplyDisposition::Unknown | ReplyDisposition::Refused(_)),
    )?;
    let body_extra = match &reply.disposition {
        ReplyDisposition::IngressAdmitted(entry)
        | ReplyDisposition::PendingKnown(entry)
        | ReplyDisposition::CommittedAwaitingApply(entry) => {
            check_entry_observation(reply.observation, *entry, false)?;
            ENTRY_BYTES
        }
        ReplyDisposition::Unknown | ReplyDisposition::Conflict => 0,
        ReplyDisposition::RetryableNext => {
            check_retry_observation(reply.observation)?;
            0
        }
        ReplyDisposition::ProcessedResultUnavailable { next_sequence } => {
            check_applied_observation(reply.observation)?;
            check_frontier(reply.binding, *next_sequence)?;
            8
        }
        ReplyDisposition::StreamGapRejected { expected_seq } => {
            check_gap(reply.binding, *expected_seq)?;
            8
        }
        ReplyDisposition::Refused(reason) => {
            check_refusal_observation(*reason, reply.observation)?;
            1
        }
        ReplyDisposition::ExactAppliedResultUnavailable(applied) => {
            check_exact(reply.binding, reply.observation, *applied)?;
            ENTRY_BYTES + 8 + 32
        }
        ReplyDisposition::Completed { entry, results } => {
            check_entry_observation(reply.observation, *entry, true)?;
            let expected = reply.binding.intent.count() as usize;
            if results.len() != expected {
                return Err(FrameFailure::ResultCount { observed: results.len(), expected });
            }
            let mut length = ENTRY_BYTES;
            let mut previous = None;
            for result in results {
                check_next(&mut previous, result.tx_seq())?;
                length = length
                    .checked_add(outcome::encoded_len(result, *entry)?)
                    .ok_or(FrameFailure::LengthOverflow)?;
            }
            length
        }
    };
    let length = BASE_BYTES.checked_add(body_extra).ok_or(FrameFailure::LengthOverflow)?;
    check_bytes(length, maximum_frame_bytes)?;
    let body_len = u32::try_from(length - HEADER_BYTES).map_err(|_| FrameFailure::LengthOverflow)?;
    let mut bytes = Vec::with_capacity(length);
    bytes.extend_from_slice(b"SVI0");
    bytes.extend_from_slice(&VERSION.to_le_bytes());
    bytes.extend_from_slice(&REPLY.to_le_bytes());
    bytes.extend_from_slice(&0_u32.to_le_bytes());
    bytes.extend_from_slice(&body_len.to_le_bytes());
    append_context(&mut bytes, &reply.binding.context);
    bytes.extend_from_slice(&reply.binding.intent.encode_v1());
    append_observation(&mut bytes, reply.observation);
    match &reply.disposition {
        ReplyDisposition::IngressAdmitted(entry) => {
            bytes.push(1);
            append_entry(&mut bytes, *entry);
        }
        ReplyDisposition::PendingKnown(entry) => {
            bytes.push(2);
            append_entry(&mut bytes, *entry);
        }
        ReplyDisposition::CommittedAwaitingApply(entry) => {
            bytes.push(3);
            append_entry(&mut bytes, *entry);
        }
        ReplyDisposition::Unknown => bytes.push(4),
        ReplyDisposition::ProcessedResultUnavailable { next_sequence } => {
            bytes.push(5);
            bytes.extend_from_slice(&next_sequence.to_le_bytes());
        }
        ReplyDisposition::RetryableNext => bytes.push(6),
        ReplyDisposition::Conflict => bytes.push(7),
        ReplyDisposition::StreamGapRejected { expected_seq } => {
            bytes.push(10);
            bytes.extend_from_slice(&expected_seq.to_le_bytes());
        }
        ReplyDisposition::Refused(reason) => {
            bytes.push(8);
            bytes.push(refusal_tag(*reason));
        }
        ReplyDisposition::Completed { entry, results } => {
            bytes.push(9);
            append_entry(&mut bytes, *entry);
            for result in results {
                outcome::append(&mut bytes, result);
            }
        }
        ReplyDisposition::ExactAppliedResultUnavailable(applied) => {
            bytes.push(11);
            // The complete intent already appears in the reply binding.
            bytes.extend_from_slice(&applied.encode_v1()[CLIENT_STREAM_BATCH_COMMITMENT_BYTES..]);
        }
    }
    Ok(bytes)
}

pub fn reply_frame_len(header: &[u8; HEADER_BYTES], maximum_frame_bytes: usize) -> Result<usize, FrameFailure> {
    decode_header(header, maximum_frame_bytes, &[REPLY], BASE_BYTES).map(|(_, length)| length)
}

pub fn decode_reply(bytes: &[u8], maximum_frame_bytes: usize, maximum_commands: usize) -> Result<Reply, FrameFailure> {
    check_bytes(bytes.len(), maximum_frame_bytes)?;
    let mut remaining = bytes;
    let length = reply_frame_len(&take(&mut remaining)?, maximum_frame_bytes)?;
    if length != bytes.len() {
        return Err(FrameFailure::InvalidLength);
    }
    let context = take_context(&mut remaining)?;
    let intent = ClientStreamBatchCommitment::decode_v1(
        &take::<CLIENT_STREAM_BATCH_COMMITMENT_BYTES>(&mut remaining)?,
        maximum_commands,
    )
    .map_err(|source| FrameFailure::Intent { source })?;
    let binding = ReplyBinding { context, intent };
    let observation = take_observation(&mut remaining)?;
    let tag = take::<1>(&mut remaining)?[0];
    if observation.is_none() && tag != 8 {
        return Err(FrameFailure::MissingReplyObservation);
    }
    // Reject reason/disposition contradictions before any result allocation.
    check_unavailability(observation, matches!(tag, 4 | 8))?;
    let disposition = match tag {
        1..=3 => {
            let entry = take_entry(&mut remaining)?;
            check_entry_observation(observation, entry, false)?;
            match tag {
                1 => ReplyDisposition::IngressAdmitted(entry),
                2 => ReplyDisposition::PendingKnown(entry),
                _ => ReplyDisposition::CommittedAwaitingApply(entry),
            }
        }
        4 => ReplyDisposition::Unknown,
        5 => {
            check_applied_observation(observation)?;
            let next_sequence = u64::from_le_bytes(take(&mut remaining)?);
            check_frontier(binding, next_sequence)?;
            ReplyDisposition::ProcessedResultUnavailable { next_sequence }
        }
        6 => {
            check_retry_observation(observation)?;
            ReplyDisposition::RetryableNext
        }
        7 => ReplyDisposition::Conflict,
        8 => {
            let reason = take_refusal(take::<1>(&mut remaining)?[0])?;
            check_refusal_observation(reason, observation)?;
            ReplyDisposition::Refused(reason)
        }
        9 => {
            let entry = take_entry(&mut remaining)?;
            check_entry_observation(observation, entry, true)?;
            let count = intent.count() as usize;
            // Every result needs at least a tag and tx_seq. Scan the complete
            // vector, including nested event lengths, before allocating anything.
            if count > remaining.len() / 9 {
                return Err(FrameFailure::InvalidLength);
            }
            let mut scan = remaining;
            let mut previous = None;
            for _ in 0..count {
                check_next(&mut previous, outcome::take_member(&mut scan, entry)?.tx_seq())?;
            }
            if !scan.is_empty() {
                return Err(FrameFailure::InvalidLength);
            }
            let mut results = Vec::with_capacity(count);
            for _ in 0..count {
                results.push(outcome::take_member(&mut remaining, entry)?.materialize()?);
            }
            ReplyDisposition::Completed { entry, results }
        }
        10 => {
            let expected_seq = u64::from_le_bytes(take(&mut remaining)?);
            check_gap(binding, expected_seq)?;
            ReplyDisposition::StreamGapRejected { expected_seq }
        }
        11 => {
            let entry = take_entry(&mut remaining)?;
            let applied = AppliedStreamBatchBindingV1::new(
                intent,
                entry.index.get(),
                entry.term.get(),
                entry.digest,
                u64::from_le_bytes(take(&mut remaining)?),
                statevec_frame::Digest32::new(take(&mut remaining)?),
            )
            .map_err(|source| FrameFailure::AppliedBinding { source })?;
            check_exact(binding, observation, applied)?;
            ReplyDisposition::ExactAppliedResultUnavailable(applied)
        }
        observed => return Err(FrameFailure::UnsupportedDisposition { observed }),
    };
    if !remaining.is_empty() {
        return Err(FrameFailure::InvalidLength);
    }
    Ok(Reply { binding, observation, disposition })
}

fn check_exact(
    binding: ReplyBinding,
    observation: Option<ReplyObservation>,
    applied: AppliedStreamBatchBindingV1,
) -> Result<(), FrameFailure> {
    if binding.intent != applied.intent() {
        return Err(FrameFailure::AppliedIntentMismatch);
    }
    check_entry_observation(
        observation,
        EntryBinding {
            index: NonZeroU64::new(applied.raft_index()).expect("checked binding index"),
            term: NonZeroU64::new(applied.raft_term()).expect("checked binding term"),
            digest: applied.entry_digest(),
        },
        true,
    )
}

fn check_retry_observation(observation: Option<ReplyObservation>) -> Result<(), FrameFailure> {
    let observation = observation.ok_or(FrameFailure::MissingReplyObservation)?;
    if observation.current_term() == 0 || observation.applied_term() != observation.current_term() {
        return Err(FrameFailure::InvalidReplyObservation);
    }
    Ok(())
}

fn check_refusal_observation(
    reason: BatchSubmitRefusal,
    observation: Option<ReplyObservation>,
) -> Result<(), FrameFailure> {
    if reason == BatchSubmitRefusal::IncompatibleContext && observation.is_some() {
        return Err(FrameFailure::InvalidReplyObservation);
    }
    check_unavailability(
        observation,
        matches!(
            reason,
            BatchSubmitRefusal::NotReady
                | BatchSubmitRefusal::Busy
                | BatchSubmitRefusal::ShuttingDown
                | BatchSubmitRefusal::InternalFailure
        ),
    )?;
    Ok(())
}

fn check_unavailability(observation: Option<ReplyObservation>, allowed: bool) -> Result<(), FrameFailure> {
    if !allowed && observation.is_some_and(|o| o.unavailable_reason().is_some()) {
        return Err(FrameFailure::InvalidReplyObservation);
    }
    Ok(())
}

fn check_applied_observation(observation: Option<ReplyObservation>) -> Result<(), FrameFailure> {
    let observation = observation.ok_or(FrameFailure::MissingReplyObservation)?;
    if observation.applied_index() == 0 {
        return Err(FrameFailure::InvalidAppliedFrontier);
    }
    Ok(())
}

fn check_entry_observation(
    observation: Option<ReplyObservation>,
    entry: EntryBinding,
    applied: bool,
) -> Result<(), FrameFailure> {
    let observation = observation.ok_or(FrameFailure::MissingReplyObservation)?;
    let index = entry.index.get();
    let term = entry.term.get();
    let ordered = match index.cmp(&observation.applied_index()) {
        std::cmp::Ordering::Less => term <= observation.applied_term(),
        std::cmp::Ordering::Equal => term == observation.applied_term(),
        std::cmp::Ordering::Greater => term >= observation.applied_term(),
    };
    if term > observation.current_term() || !ordered || (index <= observation.applied_index()) != applied {
        return Err(FrameFailure::EntryObservationMismatch);
    }
    Ok(())
}

fn append_observation(bytes: &mut Vec<u8>, observation: Option<ReplyObservation>) {
    if let Some(observation) = observation {
        bytes.push(1);
        bytes.extend_from_slice(&observation.current_term().to_le_bytes());
        bytes.extend_from_slice(&observation.applied_index().to_le_bytes());
        bytes.extend_from_slice(&observation.applied_term().to_le_bytes());
        bytes.extend_from_slice(&observation.leader_hint().map_or(0, NonZeroU64::get).to_le_bytes());
        bytes.push(observation.unavailable_reason().map_or(0, |reason| reason as u8));
    } else {
        bytes.extend_from_slice(&[0; OBSERVATION_BYTES]);
    }
}

fn take_observation(remaining: &mut &[u8]) -> Result<Option<ReplyObservation>, FrameFailure> {
    let tag = take::<1>(remaining)?[0];
    let term = u64::from_le_bytes(take(remaining)?);
    let index = u64::from_le_bytes(take(remaining)?);
    let applied_term = u64::from_le_bytes(take(remaining)?);
    let leader = u64::from_le_bytes(take(remaining)?);
    let reason = take::<1>(remaining)?[0];
    match tag {
        0 if term == 0 && index == 0 && applied_term == 0 && leader == 0 && reason == 0 => Ok(None),
        1 => {
            ReplyObservation::new(term, index, applied_term, NonZeroU64::new(leader), take_unavailable_reason(reason)?)
                .map(Some)
                .map_err(|_| FrameFailure::InvalidReplyObservation)
        }
        _ => Err(FrameFailure::InvalidReplyObservation),
    }
}

fn take_unavailable_reason(tag: u8) -> Result<Option<IngressUnavailableReason>, FrameFailure> {
    use IngressUnavailableReason::*;
    Ok(Some(match tag {
        0 => return Ok(None),
        1 => NotLeader,
        2 => LeaderNotReady,
        3 => EngineOccupied,
        4 => ReadyBacklog,
        5 => SnapshotInstalling,
        6 => HistoryScanLimited,
        7 => Draining,
        8 => Failed,
        observed => return Err(FrameFailure::UnsupportedUnavailableReason { observed }),
    }))
}

fn check_intent(intent: ClientStreamBatchCommitment, maximum_commands: usize) -> Result<(), FrameFailure> {
    let observed = intent.count() as usize;
    if observed > maximum_commands {
        return Err(FrameFailure::Intent {
            source: statevec_frame::ClientStreamBatchFailure::InvalidCount { observed, limit: maximum_commands },
        });
    }
    Ok(())
}

fn check_frontier(binding: ReplyBinding, next_sequence: u64) -> Result<(), FrameFailure> {
    if next_sequence < binding.intent.next_sequence() {
        return Err(FrameFailure::InvalidAppliedFrontier);
    }
    Ok(())
}

fn check_next(previous: &mut Option<u64>, sequence: u64) -> Result<(), FrameFailure> {
    if sequence == 0 || sequence == u64::MAX {
        return Err(FrameFailure::InvalidOutcome);
    }
    if previous.is_some_and(|value| value.checked_add(1) != Some(sequence)) {
        return Err(FrameFailure::NoncontiguousResults);
    }
    *previous = Some(sequence);
    Ok(())
}

fn check_gap(binding: ReplyBinding, expected_seq: u64) -> Result<(), FrameFailure> {
    if expected_seq == 0
        || expected_seq == binding.intent.first_sequence()
        || expected_seq >= binding.intent.next_sequence()
    {
        return Err(FrameFailure::InvalidGapFrontier);
    }
    Ok(())
}

fn append_entry(bytes: &mut Vec<u8>, entry: EntryBinding) {
    bytes.extend_from_slice(&entry.index.get().to_le_bytes());
    bytes.extend_from_slice(&entry.term.get().to_le_bytes());
    bytes.extend_from_slice(&entry.digest.bytes());
}

fn take_entry(remaining: &mut &[u8]) -> Result<EntryBinding, FrameFailure> {
    Ok(EntryBinding {
        index: NonZeroU64::new(u64::from_le_bytes(take(remaining)?)).ok_or(FrameFailure::InvalidEntryBinding)?,
        term: NonZeroU64::new(u64::from_le_bytes(take(remaining)?)).ok_or(FrameFailure::InvalidEntryBinding)?,
        digest: EntryDigest::from_untrusted_wire(take(remaining)?),
    })
}

fn refusal_tag(reason: BatchSubmitRefusal) -> u8 {
    match reason {
        BatchSubmitRefusal::NotReady => 1,
        BatchSubmitRefusal::Busy => 2,
        BatchSubmitRefusal::TooLarge => 3,
        BatchSubmitRefusal::InvalidInput => 4,
        BatchSubmitRefusal::Unauthorized => 5,
        BatchSubmitRefusal::ShuttingDown => 6,
        BatchSubmitRefusal::IncompatibleContext => 7,
        BatchSubmitRefusal::InternalFailure => 8,
    }
}

fn take_refusal(tag: u8) -> Result<BatchSubmitRefusal, FrameFailure> {
    Ok(match tag {
        1 => BatchSubmitRefusal::NotReady,
        2 => BatchSubmitRefusal::Busy,
        3 => BatchSubmitRefusal::TooLarge,
        4 => BatchSubmitRefusal::InvalidInput,
        5 => BatchSubmitRefusal::Unauthorized,
        6 => BatchSubmitRefusal::ShuttingDown,
        7 => BatchSubmitRefusal::IncompatibleContext,
        8 => BatchSubmitRefusal::InternalFailure,
        observed => return Err(FrameFailure::UnsupportedRefusal { observed }),
    })
}
