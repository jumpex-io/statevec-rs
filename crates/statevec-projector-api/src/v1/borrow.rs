use super::{ffi::*, ProjectorAbiError as A, ProjectorError as F, ProjectorOperation as Op, ProjectorCallError, decode_call};

pub(super) fn pointer<T>(raw: *const T) -> Result<(), A> {
    if raw.is_null() { return Err(A::NullPointer); }
    if !raw.addr().is_multiple_of(std::mem::align_of::<T>()) { return Err(A::Alignment { required: std::mem::align_of::<T>() }); }
    Ok(())
}

/// Exact visible evidence binding only; the native owner authenticates the MAC.
pub(super) fn cut_binds(b: &[u8; 264], magic: &[u8; 8], p: SvPosition) -> bool {
    &b[..8] == magic && b[40..72] == p.cluster && b[72..104] == p.genesis && b[104..136] == p.profile
        && b[136..140] == p.manifest.to_le_bytes() && b[140..144] == [0; 4]
        && b[144..152] == p.raft_index.to_le_bytes() && b[152..160] == p.raft_term.to_le_bytes()
        && b[160..192] == p.applied_batch_identity && b[192..200] == p.applied_tx_seq.to_le_bytes()
        && b[200..232] == p.tx_chain_hash
}

pub(super) unsafe fn bytes<'a>(raw: SvBytes) -> Result<&'a [u8], A> {
    if raw.len > isize::MAX as u64 || raw.ptr.addr().checked_add(raw.len as usize).is_none() {
        return Err(A::Length { actual: raw.len, maximum: isize::MAX as u64 });
    }
    if raw.len == 0 { return Ok(&[]); }
    pointer(raw.ptr)?;
    // SAFETY: callers require the host to supply an initialized, readable span
    // for 'a. They bound 'a by the input call, held entry, or current snapshot
    // read, as appropriate; pointer/length checks alone do not prove liveness.
    Ok(unsafe { std::slice::from_raw_parts(raw.ptr, raw.len as usize) })
}

pub(super) fn call(status: SvStatus, op: Op) -> Result<(), F> {
    match decode_call(status, op)? {
        None => Ok(()), Some(ProjectorCallError::Operation(cause)) => Err(cause),
        Some(_) => Err(A::InvalidHandle.into()),
    }
}

/// Exact table size for a header: the query table iff it provides queries.
pub(super) fn host_table_bytes(h: &SvHeader) -> u64 {
    if h.provided_caps & ENGINE_QUERY_CAPABILITY != 0 { std::mem::size_of::<SvProjectorHostV2Query>() as u64 }
    else { std::mem::size_of::<SvProjectorHostV2>() as u64 }
}

pub(super) fn validate_host(table: &SvProjectorHostV2, query: Option<&SvEngineQueryHostV1>) -> Result<(), A> {
    let h = table.h;
    if h.abi_major != PROJECTOR_ABI_MAJOR { return Err(A::Version { expected: PROJECTOR_ABI_MAJOR, actual: h.abi_major }); }
    if h.kind != PROJECTOR_HOST_KIND { return Err(A::Kind { expected: PROJECTOR_HOST_KIND, actual: h.kind }); }
    let expected = host_table_bytes(&h);
    if h.table_bytes != expected || query.is_some() != (h.provided_caps & ENGINE_QUERY_CAPABILITY != 0) {
        return Err(A::TableSize { expected, actual: h.table_bytes });
    }
    if (h.provided_caps & !PROJECTOR_HOST_CAPABILITIES) | (h.required_caps & !PROJECTOR_CAPABILITIES) != 0
        || h.provided_caps & h.required_caps != h.required_caps
        || ![PROJECTOR_BASE_CAPABILITIES | CONTINUOUS_HISTORY_CAPABILITY,
              PROJECTOR_BASE_CAPABILITIES | LATEST_STATE_CAPABILITY].contains(&h.required_caps) {
        return Err(A::UnsupportedCapabilities { required: h.required_caps });
    }
    if table.context.is_null() { return Err(A::NullPointer); }
    if table.receive.is_none() || table.stop_receive.is_none() || table.frame_info.is_none()
        || table.transaction.is_none() || table.event.is_none() || table.change.is_none()
        || table.release_frame.is_none() || table.report_durable_offset.is_none() || table.offer_status.is_none()
        || table.poll_reply.is_none() || table.release_reply.is_none() || table.stopping.is_none()
        || table.notify.is_none() || table.state_info.is_none() || table.state_next.is_none()
        || table.finish_state.is_none() || table.release_state.is_none() || table.offer_rebuild.is_none() { return Err(A::NullPointer); }
    if let Some(query) = query
        && (query.offer_query.is_none() || query.poll_query.is_none() || query.query_record.is_none() || query.release_query.is_none()) {
        return Err(A::NullPointer);
    }
    Ok(())
}

pub(super) fn frame_position(value: SvPosition) -> Result<crate::SourcePosition, A> {
    if value.raft_index == 0 { return Err(A::InvalidPosition); }
    position_allowing_genesis(value)
}
pub(super) fn position_allowing_genesis(value: SvPosition) -> Result<crate::SourcePosition, A> {
    if value.reserved != 0 { return Err(A::Reserved { actual: value.reserved.into() }); }
    if value.manifest != 1 { return Err(A::Flag { actual: value.manifest }); }
    if (value.raft_index == 0) != (value.raft_term == 0)
        || value.raft_index == 0 && (value.applied_batch_identity != [0; 32] || value.applied_tx_seq != 0) {
        return Err(A::InvalidPosition);
    }
    Ok(crate::SourcePosition {
        cluster_identity: crate::Digest32::new(value.cluster), genesis_identity: crate::Digest32::new(value.genesis),
        execution_profile_identity: crate::Digest32::new(value.profile), manifest: crate::ProjectionManifest::AllRegisteredSchemaV1,
        raft_index: value.raft_index, raft_term: value.raft_term, applied_batch_identity: crate::AppliedBatchIdentity(value.applied_batch_identity),
        frontier: crate::AppliedTxProgress { applied_tx_seq: value.applied_tx_seq, tx_chain_hash: value.tx_chain_hash },
    })
}
