//! Raw Core V5 C representations. Integer fields and optional function pointers
//! are deliberately unchecked here. A caller must validate the table/extent and
//! retain all pointees for the complete call; no Rust ownership crosses C.
//!
//! These tables alone do not supply a plugin exporter or transaction adapter.
use std::ffi::c_void;
use super::SvStatus;
use statevec_model::SchemaIdentity;

pub const CORE_ABI_MAJOR: u32 = 5;
pub const CORE_KIND: u32 = 1;
pub const CORE_MIGRATION_KIND: u32 = 6;
/// Required to distinguish raw create bytes from the unreleased visitor draft.
pub const CORE_RAW_CREATE_CAPABILITY: u64 = 1 << 6;
pub const CORE_LIFECYCLE_CAPABILITY: u64 = 1 << 7;
pub const CORE_BASE_CAPABILITIES: u64 = 0x1f | CORE_RAW_CREATE_CAPABILITY | CORE_LIFECYCLE_CAPABILITY;
pub const CORE_CANDIDATE_CAPABILITY: u64 = 1 << 5;
pub const CORE_CAPABILITIES: u64 = CORE_BASE_CAPABILITIES | CORE_CANDIDATE_CAPABILITY;
pub const CORE_ENTRY_SYMBOL: &std::ffi::CStr = c"statevec_core_plugin_entry_v5";

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SvBytes { pub ptr: *const u8, pub len: u64 }

