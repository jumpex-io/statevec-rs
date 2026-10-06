// Stateless checked construction and bounded frame sizing; no retry policy.
use super::ClientFailure;
use statevec_frame::InputRef;

pub(super) fn validate_batch_construction(
    identity: (statevec_frame::Digest32, statevec_frame::Digest32, statevec_frame::Digest32),
    stream: &InputRef,
    endpoints: &[(u64, std::net::SocketAddr)],
    maximum_commands: u32,
    maximum_frame_bytes: u32,
    maximum_outcome_bytes: u32,
    retry_millis: u64,
    operation_millis: u64,
    lifetime_millis: u64,
    unavailable_reply_limit: u32,
) -> Result<(), ClientFailure> {
    if [identity.0, identity.1, identity.2]
        .iter()
        .any(|digest| digest.bytes() == [0; 32])
    {
        return Err(ClientFailure::InvalidIdentity);
    }
    if stream.client_id == 0 || stream.stream_id == 0 || stream.client_seq == 0 || stream.request_id.is_some() {
        return Err(ClientFailure::InvalidStream);
    }
    if endpoints.is_empty()
        || endpoints.len() > super::BATCH_CLIENT_MAX_ENDPOINTS
        || endpoints.iter().enumerate().any(|(i, (peer, address))| {
            *peer == 0
                || address.port() == 0
                || address.ip().is_unspecified()
                || endpoints[..i].iter().any(|(prior, _)| prior == peer)
        })
    {
        return Err(ClientFailure::InvalidEndpoints);
    }
    let smallest_request = ingress_api::stream_batch::HEADER_BYTES
        + ingress_api::stream_batch::CONTEXT_BYTES
        + statevec_frame::CLIENT_STREAM_BATCH_HEADER_BYTES
        + 4;
    // The receive decoder uses the same frame cap as request encoding. Check
    // the largest configured Completed as well as the fixed exact-history
    // reply before any batch can acquire custody. Match the service's minimum
    // complete outcome budget; it also covers an Unavailable member's u64.
    let completed_reply =
        statevec_frame::stream_batch_protocol::completed_reply_byte_bound(maximum_commands, maximum_outcome_bytes)
            .ok_or(ClientFailure::InvalidLimit)?;
    let exact_reply = statevec_frame::stream_batch_protocol::REPLY_BASE_BYTES
        + statevec_frame::applied_binding::APPLIED_STREAM_BATCH_BINDING_BYTES
        - statevec_frame::CLIENT_STREAM_BATCH_COMMITMENT_BYTES;
    let reply_bound = completed_reply.max(exact_reply as u64);
    if maximum_commands == 0
        || maximum_outcome_bytes < statevec_frame::outcome::CommitOutcome::FIXED_ENCODED_BYTES as u32
        || (maximum_frame_bytes as usize) < smallest_request
        || reply_bound > u64::from(maximum_frame_bytes)
        || !(super::BATCH_CLIENT_RETRY_MIN_MS..=super::BATCH_CLIENT_RETRY_MAX_MS).contains(&retry_millis)
        || !(super::BATCH_CLIENT_OPERATION_MIN_MS..=super::BATCH_CLIENT_OPERATION_MAX_MS).contains(&operation_millis)
        || !(super::BATCH_CLIENT_LIFETIME_MIN_MS..=super::BATCH_CLIENT_LIFETIME_MAX_MS).contains(&lifetime_millis)
        || operation_millis > lifetime_millis
        || retry_millis >= lifetime_millis
        || !(1..=super::BATCH_CLIENT_MAX_UNAVAILABLE_REPLIES).contains(&unavailable_reply_limit)
    {
        return Err(ClientFailure::InvalidLimit);
    }
    Ok(())
}

pub(super) fn batch_frame_bytes(
    batch: &statevec_frame::ClientStreamBatch,
    maximum_commands: u32,
    maximum_frame_bytes: u32,
) -> Result<usize, ClientFailure> {
    let envelope = ingress_api::stream_batch::HEADER_BYTES + ingress_api::stream_batch::CONTEXT_BYTES;
    let maximum_intent = (maximum_frame_bytes as usize)
        .checked_sub(envelope)
        .ok_or(ClientFailure::InvalidLimit)?;
    batch
        .encoded_len_v1(maximum_intent, maximum_commands as usize)
        .map(|length| length + envelope)
        .map_err(|source| ClientFailure::TooLarge { source })
}

pub(super) fn next_batch_request_id(current: std::num::NonZeroU64) -> Result<std::num::NonZeroU64, ClientFailure> {
    current
        .get()
        .checked_add(1)
        .and_then(std::num::NonZeroU64::new)
        .ok_or(ClientFailure::RequestIdExhausted)
}

pub(super) fn next_batch_connection_id(current: u64) -> Result<u64, ClientFailure> {
    current.checked_add(1).ok_or(ClientFailure::ConnectionIdExhausted)
}
