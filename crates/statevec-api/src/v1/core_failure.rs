//! Closed Core call causes. These carry observations, not instance-recovery policy.
use super::{CoreAccessError, CoreSchemaError, CoreStatusDecodeError};
use crate::BusinessRejectCode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceError {
    Allocation { requested: usize },
    Os { code: i32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigureError {
    /// Application-defined configuration error code; never a business outcome.
    Invalid { code: u32 },
    UnsupportedCapabilities { required: u64 },
    Resource(ResourceError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreCreateError {
    Resource(ResourceError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostError {
    Resource(ResourceError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreAbiError {
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
    Status(CoreStatusDecodeError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoreExecutionError {
    Schema(CoreSchemaError),
    Access(CoreAccessError),
    Host(HostError),
    ReferenceTimeUnavailable,
    TxPositionUnavailable,
    PluginPanicked,
    HostPanicked,
    AbiContract(CoreAbiError),
}

impl CoreExecutionError {
    /// This observation must fence the accepted call even if author code
    /// catches it. Local storage/resource failures cannot become successful
    /// output. This is not an instance-unusability or retry decision.
    pub fn must_fail_call(&self) -> bool {
        matches!(
            self,
            Self::Access(CoreAccessError::Storage(_))
                | Self::Host(_)
                | Self::PluginPanicked
                | Self::HostPanicked
                | Self::AbiContract(_)
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoreTxError {
    Rejected(BusinessRejectCode),
    Execution(CoreExecutionError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoreError {
    Transaction(CoreTxError),
    Configuration(ConfigureError),
    Construction(CoreCreateError),
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
    InitializeCore,
    Migrate,
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
            Self::InitializeCore => 10,
            Self::Migrate => 11,
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
            10 => Self::InitializeCore,
            11 => Self::Migrate,
            _ => return None,
        })
    }

    pub(super) fn permits(self, tag: u32) -> bool {
        match self {
            Self::Configure => matches!(tag, 0 | 7 | 9 | 10),
            Self::Metadata => matches!(tag, 0 | 7 | 9),
            Self::CreateInstance => matches!(tag, 0 | 7 | 9 | 11),
            Self::PreflightBatch | Self::HostCallback => matches!(tag, 0 | 2..=9),
            Self::RunTx | Self::ValidateTxInvariants | Self::InitializeCore | Self::Migrate => matches!(tag, 0..=9),
            Self::DestroyInstance | Self::DestroyConfigured => matches!(tag, 0 | 7 | 9),
        }
    }
}

impl From<CoreAccessError> for CoreTxError {
    fn from(source: CoreAccessError) -> Self {
        Self::Execution(CoreExecutionError::Access(source))
    }
}

impl From<CoreExecutionError> for CoreTxError {
    fn from(source: CoreExecutionError) -> Self {
        Self::Execution(source)
    }
}

impl From<crate::ReferenceTimeUnavailable> for CoreTxError {
    fn from(_: crate::ReferenceTimeUnavailable) -> Self {
        Self::Execution(CoreExecutionError::ReferenceTimeUnavailable)
    }
}

impl From<crate::TxPositionUnavailable> for CoreTxError {
    fn from(_: crate::TxPositionUnavailable) -> Self {
        Self::Execution(CoreExecutionError::TxPositionUnavailable)
    }
}