impl SvBytes {
    pub const EMPTY: Self = Self { ptr: std::ptr::null(), len: 0 };
    /// The view must not escape the lifetime of `bytes`.
    pub fn borrowed(bytes: &[u8]) -> Self { Self { ptr: bytes.as_ptr(), len: bytes.len() as u64 } }
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SvMutBytes { pub ptr: *mut u8, pub len: u64 }

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SvSchemaIdentity {
    pub version_main: u8,
    pub version_minor: u8,
    pub types: [u8; 16],
    pub records: [u8; 16],
    pub commands: [u8; 16],
    pub events: [u8; 16],
}

impl From<SchemaIdentity> for SvSchemaIdentity {
    fn from(identity: SchemaIdentity) -> Self {
        Self {
            version_main: identity.schema_version.main(),
            version_minor: identity.schema_version.minor(),
            types: identity.types_schema_fingerprint,
            records: identity.record_schema_fingerprint,
            commands: identity.command_schema_fingerprint,
            events: identity.event_schema_fingerprint,
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SvHeader {
    pub abi_major: u32,
    pub kind: u32,
    pub table_bytes: u64,
    /// Interfaces implemented by this table's exporter, in its kind's namespace.
    pub provided_caps: u64,
    /// Interfaces the exporter needs from its counterpart. These may be zero;
    /// they do not substitute for the exporter's mandatory provided interfaces.
    pub required_caps: u64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SvDiscovery { pub table: *const c_void, pub table_bytes: u64 }

pub type SvCoreEntry = unsafe extern "C" fn(u64, *mut SvDiscovery) -> u32;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SvCommand {
    pub kind: u16,
    pub reserved16: u16,
    pub reserved32: u32,
    pub ext_seq: u64,
    pub ref_ext_time_us: u64,
    pub payload: SvBytes,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SvCoreMetadata {
    pub name: SvBytes,
    pub revision: SvBytes,
    pub schema_idl: SvBytes,
    pub tx_rules: u32,
    pub reserved: u32,
    pub schema: SvSchemaIdentity,
    pub reserved_schema: [u8; 6],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SvCoreV5 {
    pub h: SvHeader,
    pub build_target: SvBytes,
    pub configure: Option<unsafe extern "C" fn(SvBytes, *mut *mut c_void) -> SvStatus>,
    pub metadata: Option<unsafe extern "C" fn(*const c_void, *mut SvCoreMetadata) -> SvStatus>,
    pub create_instance: Option<unsafe extern "C" fn(*const c_void, *mut *mut c_void) -> SvStatus>,
    pub preflight_batch: Option<unsafe extern "C" fn(*const c_void, *const SvCommand, u64, *mut u64) -> SvStatus>,
    pub run_tx: Option<unsafe extern "C" fn(*const c_void, *const SvTxV5, *const SvCommand) -> SvStatus>,
    pub initialize_core: Option<unsafe extern "C" fn(*const c_void, *const SvTxV5, SvBytes) -> SvStatus>,
    pub migrate: Option<unsafe extern "C" fn(*const c_void, *const SvMigrationV5, *const [u8; 32], SvBytes) -> SvStatus>,
    pub validate_tx_invariants: Option<unsafe extern "C" fn(*const c_void, *const SvCandidateV5) -> SvStatus>,
    pub destroy_instance: Option<unsafe extern "C" fn(*mut *mut c_void) -> SvStatus>,
    pub destroy_configured: Option<unsafe extern "C" fn(*mut *mut c_void) -> SvStatus>,
}

/// Old records and a separate source-backed target record set. No native Rust object,
/// source-schema typed access or lifecycle/history writer crosses this table.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SvMigrationV5 {
    pub h: SvHeader,
    pub context: *const c_void,
    pub time_ns: Option<unsafe extern "C" fn(*const c_void, *mut u64) -> SvStatus>,
    pub tx_seq: Option<unsafe extern "C" fn(*const c_void, *mut u64) -> SvStatus>,
    pub source_read: Option<unsafe extern "C" fn(*const c_void, SvKey, SvReadVisitor, *mut c_void, *mut u32) -> SvStatus>,
    pub source_keys: Option<unsafe extern "C" fn(*const c_void, u16, SvKeyVisitor, *mut c_void, *mut SvWalk) -> SvStatus>,
    pub preserve_record: Option<unsafe extern "C" fn(*const c_void, SvKey, u16, SvBytes, *mut SvKey) -> SvStatus>,
    pub create_record: Option<unsafe extern "C" fn(*const c_void, u16, SvBytes, *mut SvKey) -> SvStatus>,
    pub emit_event: Option<unsafe extern "C" fn(*const c_void, u16, SvBytes) -> SvStatus>,
    pub delete_record: Option<unsafe extern "C" fn(*const c_void, SvKey) -> SvStatus>,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SvKey { pub kind: u16, pub reserved16: u16, pub reserved32: u32, pub sys_id: u64 }

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SvWalk { pub tag: u32, pub reserved: u32, pub count: u64 }

// C callers can pass NULL. Preserve it as a valid Rust value at ABI ingress;
// adapters must return CoreAbiError::NullPointer before borrowing or invoking it.
pub type SvReadVisitor = Option<unsafe extern "C" fn(*mut c_void, SvBytes) -> SvStatus>;
pub type SvWriteVisitor = Option<unsafe extern "C" fn(*mut c_void, SvKey, SvMutBytes) -> SvStatus>;
pub type SvKeyVisitor = Option<unsafe extern "C" fn(*mut c_void, SvKey, *mut u32) -> SvStatus>;

#[repr(C)]
pub struct SvTxV5 {
    pub h: SvHeader,
    pub context: *mut c_void,
    pub time_ns: Option<unsafe extern "C" fn(*const c_void, *mut u64) -> SvStatus>,
    pub tx_seq: Option<unsafe extern "C" fn(*const c_void, *mut u64) -> SvStatus>,
    pub read: Option<unsafe extern "C" fn(*const c_void, SvKey, SvReadVisitor, *mut c_void, *mut u32) -> SvStatus>,
    pub read_uk: Option<unsafe extern "C" fn(*const c_void, u16, u8, SvBytes, SvReadVisitor, *mut c_void, *mut u32) -> SvStatus>,
    pub resolve_uk: Option<unsafe extern "C" fn(*const c_void, u16, u8, SvBytes, *mut u32, *mut u64) -> SvStatus>,
    pub for_each_key: Option<unsafe extern "C" fn(*const c_void, u16, SvKeyVisitor, *mut c_void, *mut SvWalk) -> SvStatus>,
    pub count_prefix: Option<unsafe extern "C" fn(*const c_void, u16, u8, SvBytes, u64, *mut SvWalk) -> SvStatus>,
    pub create: Option<unsafe extern "C" fn(*mut c_void, u16, u32, u64, SvBytes, *mut SvKey) -> SvStatus>,
    pub update: Option<unsafe extern "C" fn(*mut c_void, SvKey, SvWriteVisitor, *mut c_void, *mut u32) -> SvStatus>,
    pub update_uk: Option<unsafe extern "C" fn(*mut c_void, u16, u8, SvBytes, SvWriteVisitor, *mut c_void, *mut u32) -> SvStatus>,
    pub delete_record: Option<unsafe extern "C" fn(*mut c_void, SvKey, *mut u32) -> SvStatus>,
    pub delete_uk: Option<unsafe extern "C" fn(*mut c_void, u16, u8, SvBytes, *mut u32) -> SvStatus>,
    pub emit_event: Option<unsafe extern "C" fn(*mut c_void, u16, SvBytes) -> SvStatus>,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SvCandidateChange {
    pub key: SvKey,
    pub operation: u32,
    pub has_before: u32,
    pub has_after: u32,
    pub reserved: u32,
    pub before: SvBytes,
    pub after: SvBytes,
}

pub type SvChangeVisitor = Option<unsafe extern "C" fn(*mut c_void, *const SvCandidateChange) -> SvStatus>;

#[repr(C)]
pub struct SvCandidateV5 {
    pub h: SvHeader,
    pub context: *const c_void,
    pub time_ns: Option<unsafe extern "C" fn(*const c_void, *mut u64) -> SvStatus>,
    pub read: Option<unsafe extern "C" fn(*const c_void, u32, SvKey, SvReadVisitor, *mut c_void, *mut u32) -> SvStatus>,
    pub resolve_uk: Option<unsafe extern "C" fn(*const c_void, u32, u16, u8, SvBytes, *mut u32, *mut u64) -> SvStatus>,
    pub changes: Option<unsafe extern "C" fn(*const c_void, u32, SvChangeVisitor, *mut c_void, *mut u64) -> SvStatus>,
}
