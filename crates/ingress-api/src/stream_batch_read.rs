//! Direct business reads: request kind 4 and reply kind 5. A read reply is an
//! observation of one node's applied state for its requester. It does not
//! settle a submission, mint commit evidence or authorize another request.
//!
//! Request after the header and context: a minimum flag (0 or 1) and the
//! minimum `applied_tx_seq` (zero when absent), a key count (u16), then each
//! key: tag (1 `sys_id`, 2 unique key), kind (u16), unique-key id (u8, zero
//! for `sys_id`), and either the `sys_id` (u64) or a key length (u16) and the
//! key bytes.
//!
//! Reply after the header and context: the observation (a flag, current term,
//! applied index, applied term, committed index, leader hint, role; all zero
//! when absent), then outcome 1 with the answer or 2 with a refusal. The answer
//! is the engine frontier (`applied_tx_seq`, `tx_chain_hash`), a record count
//! (u16), then per key a presence flag and, when present, kind (u16),
//! `sys_id`, revision, a data length (u32) and the data. A refusal is its tag
//! and one u64 argument, nonzero only as `NotYetVisible`'s `applied_tx_seq`.

use super::{
    CONTEXT_BYTES, FrameError, HEADER_BYTES, RequestContext, append_context, finish_frame, start_frame, take,
    take_context, verified_body,
};
use statevec_frame::Digest32;
use statevec_frame::read_protocol::{ReadKey, ReadLimits, ReadObservation, ReadRefusal, ReadRole};
use std::num::NonZeroU64;

pub(super) const READ: u16 = 4;
pub(super) const READ_REPLY: u16 = 5;
const MINIMUM_BYTES: usize = 9;
const KEY_HEAD_BYTES: usize = 4;
const SYS_ID_KEY_BYTES: usize = KEY_HEAD_BYTES + 8;
// A unique key's head and length, before its bytes.
const UNIQUE_KEY_HEAD_BYTES: usize = KEY_HEAD_BYTES + 2;
pub const READ_OBSERVATION_BYTES: usize = 42;
const REFUSAL_BYTES: usize = 9;
const ANSWER_HEAD_BYTES: usize = 8 + 32 + 2;
const RECORD_HEAD_BYTES: usize = 1 + 2 + 8 + 8 + 4;
const REQUEST_PREFIX_BYTES: usize = HEADER_BYTES + CONTEXT_BYTES + MINIMUM_BYTES + 2;
const REPLY_PREFIX_BYTES: usize = HEADER_BYTES + CONTEXT_BYTES + READ_OBSERVATION_BYTES + 1;

// The shortest read request holds one empty unique key.
pub(super) const MIN_REQUEST_BYTES: usize = REQUEST_PREFIX_BYTES + UNIQUE_KEY_HEAD_BYTES;
pub(super) const MIN_REPLY_BYTES: usize = REPLY_PREFIX_BYTES + REFUSAL_BYTES;

/// One direct client read. The context binds the cluster as a batch's does;
/// it carries no stream identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadRequest {
    pub context: RequestContext,
    pub min_applied_tx_seq: Option<u64>,
    pub keys: Vec<ReadKey>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ReadRecord {
    pub kind: u16,
    pub sys_id: u64,
    pub revision: u64,
    pub data: Vec<u8>,
}

impl ReadRecord {
    pub fn new(kind: u16, sys_id: u64, revision: u64, data: Vec<u8>) -> Self {
        Self { kind, sys_id, revision, data }
    }
}

/// A record borrowed from the serving engine's answer, for encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadRecordRef<'a> {
    pub kind: u16,
    pub sys_id: u64,
    pub revision: u64,
    pub data: &'a [u8],
}

/// Every record at one position, one optional record per key in request
/// order. The position's Raft entry is the observation's applied entry.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ReadAnswer {
    pub applied_tx_seq: u64,
    pub tx_chain_hash: Digest32,
    pub records: Vec<Option<ReadRecord>>,
}

