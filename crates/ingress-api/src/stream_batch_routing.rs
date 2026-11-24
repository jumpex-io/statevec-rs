//! A read-only snapshot of the routing facts observed at one member.
use std::num::NonZeroU64;

use super::{
    FrameError, PREFIX_BYTES, RequestContext, append_context, finish_frame, start_frame,
    take, take_context, verified_body,
};

pub(super) const ROUTE: u16 = 6;
pub(super) const ROUTE_REPLY: u16 = 7;
const MAX_MEMBERS: usize = 32;
const ROUTE_FIELDS_BYTES: usize = 4 * 8 + 2 + 4;
pub(super) const MIN_REPLY_BYTES: usize = PREFIX_BYTES + ROUTE_FIELDS_BYTES;

/// The member sets are the responding peer's complete applied configuration.
/// Use this observation for routing. The leader still admits each operation.
/// A later minor release may add fields; build one with [`RouteObservation::new`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RouteObservation {
    pub peer_id: NonZeroU64,
    pub term: u64,
    pub leader: Option<NonZeroU64>,
    pub applied_index: u64,
    pub writable: bool,
    pub auto_leave: bool,
    pub voters: Vec<u64>,
    pub outgoing_voters: Vec<u64>,
    pub learners: Vec<u64>,
    pub next_learners: Vec<u64>,
}

impl RouteObservation {
    /// An observation from `peer_id` with no term, leader or members yet.
    /// Assign the remaining fields; encoding checks the complete value.
    pub fn new(peer_id: NonZeroU64) -> Self {
        Self {
            peer_id,
            term: 0,
            leader: None,
            applied_index: 0,
            writable: false,
            auto_leave: false,
            voters: Vec::new(),
            outgoing_voters: Vec::new(),
            learners: Vec::new(),
            next_learners: Vec::new(),
        }
    }

    fn sets(&self) -> [&[u64]; 4] {
        [&self.voters, &self.outgoing_voters, &self.learners, &self.next_learners]
    }

    fn validate(&self) -> Result<(), FrameError> {
        if self.sets().iter().any(|set| {
            set.len() > MAX_MEMBERS || set.first() == Some(&0)
                || set.windows(2).any(|pair| pair[0] >= pair[1])
        }) || self.voters.is_empty()
            || (self.term == 0 && self.leader.is_some())
            || (self.writable && (self.leader != Some(self.peer_id)
                || !(self.voters.contains(&self.peer_id.get()) || self.outgoing_voters.contains(&self.peer_id.get()))))
        {
            return Err(FrameError::InvalidRouteObservation);
        }
        Ok(())
    }
}

pub fn encode_route_request(context: &RequestContext, maximum: usize) -> Result<Vec<u8>, FrameError> {
    let mut bytes = start_frame(PREFIX_BYTES, maximum)?;
    append_context(&mut bytes, context);
    finish_frame(bytes, ROUTE)
}

pub fn decode_route_request(bytes: &[u8], maximum: usize) -> Result<RequestContext, FrameError> {
    let (_, body) = verified_body(bytes, maximum, &[ROUTE])?;
    route_body(body)
}

pub(super) fn route_body(mut body: &[u8]) -> Result<RequestContext, FrameError> {
    let context = take_context(&mut body)?;
    if !body.is_empty() {
        return Err(FrameError::InvalidLength);
    }
    Ok(context)
}

pub fn encode_route_reply(
    context: &RequestContext, observation: &RouteObservation, maximum: usize,
) -> Result<Vec<u8>, FrameError> {
    observation.validate()?;
    let sets = observation.sets();
    let mut bytes = start_frame(MIN_REPLY_BYTES + 8 * sets.iter().map(|set| set.len()).sum::<usize>(), maximum)?;
    append_context(&mut bytes, context);
    for value in [observation.peer_id.get(), observation.term, observation.leader.map_or(0, NonZeroU64::get),
        observation.applied_index]
    {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes.extend([u8::from(observation.writable), u8::from(observation.auto_leave)]);
    for set in sets {
        bytes.push(set.len() as u8);
        for peer in set {
            bytes.extend_from_slice(&peer.to_le_bytes());
        }
    }
    finish_frame(bytes, ROUTE_REPLY)
}

pub fn decode_route_reply(bytes: &[u8], maximum: usize) -> Result<(RequestContext, RouteObservation), FrameError> {
    let (_, body) = verified_body(bytes, maximum, &[ROUTE_REPLY])?;
    route_reply_body(body)
}

pub(super) fn route_reply_body(mut body: &[u8]) -> Result<(RequestContext, RouteObservation), FrameError> {
    let context = take_context(&mut body)?;
    let peer_id = NonZeroU64::new(u64::from_le_bytes(take(&mut body)?)).ok_or(FrameError::InvalidRouteObservation)?;
    let term = u64::from_le_bytes(take(&mut body)?);
    let leader = NonZeroU64::new(u64::from_le_bytes(take(&mut body)?));
    let applied_index = u64::from_le_bytes(take(&mut body)?);
    let [writable, auto_leave] = take(&mut body)?;
    if writable > 1 || auto_leave > 1 {
        return Err(FrameError::InvalidRouteObservation);
    }
    let mut members = || {
        let [count] = take(&mut body)?;
        let count = usize::from(count);
        if count > MAX_MEMBERS || body.len() / 8 < count {
            return Err(FrameError::InvalidRouteObservation);
        }
        (0..count).map(|_| take(&mut body).map(u64::from_le_bytes)).collect()
    };
    let observation = RouteObservation {
        peer_id, term, leader, applied_index, writable: writable == 1, auto_leave: auto_leave == 1,
        voters: members()?, outgoing_voters: members()?, learners: members()?, next_learners: members()?,
    };
    observation.validate()?;
    // Later releases may append optional fields within the verified frame.
    Ok((context, observation))
}
