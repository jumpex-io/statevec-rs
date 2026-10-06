//! Borrowed response scanning keeps malformed later members from allocating
//! earlier member/event vectors. This is a wire view, never an apply result owner.

use super::{EntryBinding, FrameFailure, MemberResult, take};
use statevec_frame::outcome::{CommitOutcome, InlineResponseEvent};
use std::num::NonZeroU64;

const OUTCOME_BYTES: usize = 93;
const EVENT_HEADER_BYTES: usize = 10;

pub(super) enum MemberView<'a> {
    Unavailable(NonZeroU64),
    Outcome { value: CommitOutcome, events: &'a [u8], count: usize },
}

impl MemberView<'_> {
    pub(super) fn tx_seq(&self) -> u64 {
        match self {
            Self::Unavailable(sequence) => sequence.get(),
            Self::Outcome { value, .. } => value.tx_seq,
        }
    }

    pub(super) fn materialize(self) -> Result<MemberResult, FrameFailure> {
        Ok(match self {
            Self::Unavailable(tx_seq) => MemberResult::Unavailable { tx_seq },
            Self::Outcome { mut value, mut events, count } => {
                value.inline_events = Vec::with_capacity(count);
                for _ in 0..count {
                    let (event_seq, event_kind, payload) = take_event(&mut events)?;
                    value.inline_events.push(InlineResponseEvent {
                        tx_seq: value.tx_seq,
                        event_seq,
                        event_kind,
                        payload: payload.to_vec(),
                    });
                }
                MemberResult::Outcome(value)
            }
        })
    }
}

pub(super) fn encoded_len(result: &MemberResult, entry: EntryBinding) -> Result<usize, FrameFailure> {
    let value = match result {
        MemberResult::Unavailable { .. } => return Ok(9),
        MemberResult::Outcome(value) => value,
    };
    if value.tx_seq == 0
        || value.applied_by == 0
        || value.raft_index != entry.index.get()
        || value.raft_term != entry.term.get()
    {
        return Err(FrameFailure::InvalidOutcome);
    }
    check_event_count(value.inline_events.len(), value.emitted_event_count, value.inline_events_truncated)?;
    let mut length = 1 + OUTCOME_BYTES;
    let mut previous = None;
    for event in &value.inline_events {
        if event.tx_seq != value.tx_seq {
            return Err(FrameFailure::InvalidInlineEvent);
        }
        check_event(&mut previous, event.event_seq, event.event_kind, value.emitted_event_count)?;
        u32::try_from(event.payload.len()).map_err(|_| FrameFailure::LengthOverflow)?;
        length = length
            .checked_add(EVENT_HEADER_BYTES)
            .and_then(|n| n.checked_add(event.payload.len()))
            .ok_or(FrameFailure::LengthOverflow)?;
    }
    Ok(length)
}

pub(super) fn append(bytes: &mut Vec<u8>, result: &MemberResult) {
    match result {
        MemberResult::Unavailable { tx_seq } => {
            bytes.push(0);
            bytes.extend_from_slice(&tx_seq.get().to_le_bytes());
        }
        MemberResult::Outcome(value) => {
            bytes.push(1);
            bytes.extend_from_slice(&value.applied_by.to_le_bytes());
            bytes.extend_from_slice(&value.tx_seq.to_le_bytes());
            bytes.extend_from_slice(&value.tx_id);
            bytes.extend_from_slice(&value.tx_chain_hash);
            bytes.extend_from_slice(&value.sys_status_code.to_le_bytes());
            bytes.extend_from_slice(&value.biz_status_code.to_le_bytes());
            bytes.extend_from_slice(&value.emitted_event_count.to_le_bytes());
            bytes.extend_from_slice(&(value.inline_events.len() as u32).to_le_bytes());
            bytes.push(u8::from(value.inline_events_truncated));
            for event in &value.inline_events {
                bytes.extend_from_slice(&event.event_seq.to_le_bytes());
                bytes.extend_from_slice(&event.event_kind.to_le_bytes());
                bytes.extend_from_slice(&(event.payload.len() as u32).to_le_bytes());
                bytes.extend_from_slice(&event.payload);
            }
        }
    }
}

pub(super) fn take_member<'a>(remaining: &mut &'a [u8], entry: EntryBinding) -> Result<MemberView<'a>, FrameFailure> {
    match take::<1>(remaining)?[0] {
        0 => Ok(MemberView::Unavailable(
            NonZeroU64::new(u64::from_le_bytes(take(remaining)?)).ok_or(FrameFailure::InvalidOutcome)?,
        )),
        1 => {
            let value = CommitOutcome {
                applied_by: u64::from_le_bytes(take(remaining)?),
                tx_seq: u64::from_le_bytes(take(remaining)?),
                tx_id: take(remaining)?,
                tx_chain_hash: take(remaining)?,
                raft_index: entry.index.get(),
                raft_term: entry.term.get(),
                sys_status_code: u16::from_le_bytes(take(remaining)?),
                biz_status_code: u16::from_le_bytes(take(remaining)?),
                emitted_event_count: u32::from_le_bytes(take(remaining)?),
                inline_events: Vec::new(),
                inline_events_truncated: false,
            };
            if value.applied_by == 0 || value.tx_seq == 0 {
                return Err(FrameFailure::InvalidOutcome);
            }
            let count = u32::from_le_bytes(take(remaining)?) as usize;
            let truncated = match take::<1>(remaining)?[0] {
                0 => false,
                1 => true,
                _ => return Err(FrameFailure::InvalidInlineEvent),
            };
            check_event_count(count, value.emitted_event_count, truncated)?;
            if count > remaining.len() / EVENT_HEADER_BYTES {
                return Err(FrameFailure::InvalidLength);
            }
            let events = *remaining;
            let mut previous = None;
            for _ in 0..count {
                let (sequence, kind, _) = take_event(remaining)?;
                check_event(&mut previous, sequence, kind, value.emitted_event_count)?;
            }
            let length = events.len() - remaining.len();
            Ok(MemberView::Outcome {
                value: CommitOutcome { inline_events_truncated: truncated, ..value },
                events: &events[..length],
                count,
            })
        }
        observed => Err(FrameFailure::UnsupportedMemberResult { observed }),
    }
}

fn check_event_count(count: usize, emitted: u32, truncated: bool) -> Result<(), FrameFailure> {
    if count > emitted as usize || (truncated && count == emitted as usize) {
        return Err(FrameFailure::InvalidInlineEvent);
    }
    Ok(())
}

fn check_event(previous: &mut Option<u32>, sequence: u32, kind: u16, emitted: u32) -> Result<(), FrameFailure> {
    if kind == 0 || sequence >= emitted || previous.is_some_and(|before| before >= sequence) {
        return Err(FrameFailure::InvalidInlineEvent);
    }
    *previous = Some(sequence);
    Ok(())
}

fn take_event<'a>(remaining: &mut &'a [u8]) -> Result<(u32, u16, &'a [u8]), FrameFailure> {
    let sequence = u32::from_le_bytes(take(remaining)?);
    let kind = u16::from_le_bytes(take(remaining)?);
    let length = u32::from_le_bytes(take(remaining)?) as usize;
    let (payload, rest) = remaining.split_at_checked(length).ok_or(FrameFailure::InvalidLength)?;
    *remaining = rest;
    Ok((sequence, kind, payload))
}
