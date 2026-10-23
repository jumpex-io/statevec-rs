//! Fixed-width V2 C layouts. Integers and nullable callbacks are untrusted at ingress.
//! A table or copied cut does not prove commitment or borrower retirement.
use std::ffi::c_void;
pub use statevec_api::v1::ffi::{SvBytes, SvDiscovery, SvHeader, SvKey, SvSchemaIdentity};
pub use super::SvStatus;
pub use super::wire::{receive_tag, offer_tag, state_tag, reply_tag, status_kind, rejection_reason, recovery_tag, recovery_mode,
    query_key_tag, query_reply_tag, query_rejection};

pub const PROJECTOR_ABI_MAJOR: u32 = 2;
pub const PROJECTOR_KIND: u32 = 2;
pub const PROJECTOR_HOST_KIND: u32 = 5;
pub const PROJECTOR_BASE_CAPABILITIES: u64 = 0x1f;
pub const CONTINUOUS_HISTORY_CAPABILITY: u64 = 0x20;
pub const LATEST_STATE_CAPABILITY: u64 = 0x40;
pub const PROJECTOR_CAPABILITIES: u64 = 0x7f;
/// Optional host-table capability: the table is `SvProjectorHostV2Query`.
/// A host provides it only with that exact table size; it is never required.
pub const ENGINE_QUERY_CAPABILITY: u64 = 0x80;
pub const PROJECTOR_HOST_CAPABILITIES: u64 = PROJECTOR_CAPABILITIES | ENGINE_QUERY_CAPABILITY;
pub const MAX_ENGINE_QUERY_KEYS: u64 = 4096;
pub const MAX_ENGINE_QUERY_KEY_BYTES: u64 = 1024;
pub const PROJECTOR_ENTRY_SYMBOL: &std::ffi::CStr = c"statevec_projector_plugin_entry_v2";
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
pub struct SvDurableOffsetReport { pub cut: SvCut }
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
    pub checkpoint_interval_indices: u64, pub min_capture_residence_ms: u64,
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
pub struct SvRecordChange {
    pub key: SvKey, pub revision: u64, pub operation: u32, pub reserved: u32,
    pub before: SvBytes, pub after: SvBytes,
}
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
pub struct SvProjectorHostV2 {
    pub h: SvHeader, pub context: *mut c_void,
    pub receive: Option<unsafe extern "C" fn(*mut c_void, u64, *mut u32, *mut u64) -> SvStatus>,
    pub stop_receive: Option<unsafe extern "C" fn(*mut c_void) -> SvStatus>,
    pub frame_info: Option<unsafe extern "C" fn(*mut c_void, u64, *mut SvFrameInfo) -> SvStatus>,
    pub transaction: Option<unsafe extern "C" fn(*mut c_void, u64, u64, *mut SvTransactionInfo) -> SvStatus>,
    pub event: Option<unsafe extern "C" fn(*mut c_void, u64, u64, u64, *mut SvEvent) -> SvStatus>,
    pub change: Option<unsafe extern "C" fn(*mut c_void, u64, u64, u64, *mut SvRecordChange) -> SvStatus>,
    pub release_frame: Option<unsafe extern "C" fn(*mut c_void, *mut u64) -> SvStatus>,
    pub report_durable_offset: Option<unsafe extern "C" fn(*mut c_void, *const SvDurableOffsetReport, *mut u32, *mut u64) -> SvStatus>,
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
pub struct SvProjectorV2 {
    pub h: SvHeader, pub build_target: SvBytes,
    pub configure: Option<unsafe extern "C" fn(SvBytes, SvBytes, *mut *mut c_void) -> SvStatus>,
    pub prepare: Option<unsafe extern "C" fn(*const c_void, *const SvProjectorPrepare, *mut *mut c_void, *mut SvRecovery, *mut SvDeliveryLimits) -> SvStatus>,
    pub start: Option<unsafe extern "C" fn(*mut *mut c_void, *const SvProjectorHostV2, *const SvProjectorStart, *mut *mut c_void) -> SvStatus>,
    pub request_stop: Option<unsafe extern "C" fn(*mut c_void) -> SvStatus>,
    pub poll_worker: Option<unsafe extern "C" fn(*const c_void, *mut u32, *mut SvProjectorFailureObservation) -> SvStatus>,
    pub join: Option<unsafe extern "C" fn(*mut *mut c_void, *mut SvProjectorCompletion) -> SvStatus>,
    pub destroy_prepared: Option<unsafe extern "C" fn(*mut *mut c_void) -> SvStatus>,
    pub destroy_configured: Option<unsafe extern "C" fn(*mut *mut c_void) -> SvStatus>,
}

/// One lookup. `UNIQUE_KEY` uses kind, uk_id and the encoded key bytes;
/// `SYS_ID` uses kind and sys_id with uk_id 0 and an empty key.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SvEngineQueryKey { pub tag: u32, pub kind: u16, pub uk_id: u8, pub reserved: u8, pub sys_id: u64, pub key: SvBytes }
/// Borrowed only for the offer call; the host copies every key it accepts.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SvEngineQuery { pub keys: *const SvEngineQueryKey, pub count: u64 }
/// Position fields are nonzero only for `READY`, `cause` only for `REJECTED`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SvEngineQueryInfo {
    pub tag: u32, pub cause: u32, pub raft_index: u64, pub raft_term: u64,
    pub applied_tx_seq: u64, pub tx_chain_hash: [u8; 32], pub count: u64,
}
/// `record` stays readable until the query handle is released.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SvEngineQueryRecord { pub found: u32, pub reserved: u32, pub key: SvKey, pub revision: u64, pub record: SvBytes }
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SvEngineQueryHostV1 {
    pub offer_query: Option<unsafe extern "C" fn(*mut c_void, *const SvEngineQuery, *mut u32, *mut u64) -> SvStatus>,
    pub poll_query: Option<unsafe extern "C" fn(*mut c_void, u64, *mut SvEngineQueryInfo) -> SvStatus>,
    pub query_record: Option<unsafe extern "C" fn(*mut c_void, u64, u64, *mut SvEngineQueryRecord) -> SvStatus>,
    pub release_query: Option<unsafe extern "C" fn(*mut c_void, *mut u64) -> SvStatus>,
}
/// The V2 host table followed by the engine query callbacks. `base.h`
/// describes this whole table and provides `ENGINE_QUERY_CAPABILITY`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SvProjectorHostV2Query { pub base: SvProjectorHostV2, pub query: SvEngineQueryHostV1 }
