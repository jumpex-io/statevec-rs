use super::{ffi::*, ProjectorAbiFailure as A, ProjectorFailure as F, ProjectorOperation as Op, ProjectorCallFailure, decode_call};

pub(super) fn pointer<T>(raw: *const T) -> Result<(), A> {
    if raw.is_null() { return Err(A::NullPointer); }
    if !raw.addr().is_multiple_of(std::mem::align_of::<T>()) { return Err(A::Alignment { required: std::mem::align_of::<T>() }); }
    Ok(())
}

pub(super) unsafe fn bytes<'a>(raw: SvBytes) -> Result<&'a [u8], A> {
    if raw.len > isize::MAX as u64 || raw.ptr.addr().checked_add(raw.len as usize).is_none() {
        return Err(A::Length { actual: raw.len, maximum: isize::MAX as u64 });
    }
    if raw.len == 0 { return Ok(&[]); }
    pointer(raw.ptr)?;
    // SAFETY: the checked host supplies readable spans until the owning frame
    // is released; callers tie this borrow to that loan, never to the table.
    Ok(unsafe { std::slice::from_raw_parts(raw.ptr, raw.len as usize) })
}

pub(super) fn call(status: SvStatus, op: Op) -> Result<(), F> {
    match decode_call(status, op)? {
        None => Ok(()), Some(ProjectorCallFailure::Failure(cause)) => Err(cause),
        Some(_) => Err(A::InvalidHandle.into()),
    }
}

pub(super) fn validate_host(table: &SvProjectorHostV1) -> Result<(), A> {
    let h = table.h;
    if h.abi_major != 1 { return Err(A::Version { expected: 1, actual: h.abi_major }); }
    if h.kind != PROJECTOR_HOST_KIND { return Err(A::Kind { expected: PROJECTOR_HOST_KIND, actual: h.kind }); }
    if h.table_bytes != std::mem::size_of::<SvProjectorHostV1>() as u64 { return Err(A::TableSize { expected: std::mem::size_of::<SvProjectorHostV1>() as u64, actual: h.table_bytes }); }
    if (h.provided_caps | h.required_caps) & !PROJECTOR_CAPABILITIES != 0
        || h.provided_caps & (PROJECTOR_BASE_CAPABILITIES | CONTINUOUS_HISTORY_CAPABILITY) != PROJECTOR_BASE_CAPABILITIES | CONTINUOUS_HISTORY_CAPABILITY
        || h.required_caps != PROJECTOR_BASE_CAPABILITIES | CONTINUOUS_HISTORY_CAPABILITY {
        return Err(A::UnsupportedCapabilities { required: h.required_caps });
    }
    if table.context.is_null() { return Err(A::NullPointer); }
    if table.receive.is_none() || table.stop_receive.is_none() || table.frame_info.is_none()
        || table.transaction.is_none() || table.event.is_none() || table.change.is_none()
        || table.release_frame.is_none() || table.offer_checkpoint.is_none() || table.offer_status.is_none()
        || table.poll_reply.is_none() || table.release_reply.is_none() || table.stopping.is_none()
        || table.notify.is_none() || table.state_info.is_none() || table.state_next.is_none()
        || table.finish_state.is_none() || table.release_state.is_none() || table.offer_rebuild.is_none() { return Err(A::NullPointer); }
    Ok(())
}

pub(super) fn position(value: SvPosition) -> Result<crate::ProjectionPosition, A> {
    if value.reserved != 0 { return Err(A::Reserved { actual: value.reserved.into() }); }
    if value.manifest != 1 { return Err(A::Flag { actual: value.manifest }); }
    if value.raft_index == 0 || value.raft_term == 0 { return Err(A::InvalidHandle); }
    Ok(crate::ProjectionPosition {
        cluster_identity: crate::Digest32::new(value.cluster), genesis_identity: crate::Digest32::new(value.genesis),
        execution_profile_identity: crate::Digest32::new(value.profile), manifest: crate::ProjectionManifest::AllRegisteredSchemaV1,
        raft_index: value.raft_index, raft_term: value.raft_term, applied_batch_identity: crate::ProjectionAppliedIdentity(value.applied_batch_identity),
        frontier: crate::ProjectionFrontier { applied_tx_seq: value.applied_tx_seq, tx_chain_hash: value.tx_chain_hash },
    })
}
