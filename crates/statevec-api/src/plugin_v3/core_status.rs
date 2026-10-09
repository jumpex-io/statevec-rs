use super::{
    AbiFailure, ConfigureFailure, CoreFailure, CoreOperation, CreateFailure,
    ExecutionFailure, HostFailure, ResourceFailure, StatusDecodeFailure, SvStatus, TxFailure,
};

impl SvStatus {
    /// Decode the entire status, then check its meaning against the operation
    /// actually invoked. This does not determine instance reusability.
    pub fn decode_for(self, operation: CoreOperation) -> Result<Option<CoreFailure>, StatusDecodeFailure> {
        let failure = match self.tag {
            0 => {
                if self.cause != 0 {
                    return Err(StatusDecodeFailure::NonzeroSuccessCause { cause: self.cause });
                }
                self.zero_from(0)?;
                return Ok(None);
            }
            1 => CoreFailure::Transaction(TxFailure::Rejected(self.decode_business_rejection()?)),
            2 => execution(ExecutionFailure::Schema(self.decode_schema()?)),
            3 => execution(ExecutionFailure::Access(self.decode_access()?)),
            4 => execution(ExecutionFailure::Host(HostFailure::Resource(self.decode_resource()?))),
            5..=8 => {
                if self.cause != 0 { return Err(self.unknown_cause()); }
                self.zero_from(0)?;
                execution(match self.tag {
                    5 => ExecutionFailure::ReferenceTimeUnavailable,
                    6 => ExecutionFailure::TxPositionUnavailable,
                    7 => ExecutionFailure::PluginPanicked,
                    8 => ExecutionFailure::HostPanicked,
                    _ => unreachable!(),
                })
            }
            9 => execution(ExecutionFailure::AbiContract(self.decode_abi()?)),
            10 => {
                self.zero_from(1)?;
                CoreFailure::Configuration(match self.cause {
                    1 => ConfigureFailure::Invalid { code: self.u32(0)? },
                    2 => ConfigureFailure::UnsupportedCapabilities { required: self.detail[0] },
                    3 => ConfigureFailure::Resource(ResourceFailure::Allocation { requested: self.usize(0)? }),
                    4 => ConfigureFailure::Resource(ResourceFailure::Os { code: self.u32(0)? as i32 }),
                    _ => return Err(self.unknown_cause()),
                })
            }
            11 => CoreFailure::Construction(CreateFailure::Resource(self.decode_resource()?)),
            tag => return Err(StatusDecodeFailure::UnknownTag { tag }),
        };
        if !operation.permits(self.tag) {
            return Err(StatusDecodeFailure::IllegalReturn { operation, tag: self.tag });
        }
        Ok(Some(failure))
    }

    fn decode_resource(self) -> Result<ResourceFailure, StatusDecodeFailure> {
        self.zero_from(1)?;
        match self.cause {
            1 => Ok(ResourceFailure::Allocation { requested: self.usize(0)? }),
            2 => Ok(ResourceFailure::Os { code: self.u32(0)? as i32 }),
            _ => Err(self.unknown_cause()),
        }
    }

    fn decode_abi(self) -> Result<AbiFailure, StatusDecodeFailure> {
        let used = match self.cause {
            1 | 10..=14 => 0,
            2 | 7..=9 | 100 | 101 | 103 | 106 | 107 => 1,
            3..=6 | 15 | 102 | 104 | 105 | 109 => 2,
            108 => 3,
            _ => return Err(self.unknown_cause()),
        };
        self.zero_from(used)?;
        Ok(match self.cause {
            1 => AbiFailure::NullPointer,
            2 => AbiFailure::Alignment { required: self.usize(0)? },
            3 => AbiFailure::Length { actual: self.detail[0], maximum: self.detail[1] },
            4 => AbiFailure::Version { expected: self.u32(0)?, actual: self.u32(1)? },
            5 => AbiFailure::Kind { expected: self.u32(0)?, actual: self.u32(1)? },
            6 => AbiFailure::TableSize { expected: self.detail[0], actual: self.detail[1] },
            7 => AbiFailure::UnsupportedCapabilities { required: self.detail[0] },
            8 => AbiFailure::Flag { actual: self.u32(0)? },
            9 => AbiFailure::Reserved { actual: self.detail[0] },
            10 => AbiFailure::Utf8,
            11 => AbiFailure::EmbeddedNul,
            12 => AbiFailure::InvalidHandle,
            13 => AbiFailure::BorrowConflict,
            14 => AbiFailure::ReentrantCall,
            15 => AbiFailure::CallbackCount { expected: self.detail[0], actual: self.detail[1] },
            100 => AbiFailure::Status(StatusDecodeFailure::UnknownTag { tag: self.u32(0)? }),
            101 => AbiFailure::Status(StatusDecodeFailure::NonzeroSuccessCause { cause: self.u32(0)? }),
            102 => {
                let raw = self.u32(0)?;
                let operation = CoreOperation::from_code(raw).ok_or(StatusDecodeFailure::UnknownOperation { raw })?;
                AbiFailure::Status(StatusDecodeFailure::IllegalReturn { operation, tag: self.u32(1)? })
            }
            103 => AbiFailure::Status(StatusDecodeFailure::UnknownOperation { raw: self.u32(0)? }),
            104 => AbiFailure::Status(StatusDecodeFailure::UnexpectedTag { expected: self.u32(0)?, actual: self.u32(1)? }),
            105 => AbiFailure::Status(StatusDecodeFailure::UnknownCause { tag: self.u32(0)?, cause: self.u32(1)? }),
            106 => AbiFailure::Status(StatusDecodeFailure::UnknownStorageCause { raw: self.detail[0] }),
            107 => AbiFailure::Status(StatusDecodeFailure::InvalidBusinessCode { raw: self.u32(0)? }),
            108 => AbiFailure::Status(StatusDecodeFailure::DetailOutOfRange { slot: self.u8(0)?, value: self.detail[1], maximum: self.detail[2] }),
            109 => AbiFailure::Status(StatusDecodeFailure::NonzeroUnusedDetail { slot: self.u8(0)?, value: self.detail[1] }),
            _ => unreachable!(),
        })
    }

