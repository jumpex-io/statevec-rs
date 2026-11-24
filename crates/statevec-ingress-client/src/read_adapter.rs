// Stateless checked construction for the read owner; no retry policy.
use crate::client::ClientError;

/// A read refusal with its observation: the least frame every read session
/// must receive.
const SMALLEST_READ_REPLY_BYTES: usize = ingress_api::stream_batch::HEADER_BYTES
    + ingress_api::stream_batch::CONTEXT_BYTES
    + ingress_api::stream_batch::READ_OBSERVATION_BYTES
    + 1 // outcome
    + 1 // refusal tag
    + 8; // refusal argument

pub(super) fn validate_read_construction(
    identity: crate::DeploymentIdentity,
    endpoints: &[(u64, std::net::SocketAddr)],
    maximum_frame_bytes: u32,
    retry_millis: u64,
    operation_millis: u64,
) -> Result<(), ClientError> {
    if [identity.cluster_identity(), identity.genesis_identity(), identity.execution_lineage()]
        .iter()
        .any(|digest| digest.bytes() == [0; 32])
    {
        return Err(ClientError::InvalidIdentity);
    }
    crate::client::validate_endpoints(endpoints)?;
    if (maximum_frame_bytes as usize) < SMALLEST_READ_REPLY_BYTES
        || !(crate::client::BATCH_CLIENT_RETRY_MIN_MS..=crate::client::BATCH_CLIENT_RETRY_MAX_MS)
            .contains(&retry_millis)
        || !(crate::client::BATCH_CLIENT_OPERATION_MIN_MS..=crate::client::BATCH_CLIENT_OPERATION_MAX_MS)
            .contains(&operation_millis)
    {
        return Err(ClientError::InvalidLimit);
    }
    Ok(())
}
