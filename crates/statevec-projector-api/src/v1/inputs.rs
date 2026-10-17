//! Call-borrowed DTO validation. Native attachment still decides recovery.
use super::{plugin::*, borrow, ffi::*, ProjectorAbiError as A};
use crate::{Digest32, AppliedTxProgress, ProjectionManifest, StartInstruction, HistoryMode as Mode, SupportedHistoryModes};
use std::num::NonZeroU64;

pub(super) const MAX_CONFIG_BYTES: u64 = 1024 * 1024;
pub(super) const MAX_IDL_BYTES: u64 = 16 * 1024 * 1024;
pub(super) const MAX_PATH_BYTES: u64 = 4096;

pub(super) const fn support(value: SupportedHistoryModes) -> u64 {
    match value {
        SupportedHistoryModes::Continuous => CONTINUOUS_HISTORY_CAPABILITY,
        SupportedHistoryModes::LatestState => LATEST_STATE_CAPABILITY,
        SupportedHistoryModes::Both => CONTINUOUS_HISTORY_CAPABILITY | LATEST_STATE_CAPABILITY,
    }
}
pub(super) fn mode(raw: u32) -> Result<Mode, A> {
    match raw { recovery_mode::CONTINUOUS_HISTORY => Ok(Mode::Continuous), recovery_mode::LATEST_STATE_ONLY => Ok(Mode::LatestState), actual => Err(A::Flag { actual }) }
}
pub(super) fn identity(raw: SvProjectionIdentity) -> Result<SourceIdentity, A> {
    if raw.reserved != 0 { return Err(A::Reserved { actual: raw.reserved.into() }); }
    if raw.manifest != 1 { return Err(A::Flag { actual: raw.manifest }); }
    Ok(SourceIdentity {
        cluster_identity: Digest32::new(raw.cluster), genesis_identity: Digest32::new(raw.genesis),
        execution_profile_identity: Digest32::new(raw.profile), manifest: ProjectionManifest::AllRegisteredSchemaV1,
    })
}
pub(super) unsafe fn text<'a>(raw: SvBytes, maximum: u64) -> Result<&'a str, A> {
    if raw.len > maximum { return Err(A::Length { actual: raw.len, maximum }); }
    // SAFETY: the caller lends this span for the synchronous invocation only.
    let bytes = unsafe { borrow::bytes(raw)? };
    let text = std::str::from_utf8(bytes).map_err(|_| A::Utf8)?;
    if text.contains('\0') { return Err(A::EmbeddedNul); }
    Ok(text)
}
pub(super) unsafe fn base_dir<'a>(raw: SvBytes) -> Result<&'a std::path::Path, A> {
    if raw.len > MAX_PATH_BYTES { return Err(A::Length { actual: raw.len, maximum: MAX_PATH_BYTES }); }
    let bytes = unsafe { borrow::bytes(raw)? };
    if bytes.contains(&0) { return Err(A::EmbeddedNul); }
    #[cfg(unix)]
    let path = {
        use std::os::unix::ffi::OsStrExt;
        std::path::Path::new(std::ffi::OsStr::from_bytes(bytes))
    };
    #[cfg(not(unix))]
    let path = std::path::Path::new(std::str::from_utf8(bytes).map_err(|_| A::Utf8)?);
    if !path.is_absolute() { return Err(A::RelativeBaseDir); }
    Ok(path)
}
pub(super) fn cluster_id(bytes: [u8; 32]) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(64);
    for byte in bytes { write!(&mut out, "{byte:02x}").expect("String formatting"); }
    out
}
pub(super) unsafe fn prepare<'a>(raw: &'a SvProjectorPrepare, cluster_id: &'a str, caps: u64) -> Result<ProjectorPrepare<'a>, A> {
    if raw.reserved != 0 { return Err(A::Reserved { actual: raw.reserved.into() }); }
    let mode = mode(raw.recovery_mode)?;
    let required = match mode { Mode::Continuous => CONTINUOUS_HISTORY_CAPABILITY, Mode::LatestState => LATEST_STATE_CAPABILITY };
    if caps & required == 0 { return Err(A::UnsupportedCapabilities { required }); }
    for actual in [raw.learner_only, raw.external_retention] {
        if actual != 1 { return Err(A::Flag { actual }); }
    }
    if raw.peer_id == 0 { return Err(A::ZeroPeer); }
    let checkpoint_policy = match (NonZeroU64::new(raw.checkpoint_interval_indices), NonZeroU64::new(raw.min_capture_residence_ms)) {
        (None, None) => None,
        (Some(interval_indices), Some(min_capture_residence_ms)) => Some(EngineCheckpointPolicy { interval_indices, min_capture_residence_ms }),
        _ => return Err(A::PartialCheckpointPolicy),
    };
    Ok(ProjectorPrepare {
        identity: identity(raw.identity)?, peer_id: raw.peer_id, cluster_id,
        projector_id: raw.projector_id.decode()?,
        schema_idl: unsafe { text(raw.schema_idl, MAX_IDL_BYTES)? }.as_bytes(),
        checkpoint_policy, history_mode: mode,
    })
}
pub(super) fn startup(raw: &SvProjectorStart) -> Result<(StartupPosition, StartInstruction), A> {
    raw.projector_id.decode()?;
    if raw.peer_id == 0 { return Err(A::ZeroPeer); }
    if (raw.raft_index == 0) != (raw.raft_term == 0) { return Err(A::InvalidPosition); }
    let initialization = match (mode(raw.recovery_mode)?, raw.initialization, raw.rebuild_target) {
        (Mode::Continuous, 0, 0) => StartInstruction::DeliverEntries,
        (Mode::LatestState, 1, target) if target >= raw.raft_index => StartInstruction::Rebuild { target },
        _ => return Err(A::BindingMismatch),
    };
    Ok((StartupPosition {
        identity: identity(raw.identity)?, peer_id: raw.peer_id, raft_index: raw.raft_index, raft_term: raw.raft_term,
        frontier: AppliedTxProgress { applied_tx_seq: raw.applied_tx_seq, tx_chain_hash: raw.tx_chain_hash },
    }, initialization))
}