    fn zero_from(self, first: u8) -> Result<(), StatusDecodeFailure> {
        for slot in first..3 { self.require_zero(slot)?; }
        Ok(())
    }
}

fn execution(failure: ExecutionFailure) -> CoreFailure {
    CoreFailure::Transaction(TxFailure::Execution(failure))
}

fn resource_status(tag: u32, source: ResourceFailure) -> SvStatus {
    let (cause, value) = match source {
        ResourceFailure::Allocation { requested } => (1, requested as u64),
        ResourceFailure::Os { code } => (2, u64::from(code as u32)),
    };
    SvStatus { tag, cause, detail: [value, 0, 0] }
}

impl From<&CoreFailure> for SvStatus {
    fn from(source: &CoreFailure) -> Self {
        match source {
            CoreFailure::Transaction(source) => source.into(),
            CoreFailure::Configuration(source) => source.into(),
            CoreFailure::Construction(CreateFailure::Resource(source)) => resource_status(11, *source),
        }
    }
}

impl From<&ConfigureFailure> for SvStatus {
    fn from(source: &ConfigureFailure) -> Self {
        let (cause, value) = match *source {
            ConfigureFailure::Invalid { code } => (1, u64::from(code)),
            ConfigureFailure::UnsupportedCapabilities { required } => (2, required),
            ConfigureFailure::Resource(ResourceFailure::Allocation { requested }) => (3, requested as u64),
            ConfigureFailure::Resource(ResourceFailure::Os { code }) => (4, u64::from(code as u32)),
        };
        Self { tag: 10, cause, detail: [value, 0, 0] }
    }
}

impl From<&TxFailure> for SvStatus {
    fn from(source: &TxFailure) -> Self {
        match source {
            TxFailure::Rejected(code) => Self::business_rejection(*code),
            TxFailure::Execution(source) => source.into(),
        }
    }
}

impl From<&ExecutionFailure> for SvStatus {
    fn from(source: &ExecutionFailure) -> Self {
        let tag = match source {
            ExecutionFailure::Schema(source) => return source.into(),
            ExecutionFailure::Access(source) => return source.into(),
            ExecutionFailure::Host(HostFailure::Resource(source)) => return resource_status(4, *source),
            ExecutionFailure::ReferenceTimeUnavailable => 5,
            ExecutionFailure::TxPositionUnavailable => 6,
            ExecutionFailure::PluginPanicked => 7,
            ExecutionFailure::HostPanicked => 8,
            ExecutionFailure::AbiContract(source) => return source.into(),
        };
        Self { tag, cause: 0, detail: [0; 3] }
    }
}

impl From<&AbiFailure> for SvStatus {
    fn from(source: &AbiFailure) -> Self {
        let (cause, detail) = match *source {
            AbiFailure::NullPointer => (1, [0; 3]),
            AbiFailure::Alignment { required } => (2, [required as u64, 0, 0]),
            AbiFailure::Length { actual, maximum } => (3, [actual, maximum, 0]),
            AbiFailure::Version { expected, actual } => (4, [expected.into(), actual.into(), 0]),
            AbiFailure::Kind { expected, actual } => (5, [expected.into(), actual.into(), 0]),
            AbiFailure::TableSize { expected, actual } => (6, [expected, actual, 0]),
            AbiFailure::UnsupportedCapabilities { required } => (7, [required, 0, 0]),
            AbiFailure::Flag { actual } => (8, [actual.into(), 0, 0]),
            AbiFailure::Reserved { actual } => (9, [actual, 0, 0]),
            AbiFailure::Utf8 => (10, [0; 3]),
            AbiFailure::EmbeddedNul => (11, [0; 3]),
            AbiFailure::InvalidHandle => (12, [0; 3]),
            AbiFailure::BorrowConflict => (13, [0; 3]),
            AbiFailure::ReentrantCall => (14, [0; 3]),
            AbiFailure::CallbackCount { expected, actual } => (15, [expected, actual, 0]),
            AbiFailure::Status(source) => match source {
                StatusDecodeFailure::UnknownTag { tag } => (100, [tag.into(), 0, 0]),
                StatusDecodeFailure::NonzeroSuccessCause { cause } => (101, [cause.into(), 0, 0]),
                StatusDecodeFailure::IllegalReturn { operation, tag } => (102, [operation.code().into(), tag.into(), 0]),
                StatusDecodeFailure::UnknownOperation { raw } => (103, [raw.into(), 0, 0]),
                StatusDecodeFailure::UnexpectedTag { expected, actual } => (104, [expected.into(), actual.into(), 0]),
                StatusDecodeFailure::UnknownCause { tag, cause } => (105, [tag.into(), cause.into(), 0]),
                StatusDecodeFailure::UnknownStorageCause { raw } => (106, [raw, 0, 0]),
                StatusDecodeFailure::InvalidBusinessCode { raw } => (107, [raw.into(), 0, 0]),
                StatusDecodeFailure::DetailOutOfRange { slot, value, maximum } => (108, [slot.into(), value, maximum]),
                StatusDecodeFailure::NonzeroUnusedDetail { slot, value } => (109, [slot.into(), value, 0]),
            },
        };
        Self { tag: 9, cause, detail }
    }
}