impl ReadAnswer {
    pub fn new(applied_tx_seq: u64, tx_chain_hash: Digest32, records: Vec<Option<ReadRecord>>) -> Self {
        Self { applied_tx_seq, tx_chain_hash, records }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadOutcome {
    Answered(ReadAnswer),
    Refused(ReadRefusal),
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ReadReply {
    pub context: RequestContext,
    pub observation: Option<ReadObservation>,
    pub outcome: ReadOutcome,
}

impl ReadReply {
    pub fn new(context: RequestContext, observation: Option<ReadObservation>, outcome: ReadOutcome) -> Self {
        Self { context, observation, outcome }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum ReadReplyBody<'a> {
    Answered { applied_tx_seq: u64, tx_chain_hash: Digest32, records: &'a [Option<ReadRecordRef<'a>>] },
    Refused(ReadRefusal),
}

/// Encode under a total-frame limit and the origin's key bounds. Failure
/// leaves the request unchanged.
pub fn encode_read_request(
    request: &ReadRequest,
    maximum_frame_bytes: usize,
    limits: ReadLimits,
) -> Result<Vec<u8>, FrameError> {
    check_key_count(request.keys.len(), limits)?;
    let mut frame_len = REQUEST_PREFIX_BYTES;
    for (index, key) in request.keys.iter().enumerate() {
        let key_len = match key {
            ReadKey::SysId { .. } => SYS_ID_KEY_BYTES,
            ReadKey::UniqueKey { key, .. } => {
                check_key_length(index, key.len(), limits)?;
                UNIQUE_KEY_HEAD_BYTES + key.len()
            }
        };
        frame_len = frame_len.checked_add(key_len).ok_or(FrameError::LengthOverflow)?;
    }
    let mut bytes = start_frame(frame_len, maximum_frame_bytes)?;
    append_context(&mut bytes, &request.context);
    bytes.push(u8::from(request.min_applied_tx_seq.is_some()));
    bytes.extend_from_slice(&request.min_applied_tx_seq.unwrap_or(0).to_le_bytes());
    bytes.extend_from_slice(&(request.keys.len() as u16).to_le_bytes());
    for key in &request.keys {
        match key {
            ReadKey::SysId { kind, sys_id } => {
                bytes.push(1);
                bytes.extend_from_slice(&kind.to_le_bytes());
                bytes.push(0);
                bytes.extend_from_slice(&sys_id.to_le_bytes());
            }
            ReadKey::UniqueKey { kind, uk_id, key } => {
                bytes.push(2);
                bytes.extend_from_slice(&kind.to_le_bytes());
                bytes.push(*uk_id);
                bytes.extend_from_slice(&(key.len() as u16).to_le_bytes());
                bytes.extend_from_slice(key);
            }
        }
    }
    finish_frame(bytes, READ)
}

/// Validate the complete frame and every key bound before allocating keys.
pub fn decode_read_request(
    bytes: &[u8],
    maximum_frame_bytes: usize,
    limits: ReadLimits,
) -> Result<ReadRequest, FrameError> {
    let (_, body) = verified_body(bytes, maximum_frame_bytes, &[READ])?;
    read_request_body(body, limits)
}

pub(super) fn read_request_body(mut remaining: &[u8], limits: ReadLimits) -> Result<ReadRequest, FrameError> {
    let context = take_context(&mut remaining)?;
    let flag = take::<1>(&mut remaining)?[0];
    let minimum = u64::from_le_bytes(take(&mut remaining)?);
    let min_applied_tx_seq = match flag {
        0 if minimum == 0 => None,
        1 => Some(minimum),
        _ => return Err(FrameError::InvalidReadMinimum),
    };
    let count = usize::from(u16::from_le_bytes(take(&mut remaining)?));
    check_key_count(count, limits)?;
    // Scan every key against its bounds before allocating any of them.
    let mut scan = remaining;
    for index in 0..count {
        take_key(&mut scan, index, limits)?;
    }
    if !scan.is_empty() {
        return Err(FrameError::InvalidLength);
    }
    let mut keys = Vec::with_capacity(count);
    for index in 0..count {
        keys.push(take_key(&mut remaining, index, limits)?.to_key());
    }
    Ok(ReadRequest { context, min_applied_tx_seq, keys })
}

enum KeyView<'a> {
    SysId { kind: u16, sys_id: u64 },
    UniqueKey { kind: u16, uk_id: u8, key: &'a [u8] },
}

impl KeyView<'_> {
    fn to_key(&self) -> ReadKey {
        match *self {
            Self::SysId { kind, sys_id } => ReadKey::SysId { kind, sys_id },
            Self::UniqueKey { kind, uk_id, key } => ReadKey::UniqueKey { kind, uk_id, key: key.to_vec() },
        }
    }
}

fn take_key<'a>(remaining: &mut &'a [u8], index: usize, limits: ReadLimits) -> Result<KeyView<'a>, FrameError> {
    let tag = take::<1>(remaining)?[0];
    let kind = u16::from_le_bytes(take(remaining)?);
    let uk_id = take::<1>(remaining)?[0];
    match tag {
        1 if uk_id == 0 => Ok(KeyView::SysId { kind, sys_id: u64::from_le_bytes(take(remaining)?) }),
        1 => Err(FrameError::InvalidReadKey),
        2 => {
            let length = usize::from(u16::from_le_bytes(take(remaining)?));
            check_key_length(index, length, limits)?;
            let (key, rest) = remaining.split_at_checked(length).ok_or(FrameError::InvalidLength)?;
            *remaining = rest;
            Ok(KeyView::UniqueKey { kind, uk_id, key })
        }
        observed => Err(FrameError::UnsupportedReadKey { observed }),
    }
}

