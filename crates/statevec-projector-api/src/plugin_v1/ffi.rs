//! Fixed-width V1 C layouts. Integers and nullable callbacks are untrusted at ingress.
//! A table or copied cut does not prove commitment or borrower retirement.
use std::ffi::c_void;
pub use statevec_api::plugin_v3::ffi::{SvBytes, SvDiscovery, SvHeader, SvKey, SvSchemaIdentity};
pub use super::SvStatus;

pub const PROJECTOR_ABI_MAJOR: u32 = 1;
pub const PROJECTOR_KIND: u32 = 2;
pub const PROJECTOR_HOST_KIND: u32 = 5;
pub const PROJECTOR_BASE_CAPABILITIES: u64 = 0x1f;
pub const CONTINUOUS_HISTORY_CAPABILITY: u64 = 0x20;
pub const LATEST_STATE_CAPABILITY: u64 = 0x40;
pub const PROJECTOR_CAPABILITIES: u64 = 0x7f;
pub const PROJECTOR_ENTRY_SYMBOL: &std::ffi::CStr = c"statevec_projector_plugin_entry_v1";
pub type SvProjectorEntry = unsafe extern "C" fn(u64, *mut SvDiscovery) -> u32;

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SvPosition {
    pub cluster: [u8; 32], pub genesis: [u8; 32], pub profile: [u8; 32],
    pub manifest: u32, pub reserved: u32,
    pub raft_index: u64, pub raft_term: u64,
    pub applied_batch_identity: [u8; 32], pub applied_tx_seq: u64, pub tx_chain_hash: [u8; 32],
}
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SvCut { pub bytes: [u8; 264] }
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SvRebuildCut { pub bytes: [u8; 264] }
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SvProjectorId { pub len: u64, pub bytes: [u8; 128] }
impl Default for SvCut { fn default() -> Self { Self { bytes: [0; 264] } } }
impl Default for SvRebuildCut { fn default() -> Self { Self { bytes: [0; 264] } } }
impl Default for SvProjectorId { fn default() -> Self { Self { len: 0, bytes: [0; 128] } } }

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SvConsumeOffset {
    pub cluster: [u8; 32], pub projector_id: SvProjectorId, pub raft_term: u64, pub raft_index: u64,
}
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SvOffsetCheckpoint { pub cut: SvCut }
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SvRecovery { pub tag: u32, pub reserved: u32, pub offset: SvConsumeOffset }
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SvProjectionIdentity {
    pub cluster: [u8; 32], pub genesis: [u8; 32], pub profile: [u8; 32],
    pub manifest: u32, pub reserved: u32,
}
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SvProjectorPrepare {
    pub identity: SvProjectionIdentity, pub peer_id: u64, pub schema_idl: SvBytes,
    pub learner_only: u32, pub external_retention: u32,
    pub checkpoint_interval_indices: u64, pub min_selected_duration_ms: u64,
    pub recovery_mode: u32, pub reserved: u32, pub projector_id: SvProjectorId,
}
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SvDeliveryLimits { pub frame_capacity: u64, pub maximum_accounted_bytes: u64 }
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SvProjectorStart {
    pub identity: SvProjectionIdentity, pub peer_id: u64, pub projector_id: SvProjectorId,
    pub raft_index: u64, pub raft_term: u64, pub applied_tx_seq: u64, pub tx_chain_hash: [u8; 32],
    pub recovery_mode: u32, pub initialization: u32, pub rebuild_target: u64,
}
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SvStateInfo { pub position: SvPosition, pub record_count: u64 }
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SvStateRecord { pub key: SvKey, pub revision: u64, pub record: SvBytes }
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SvFrameInfo {
    pub position: SvPosition, pub cut: SvCut, pub peer_id: u64, pub transaction_count: u64,
    pub context: u32, pub reserved: u32,
}
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SvTransactionInfo {
    pub tx_seq: u64, pub ordinal: u32, pub reserved: u32,
    pub tx_id: [u8; 32], pub tx_chain_hash: [u8; 32],
    pub result_digest_profile: u8, pub reserved_profile: [u8; 3],
    pub sys_result_code: u16, pub biz_code: u16, pub result_digest: SvBytes,
    pub schema: SvSchemaIdentity, pub reserved_schema: [u8; 6],
    pub ref_tx_time_ns: u64, pub event_count: u64, pub change_count: u64,
}
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SvEvent { pub event_seq: u32, pub kind: u16, pub reserved: u16, pub payload: SvBytes }
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SvRecordChange { pub key: SvKey, pub revision: u64, pub operation: u32, pub reserved: u32, pub after: SvBytes }
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SvControlReply { pub tag: u32, pub cause: u32, pub value0: u64, pub value1: u64 }
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SvProjectorFailureObservation { pub slot: u32, pub reserved: u32, pub cause: SvStatus }
impl SvProjectorFailureObservation {
    pub const NONE: Self = Self { slot: 0, reserved: 0, cause: SvStatus::SUCCESS };
}
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SvProjectorCompletion {
    pub joined: u32, pub reserved: u32,
    pub run_failure: SvStatus, pub service_join_failure: SvStatus,
    pub notification_failure: SvStatus, pub worker_join_failure: SvStatus,
    pub frame_release_failure: SvStatus, pub state_release_failure: SvStatus, pub reply_release_failure: SvStatus,
    pub failed_service_joins: u64,
}
impl SvProjectorCompletion {
    pub const EMPTY: Self = Self {
        joined: 0, reserved: 0, run_failure: SvStatus::SUCCESS, service_join_failure: SvStatus::SUCCESS,
        notification_failure: SvStatus::SUCCESS, worker_join_failure: SvStatus::SUCCESS,
        frame_release_failure: SvStatus::SUCCESS, state_release_failure: SvStatus::SUCCESS,
        reply_release_failure: SvStatus::SUCCESS, failed_service_joins: 0,
    };
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SvProjectorHostV1 {
    pub h: SvHeader, pub context: *mut c_void,
    pub receive: Option<unsafe extern "C" fn(*mut c_void, u64, *mut u32, *mut u64) -> SvStatus>,
    pub stop_receive: Option<unsafe extern "C" fn(*mut c_void) -> SvStatus>,
    pub frame_info: Option<unsafe extern "C" fn(*mut c_void, u64, *mut SvFrameInfo) -> SvStatus>,
    pub transaction: Option<unsafe extern "C" fn(*mut c_void, u64, u64, *mut SvTransactionInfo) -> SvStatus>,
    pub event: Option<unsafe extern "C" fn(*mut c_void, u64, u64, u64, *mut SvEvent) -> SvStatus>,
    pub change: Option<unsafe extern "C" fn(*mut c_void, u64, u64, u64, *mut SvRecordChange) -> SvStatus>,
    pub release_frame: Option<unsafe extern "C" fn(*mut c_void, *mut u64) -> SvStatus>,
    pub offer_checkpoint: Option<unsafe extern "C" fn(*mut c_void, *const SvOffsetCheckpoint, *mut u32, *mut u64) -> SvStatus>,
    pub offer_status: Option<unsafe extern "C" fn(*mut c_void, *mut u32, *mut u64) -> SvStatus>,
    pub poll_reply: Option<unsafe extern "C" fn(*mut c_void, u64, *mut SvControlReply) -> SvStatus>,
    pub release_reply: Option<unsafe extern "C" fn(*mut c_void, *mut u64) -> SvStatus>,
    pub stopping: Option<unsafe extern "C" fn(*const c_void, *mut u32) -> SvStatus>,
    pub notify: Option<unsafe extern "C" fn(*mut c_void, u32, *const SvProjectorFailureObservation) -> SvStatus>,
    pub state_info: Option<unsafe extern "C" fn(*mut c_void, u64, *mut SvStateInfo) -> SvStatus>,
    pub state_next: Option<unsafe extern "C" fn(*mut c_void, u64, *mut u32, *mut SvStateRecord) -> SvStatus>,
    pub finish_state: Option<unsafe extern "C" fn(*mut c_void, *mut u64, *mut SvRebuildCut) -> SvStatus>,
    pub release_state: Option<unsafe extern "C" fn(*mut c_void, *mut u64) -> SvStatus>,
    pub offer_rebuild: Option<unsafe extern "C" fn(*mut c_void, *const SvRebuildCut, *mut u32, *mut u64) -> SvStatus>,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SvProjectorV1 {
    pub h: SvHeader, pub build_target: SvBytes,
    pub configure: Option<unsafe extern "C" fn(SvBytes, SvBytes, *mut *mut c_void) -> SvStatus>,
    pub prepare: Option<unsafe extern "C" fn(*const c_void, *const SvProjectorPrepare, *mut *mut c_void, *mut SvRecovery, *mut SvDeliveryLimits) -> SvStatus>,
    pub start: Option<unsafe extern "C" fn(*mut *mut c_void, *const SvProjectorHostV1, *const SvProjectorStart, *mut *mut c_void) -> SvStatus>,
    pub request_stop: Option<unsafe extern "C" fn(*mut c_void) -> SvStatus>,
    pub poll_worker: Option<unsafe extern "C" fn(*const c_void, *mut u32, *mut SvProjectorFailureObservation) -> SvStatus>,
    pub join: Option<unsafe extern "C" fn(*mut *mut c_void, *mut SvProjectorCompletion) -> SvStatus>,
    pub destroy_prepared: Option<unsafe extern "C" fn(*mut *mut c_void) -> SvStatus>,
    pub destroy_configured: Option<unsafe extern "C" fn(*mut *mut c_void) -> SvStatus>,
}
