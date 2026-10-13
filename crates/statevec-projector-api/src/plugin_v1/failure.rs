use super::{ConfigureFailure, HostFailure, ResourceFailure};

macro_rules! scalar_enum {
    ($name:ident { $($variant:ident = $code:literal),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        #[non_exhaustive]
        #[repr(u32)]
        pub enum $name { $($variant = $code),+ }
        impl $name {
            pub const fn code(self) -> u32 { self as u32 }
            pub const fn from_code(raw: u32) -> Option<Self> {
                match raw { $($code => Some(Self::$variant),)+ _ => None }
            }
        }
    };
}

scalar_enum!(ProjectorOperation {
    Configure=1, Prepare=2, Start=3, RequestStop=4, PollWorker=5, Join=6,
    DestroyPrepared=7, DestroyConfigured=8, Receive=9, StopReceive=10,
    FrameInfo=11, Transaction=12, Event=13, Change=14, ReleaseFrame=15,
    OfferCheckpoint=16, OfferStatus=17, PollReply=18, ReleaseReply=19,
    Stopping=20, Notify=21, StateInfo=22, StateNext=23, FinishState=24,
    ReleaseState=25, OfferRebuild=26,
});
scalar_enum!(ProjectorIoStage {
    StoreOpen=1, StoreRead=2, StoreWrite=3, StoreSync=4, OffsetCheckpoint=5,
    DirectorySync=6, Service=7, WorkerSpawn=8, ServiceSpawn=9, StateRead=10,
});
scalar_enum!(ProjectorIoKind {
    Other=0, NotFound=1, PermissionDenied=2, AlreadyExists=3, InvalidInput=4,
    InvalidData=5, TimedOut=6, WouldBlock=7, Interrupted=8, UnexpectedEof=9,
    WriteZero=10, OutOfMemory=11, StorageFull=12, Unsupported=13, BrokenPipe=14, Connection=15,
});
scalar_enum!(RebuildSourceFailure {
    Unavailable=1, InvalidRecord=2, Incomplete=3, EvidenceMismatch=4, RebuildAbandoned=5,
});

impl ProjectorIoKind {
    pub fn from_io(kind: std::io::ErrorKind) -> Self {
        use std::io::ErrorKind as K;
        match kind {
            K::NotFound => Self::NotFound, K::PermissionDenied => Self::PermissionDenied,
            K::AlreadyExists => Self::AlreadyExists, K::InvalidInput => Self::InvalidInput,
            K::InvalidData => Self::InvalidData, K::TimedOut => Self::TimedOut,
            K::WouldBlock => Self::WouldBlock, K::Interrupted => Self::Interrupted,
            K::UnexpectedEof => Self::UnexpectedEof, K::WriteZero => Self::WriteZero,
            K::OutOfMemory => Self::OutOfMemory, K::StorageFull => Self::StorageFull,
            K::Unsupported => Self::Unsupported, K::BrokenPipe => Self::BrokenPipe,
            K::ConnectionAborted | K::ConnectionRefused | K::ConnectionReset | K::NotConnected => Self::Connection,
            _ => Self::Other,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ProjectorFailure {
    Application { code: u32 },
    Io { stage: ProjectorIoStage, kind: ProjectorIoKind, os_code: Option<i32> },
    Construction(ResourceFailure),
    PluginPanicked,
    HostPanicked,
    Host(HostFailure),
    AbiContract(ProjectorAbiFailure),
    RebuildSource(RebuildSourceFailure),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ProjectorWorkerFailure {
    Run(ProjectorFailure),
    FrameRelease(ProjectorFailure),
    StateRelease(ProjectorFailure),
    ReplyRelease(ProjectorFailure),
    ServiceJoinPanicked,
    Notification(ProjectorFailure),
    WorkerJoinPanicked,
}

/// Complete decoded wire vocabulary, prior to invocation/slot validation.
/// An arbitrary variant is not a legal return from every operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ProjectorCallFailure {
    Configuration(ConfigureFailure),
    Failure(ProjectorFailure),
    WorkerJoinPanicked,
    ServiceJoinPanicked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ProjectorAbiFailure {
    NullPointer, Alignment { required: usize }, Length { actual: u64, maximum: u64 },
    Version { expected: u32, actual: u32 }, Kind { expected: u32, actual: u32 },
    TableSize { expected: u64, actual: u64 }, UnsupportedCapabilities { required: u64 },
    Flag { actual: u32 }, Reserved { actual: u64 }, Utf8, EmbeddedNul,
    InvalidHandle, BorrowConflict, ReentrantCall, CallbackCount { expected: u64, actual: u64 },
    ControlCharacter { scalar: u32 },
    Status(ProjectorStatusDecodeFailure),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ProjectorStatusDecodeFailure {
    UnknownTag { tag: u32 },
    NonzeroSuccessCause { cause: u32 },
    IllegalReturn {
        operation: ProjectorOperation, tag: u32, cause: u32,
        stage: Option<ProjectorIoStage>, rebuild_reason: Option<RebuildSourceFailure>,
    },
    UnknownOperation { raw: u32 },
    UnexpectedTag { expected: u32, actual: u32 },
    UnknownCause { tag: u32, cause: u32 },
    DetailOutOfRange { slot: u8, value: u64, maximum: u64 },
    NonzeroUnusedDetail { slot: u8, value: u64 },
    NoncanonicalNestedFailure { cause: u32 },
}
impl std::fmt::Display for ProjectorFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "projector: {self:?}") }
}
impl std::error::Error for ProjectorFailure {}
impl From<ProjectorAbiFailure> for ProjectorFailure {
    fn from(source: ProjectorAbiFailure) -> Self { Self::AbiContract(source) }
}
impl From<ProjectorStatusDecodeFailure> for ProjectorFailure {
    fn from(source: ProjectorStatusDecodeFailure) -> Self { Self::AbiContract(ProjectorAbiFailure::Status(source)) }
}

impl ProjectorOperation {
    /// The one direct/nested return predicate. Inputs must already be structurally
    /// decoded, except known Core-only tags whose payload is deliberately opaque.
    pub(super) fn permits(self, tag: u32, cause: u32, subcategory: u32) -> bool {
        if tag == 0 || tag == 9 { return true; }
        match self {
            Self::Configure => matches!(tag, 7 | 10),
            Self::Prepare => matches!(tag, 7 | 11) || tag == 12 && (cause == 1 || cause == 2 && (1..=6).contains(&subcategory)),
            Self::Start => matches!(tag, 7 | 11) || tag == 12 && cause == 2 && subcategory == 8,
            Self::RequestStop | Self::PollWorker | Self::Join | Self::DestroyPrepared | Self::DestroyConfigured => tag == 7,
            _ => {
                matches!(tag, 4 | 8) || tag == 12 && match cause {
                    2 => matches!(self, Self::StateNext | Self::FinishState) && subcategory == 10,
                    5 => match self {
                        Self::StateInfo => matches!(subcategory, 1 | 4),
                        Self::StateNext => matches!(subcategory, 1 | 2 | 4),
                        Self::FinishState => matches!(subcategory, 1 | 3 | 4),
                        Self::ReleaseState => subcategory == 5,
                        _ => false,
                    },
                    _ => false,
                }
            }
        }
    }
}