/// Encode one reply under the transport frame limit. An answer needs the
/// observation stamped with it; `IncompatibleContext` carries none.
pub fn encode_read_reply(
    context: &RequestContext,
    observation: Option<ReadObservation>,
    body: ReadReplyBody<'_>,
    maximum_frame_bytes: usize,
) -> Result<Vec<u8>, FrameError> {
    let frame_len = match body {
        ReadReplyBody::Answered { records, .. } => {
            check_answer_observation(observation)?;
            u16::try_from(records.len()).map_err(|_| FrameError::LengthOverflow)?;
            let mut length = REPLY_PREFIX_BYTES + ANSWER_HEAD_BYTES;
            for record in records {
                let record_len = match record {
                    None => 1,
                    Some(record) => {
                        u32::try_from(record.data.len()).map_err(|_| FrameError::LengthOverflow)?;
                        RECORD_HEAD_BYTES + record.data.len()
                    }
                };
                length = length.checked_add(record_len).ok_or(FrameError::LengthOverflow)?;
            }
            length
        }
        ReadReplyBody::Refused(refusal) => {
            refusal_tag(refusal)?;
            check_refusal_observation(refusal, observation)?;
            REPLY_PREFIX_BYTES + REFUSAL_BYTES
        }
    };
    let mut bytes = start_frame(frame_len, maximum_frame_bytes)?;
    append_context(&mut bytes, context);
    append_observation(&mut bytes, observation);
    match body {
        ReadReplyBody::Answered { applied_tx_seq, tx_chain_hash, records } => {
            bytes.push(1);
            bytes.extend_from_slice(&applied_tx_seq.to_le_bytes());
            bytes.extend_from_slice(&tx_chain_hash.bytes());
            bytes.extend_from_slice(&(records.len() as u16).to_le_bytes());
            for record in records {
                match record {
                    None => bytes.push(0),
                    Some(record) => {
                        bytes.push(1);
                        bytes.extend_from_slice(&record.kind.to_le_bytes());
                        bytes.extend_from_slice(&record.sys_id.to_le_bytes());
                        bytes.extend_from_slice(&record.revision.to_le_bytes());
                        bytes.extend_from_slice(&(record.data.len() as u32).to_le_bytes());
                        bytes.extend_from_slice(record.data);
                    }
                }
            }
        }
        ReadReplyBody::Refused(refusal) => {
            bytes.push(2);
            bytes.push(refusal_tag(refusal)?);
            let argument = match refusal {
                ReadRefusal::NotYetVisible { applied_tx_seq } => applied_tx_seq,
                ReadRefusal::Unrecognized { argument, .. } => argument,
                _ => 0,
            };
            bytes.extend_from_slice(&argument.to_le_bytes());
        }
    }
    finish_frame(bytes, READ_REPLY)
}

/// Validate the complete frame, the record count and the total record bytes
/// against the origin's bounds before allocating records.
pub fn decode_read_reply(
    bytes: &[u8],
    maximum_frame_bytes: usize,
    limits: ReadLimits,
) -> Result<ReadReply, FrameError> {
    let (_, body) = verified_body(bytes, maximum_frame_bytes, &[READ_REPLY])?;
    read_reply_body(body, limits)
}

