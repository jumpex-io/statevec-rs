//! Closed Core call causes. These carry observations, not instance-recovery policy.
use super::{AccessFailure, SchemaFailure, StatusDecodeFailure};
use crate::BusinessRejectCode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceFailure {
    Allocation { requested: usize },
    Os { code: i32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigureFailure {
    /// Application-defined configuration error code; never a business outcome.
    Invalid { code: u32 },
    UnsupportedCapabilities { required: u64 },
    Resource(ResourceFailure),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreateFailure {
    Resource(ResourceFailure),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostFailure {
    Resource(ResourceFailure),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbiFailure {
    NullPointer,
    Alignment { required: usize },
    Length { actual: u64, maximum: u64 },
    Version { expected: u32, actual: u32 },
    Kind { expected: u32, actual: u32 },
    TableSize { expected: u64, actual: u64 },
    UnsupportedCapabilities { required: u64 },
    Flag { actual: u32 },
    Reserved { actual: u64 },
    Utf8,
    EmbeddedNul,
    InvalidHandle,
    BorrowConflict,
    ReentrantCall,
    CallbackCount { expected: u64, actual: u64 },
    Status(StatusDecodeFailure),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecutionFailure {
    Schema(SchemaFailure),
    Access(AccessFailure),
    Host(HostFailure),
    ReferenceTimeUnavailable,
    TxPositionUnavailable,
    PluginPanicked,
    HostPanicked,
    AbiContract(AbiFailure),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TxFailure {
    Rejected(BusinessRejectCode),
    Execution(ExecutionFailure),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoreFailure {
    Transaction(TxFailure),
    Configuration(ConfigureFailure),
    Construction(CreateFailure),
}

/// The accepted call determines legal result families. No returned tag can
/// change its operation or establish that the host discarded speculation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreOperation {
    Configure,
    Metadata,
    CreateInstance,
    PreflightBatch,
    RunTx,
    ValidateTxInvariants,
    DestroyInstance,
    DestroyConfigured,
    HostCallback,
}

impl CoreOperation {
    pub(super) fn code(self) -> u32 {
        match self {
            Self::Configure => 1,
            Self::Metadata => 2,
            Self::CreateInstance => 3,
            Self::PreflightBatch => 4,
            Self::RunTx => 5,
            Self::ValidateTxInvariants => 6,
            Self::DestroyInstance => 7,
            Self::DestroyConfigured => 8,
            Self::HostCallback => 9,
        }
    }

    pub(super) fn from_code(raw: u32) -> Option<Self> {
        Some(match raw {
            1 => Self::Configure,
            2 => Self::Metadata,
            3 => Self::CreateInstance,
            4 => Self::PreflightBatch,
            5 => Self::RunTx,
            6 => Self::ValidateTxInvariants,
            7 => Self::DestroyInstance,
            8 => Self::DestroyConfigured,
            9 => Self::HostCallback,
            _ => return None,
        })
    }

    pub(super) fn permits(self, tag: u32) -> bool {
        match self {
            Self::Configure => matches!(tag, 0 | 7 | 9 | 10),
            Self::Metadata => matches!(tag, 0 | 7 | 9),
            Self::CreateInstance => matches!(tag, 0 | 7 | 9 | 11),
            Self::PreflightBatch | Self::HostCallback => matches!(tag, 0 | 2..=9),
            Self::RunTx | Self::ValidateTxInvariants => matches!(tag, 0..=9),
            Self::DestroyInstance | Self::DestroyConfigured => matches!(tag, 0 | 7 | 9),
        }
    }
}

impl From<AccessFailure> for TxFailure {
    fn from(source: AccessFailure) -> Self {
        Self::Execution(ExecutionFailure::Access(source))
    }
}

impl From<ExecutionFailure> for TxFailure {
    fn from(source: ExecutionFailure) -> Self {
        Self::Execution(source)
    }
}

impl From<crate::ReferenceTimeUnavailable> for TxFailure {
    fn from(_: crate::ReferenceTimeUnavailable) -> Self {
        Self::Execution(ExecutionFailure::ReferenceTimeUnavailable)
    }
}

impl From<crate::TxPositionUnavailable> for TxFailure {
    fn from(_: crate::TxPositionUnavailable) -> Self {
        Self::Execution(ExecutionFailure::TxPositionUnavailable)
    }
}
