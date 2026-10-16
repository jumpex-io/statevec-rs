//! Loaded conformance only: unsupported readers are ABI refusals, whereas a
//! wrong-mode rebuild must reach the native owner's queued ordinary refusal.
use statevec_projector_api::plugin_v2::host::*;
use statevec_projector_api::{ProjectorCheckpointRejection, plugin_v2::{ffi::*, *}};
use std::time::{Duration, Instant};

fn failure(code: u32) -> ProjectorFailure { ProjectorFailure::Application { code } }

/// The caller lends a complete live host table through all calls and reply release.
pub(super) unsafe fn check_continuous_refusals(table: &SvProjectorHostV2) -> Result<u64, ProjectorFailure> {
    let (Some(info_fn), Some(next), Some(finish), Some(release_state), Some(offer), Some(poll), Some(release_reply)) =
        (table.state_info, table.state_next, table.finish_state, table.release_state,
         table.offer_rebuild, table.poll_reply, table.release_reply)
    else { return Err(failure(910)); };
    let unsupported = SvStatus::from(&ProjectorAbiFailure::UnsupportedCapabilities { required: 0x40 });
    let mut info = SvStateInfo { record_count: 1, ..SvStateInfo::default() };
    // 123 is deliberately not a state loan. Continuous-only mode refuses before
    // interpreting it, and must not consume it or leave stale output behind.
    let mut state_handle = 123;
    if unsafe { info_fn(table.context, state_handle, &mut info) } != unsupported || info != SvStateInfo::default() {
        return Err(failure(911));
    }
    let mut tag = u32::MAX;
    let mut record = SvStateRecord {
        key: SvKey { kind: 1, reserved16: 1, reserved32: 1, sys_id: 1 },
        revision: 1, record: SvBytes::borrowed(b"stale output"),
    };
    if unsafe { next(table.context, state_handle, &mut tag, &mut record) } != unsupported
        || tag != 0 || (record.key.kind, record.key.reserved16, record.key.reserved32, record.key.sys_id) != (0, 0, 0, 0)
        || record.revision != 0 || !record.record.ptr.is_null() || record.record.len != 0 {
        return Err(failure(912));
    }
    let mut finished = SvRebuildCut { bytes: [u8::MAX; 264] };
    if unsafe { finish(table.context, &mut state_handle, &mut finished) } != unsupported
        || state_handle != 123 || finished != SvRebuildCut::default() {
        return Err(failure(913));
    }
    if unsafe { release_state(table.context, &mut state_handle) } != unsupported || state_handle != 123 {
        return Err(failure(914));
    }

    // Untrusted zero bytes are not a native certificate. The mode decision must
    // precede authentication and arrive via the actual owner's reply channel.
    // The bound is a test deadline, never a product retry/completion policy.
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut handle = 0;
    loop {
        let status = unsafe { offer(table.context, &SvRebuildCut::default(), &mut tag, &mut handle) };
        if status != SvStatus::SUCCESS { return Err(failure(915)); }
        match (tag, handle) {
            (0, 1..) => break,
            (1, 0) if Instant::now() < deadline => std::thread::yield_now(),
            _ => return Err(failure(916)),
        }
    }
    let result = loop {
        let mut reply = SvControlReply::default();
        if unsafe { poll(table.context, handle, &mut reply) } != SvStatus::SUCCESS { break Err(failure(917)); }
        match reply.decode() {
            Ok(ControlReply::Rejected(ProjectorCheckpointRejection::WrongRecoveryMode)) => break Ok(()),
            Ok(ControlReply::Pending) if Instant::now() < deadline => std::thread::yield_now(),
            _ => break Err(failure(918)),
        }
    };
    // Release also on timeout, malformed reply or unexpected refusal. No early
    // Result return above this point abandons an acquired reply handle.
    if unsafe { release_reply(table.context, &mut handle) } != SvStatus::SUCCESS || handle != 0 {
        return Err(failure(919));
    }
    result?;
    Ok(5)
}