pub(super) fn read_reply_body(mut remaining: &[u8], limits: ReadLimits) -> Result<ReadReply, FrameError> {
    let context = take_context(&mut remaining)?;
    let observation = take_observation(&mut remaining)?;
    let outcome = match take::<1>(&mut remaining)?[0] {
        1 => {
            check_answer_observation(observation)?;
            let applied_tx_seq = u64::from_le_bytes(take(&mut remaining)?);
            let tx_chain_hash = Digest32::new(take(&mut remaining)?);
            let count = usize::from(u16::from_le_bytes(take(&mut remaining)?));
            check_key_count(count, limits)?;
            let mut scan = remaining;
            let mut record_bytes = 0_usize;
            for _ in 0..count {
                if let Some(data) = take_record(&mut scan)?.map(|(_, data)| data.len()) {
                    record_bytes = record_bytes.checked_add(data).ok_or(FrameError::LengthOverflow)?;
                    if record_bytes > limits.max_record_bytes {
                        return Err(FrameError::ReadRecordBytes {
                            observed: record_bytes,
                            limit: limits.max_record_bytes,
                        });
                    }
                }
            }
            let mut records = Vec::with_capacity(count);
            for _ in 0..count {
                let record = take_record(&mut remaining)?
                    .map(|(head, data)| ReadRecord::new(head.kind, head.sys_id, head.revision, data.to_vec()));
                records.push(record);
            }
            ReadOutcome::Answered(ReadAnswer::new(applied_tx_seq, tx_chain_hash, records))
        }
        2 => {
            let tag = take::<1>(&mut remaining)?[0];
            let argument = u64::from_le_bytes(take(&mut remaining)?);
            let refusal = take_refusal(tag, argument)?;
            check_refusal_observation(refusal, observation)?;
            ReadOutcome::Refused(refusal)
        }
        observed => return Err(FrameError::UnsupportedReadOutcome { observed }),
    };
    Ok(ReadReply::new(context, observation, outcome))
}

struct RecordHead {
    kind: u16,
    sys_id: u64,
    revision: u64,
}

fn take_record<'a>(remaining: &mut &'a [u8]) -> Result<Option<(RecordHead, &'a [u8])>, FrameError> {
    match take::<1>(remaining)?[0] {
        0 => Ok(None),
        1 => {
            let head = RecordHead {
                kind: u16::from_le_bytes(take(remaining)?),
                sys_id: u64::from_le_bytes(take(remaining)?),
                revision: u64::from_le_bytes(take(remaining)?),
            };
            let length =
                usize::try_from(u32::from_le_bytes(take(remaining)?)).map_err(|_| FrameError::LengthOverflow)?;
            let (data, rest) = remaining.split_at_checked(length).ok_or(FrameError::InvalidLength)?;
            *remaining = rest;
            Ok(Some((head, data)))
        }
        _ => Err(FrameError::InvalidReadRecord),
    }
}

fn check_key_count(count: usize, limits: ReadLimits) -> Result<(), FrameError> {
    if count == 0 || count > limits.max_keys || count > usize::from(u16::MAX) {
        return Err(FrameError::ReadKeyCount { observed: count, limit: limits.max_keys });
    }
    Ok(())
}

fn check_key_length(index: usize, observed: usize, limits: ReadLimits) -> Result<(), FrameError> {
    if observed > limits.max_key_bytes || observed > usize::from(u16::MAX) {
        return Err(FrameError::ReadKeyLength { index, observed, limit: limits.max_key_bytes });
    }
    Ok(())
}

/// An answer states its position, so its observation has an applied entry.
fn check_answer_observation(observation: Option<ReadObservation>) -> Result<(), FrameError> {
    match observation {
        None => Err(FrameError::MissingReadObservation),
        Some(observation) if observation.applied_index() == 0 => Err(FrameError::InvalidReadObservation),
        Some(_) => Ok(()),
    }
}

fn check_refusal_observation(refusal: ReadRefusal, observation: Option<ReadObservation>) -> Result<(), FrameError> {
    if refusal == ReadRefusal::IncompatibleContext && observation.is_some() {
        return Err(FrameError::InvalidReadObservation);
    }
    Ok(())
}

fn append_observation(bytes: &mut Vec<u8>, observation: Option<ReadObservation>) {
    let Some(observation) = observation else {
        bytes.extend_from_slice(&[0; READ_OBSERVATION_BYTES]);
        return;
    };
    bytes.push(1);
    bytes.extend_from_slice(&observation.current_term().to_le_bytes());
    bytes.extend_from_slice(&observation.applied_index().to_le_bytes());
    bytes.extend_from_slice(&observation.applied_term().to_le_bytes());
    bytes.extend_from_slice(&observation.committed_index().to_le_bytes());
    bytes.extend_from_slice(&observation.leader_hint().map_or(0, NonZeroU64::get).to_le_bytes());
    bytes.push(match observation.role() {
        ReadRole::Leader => 1,
        ReadRole::Follower => 2,
        ReadRole::Candidate => 3,
        ReadRole::Learner => 4,
    });
}

fn take_observation(remaining: &mut &[u8]) -> Result<Option<ReadObservation>, FrameError> {
    let flag = take::<1>(remaining)?[0];
    let current_term = u64::from_le_bytes(take(remaining)?);
    let applied_index = u64::from_le_bytes(take(remaining)?);
    let applied_term = u64::from_le_bytes(take(remaining)?);
    let committed_index = u64::from_le_bytes(take(remaining)?);
    let leader = u64::from_le_bytes(take(remaining)?);
    let role = take::<1>(remaining)?[0];
    if flag == 0 {
        let empty = current_term == 0
            && applied_index == 0
            && applied_term == 0
            && committed_index == 0
            && leader == 0
            && role == 0;
        return if empty { Ok(None) } else { Err(FrameError::InvalidReadObservation) };
    }
    if flag != 1 {
        return Err(FrameError::InvalidReadObservation);
    }
    let role = match role {
        1 => ReadRole::Leader,
        2 => ReadRole::Follower,
        3 => ReadRole::Candidate,
        4 => ReadRole::Learner,
        observed => return Err(FrameError::UnsupportedReadRole { observed }),
    };
    ReadObservation::new(current_term, applied_index, applied_term, committed_index, NonZeroU64::new(leader), role)
        .map(Some)
        .map_err(|_| FrameError::InvalidReadObservation)
}

fn refusal_tag(refusal: ReadRefusal) -> Result<u8, FrameError> {
    Ok(match refusal {
        ReadRefusal::IncompatibleContext => 1,
        ReadRefusal::Fatal => 2,
        ReadRefusal::Draining => 3,
        ReadRefusal::ReadsDisabled => 4,
        ReadRefusal::NotVoter => 5,
        ReadRefusal::LeaderReadsDisabled => 6,
        ReadRefusal::RecoveryPending => 7,
        ReadRefusal::SnapshotInstalling => 8,
        ReadRefusal::RequestTooLarge => 9,
        ReadRefusal::Busy => 10,
        ReadRefusal::NotYetVisible { .. } => 11,
        ReadRefusal::UnknownKind => 12,
        ReadRefusal::UnknownUniqueKey => 13,
        ReadRefusal::ReplyTooLarge => 14,
        ReadRefusal::Unavailable => 15,
        ReadRefusal::Unrecognized { observed, argument } if take_refusal(observed, argument) == Ok(refusal) => observed,
        ReadRefusal::Unrecognized { observed, .. } => return Err(FrameError::UnsupportedReadRefusal { observed }),
        ReadRefusal::ProjectorRebuilding | ReadRefusal::Unsupported => return Err(FrameError::NonClientReadRefusal),
        _ => return Err(FrameError::UnencodableReadRefusal { reason: refusal }),
    })
}

fn take_refusal(tag: u8, argument: u64) -> Result<ReadRefusal, FrameError> {
    let refusal = match tag {
        1 => ReadRefusal::IncompatibleContext,
        2 => ReadRefusal::Fatal,
        3 => ReadRefusal::Draining,
        4 => ReadRefusal::ReadsDisabled,
        5 => ReadRefusal::NotVoter,
        6 => ReadRefusal::LeaderReadsDisabled,
        7 => ReadRefusal::RecoveryPending,
        8 => ReadRefusal::SnapshotInstalling,
        9 => ReadRefusal::RequestTooLarge,
        10 => ReadRefusal::Busy,
        11 => return Ok(ReadRefusal::NotYetVisible { applied_tx_seq: argument }),
        12 => ReadRefusal::UnknownKind,
        13 => ReadRefusal::UnknownUniqueKey,
        14 => ReadRefusal::ReplyTooLarge,
        15 => ReadRefusal::Unavailable,
        0 => return Err(FrameError::UnsupportedReadRefusal { observed: 0 }),
        observed => return Ok(ReadRefusal::Unrecognized { observed, argument }),
    };
    if argument != 0 {
        return Err(FrameError::InvalidReadRefusal);
    }
    Ok(refusal)
}
