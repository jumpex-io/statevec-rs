use super::{
    CoreAbiError, ConfigureError, CoreError, CoreOperation, CoreCreateError,
    CoreExecutionError, HostError, ResourceError, CoreStatusDecodeError, SvStatus, CoreTxError,
};

impl SvStatus {
    /// Decode the entire status, then check its meaning against the operation
    /// actually invoked. This does not determine instance reusability.
    pub fn decode_for(self, operation: CoreOperation) -> Result<Option<CoreError>, CoreStatusDecodeError> {
        let failure = self.decode_fields()?;
        if let Some(CoreError::Transaction(CoreTxError::Execution(
            CoreExecutionError::AbiContract(CoreAbiError::Status(nested)),
        ))) = failure {
            self.check_nested_status(nested)?;
        }
        if failure.is_some() && !operation.permits(self.tag) {
            return Err(CoreStatusDecodeError::IllegalReturn { operation, tag: self.tag });
        }
        Ok(failure)
    }

    // Decode representation only. Nested diagnostics consult this same cause
    // decoder without recursively validating a diagnostic about a diagnostic.
    fn decode_fields(self) -> Result<Option<CoreError>, CoreStatusDecodeError> {
        let failure = match self.tag {
            0 => {
                if self.cause != 0 {
                    return Err(CoreStatusDecodeError::NonzeroSuccessCause { cause: self.cause });
                }
                self.zero_from(0)?;
                return Ok(None);
            }
            1 => CoreError::Transaction(CoreTxError::Rejected(self.decode_business_rejection()?)),
            2 => execution(CoreExecutionError::Schema(self.decode_schema()?)),
            3 => execution(CoreExecutionError::Access(self.decode_access()?)),
            4 => execution(CoreExecutionError::Host(HostError::Resource(self.decode_resource()?))),
            5..=8 => {
                if self.cause != 0 { return Err(self.unknown_cause()); }
                self.zero_from(0)?;
                execution(match self.tag {
                    5 => CoreExecutionError::ReferenceTimeUnavailable,
                    6 => CoreExecutionError::TxPositionUnavailable,
                    7 => CoreExecutionError::PluginPanicked,
                    8 => CoreExecutionError::HostPanicked,
                    _ => unreachable!(),
                })
            }
            9 => execution(CoreExecutionError::AbiContract(self.decode_abi()?)),
            10 => {
                self.zero_from(1)?;
                CoreError::Configuration(match self.cause {
                    1 => ConfigureError::Invalid { code: self.u32(0)? },
                    2 => ConfigureError::UnsupportedCapabilities { required: self.detail[0] },
                    3 => ConfigureError::Resource(ResourceError::Allocation { requested: self.usize(0)? }),
                    4 => ConfigureError::Resource(ResourceError::Os { code: self.u32(0)? as i32 }),
                    _ => return Err(self.unknown_cause()),
                })
            }
            11 => CoreError::Construction(CoreCreateError::Resource(self.decode_resource()?)),
            tag => return Err(CoreStatusDecodeError::UnknownTag { tag }),
        };
        Ok(Some(failure))
    }

    fn decode_resource(self) -> Result<ResourceError, CoreStatusDecodeError> {
        self.zero_from(1)?;
        match self.cause {
            1 => Ok(ResourceError::Allocation { requested: self.usize(0)? }),
            2 => Ok(ResourceError::Os { code: self.u32(0)? as i32 }),
            _ => Err(self.unknown_cause()),
        }
    }

    fn decode_abi(self) -> Result<CoreAbiError, CoreStatusDecodeError> {
        let used = match self.cause {
            1 | 10..=14 => 0,
            2 | 7..=9 | 100 | 101 | 103 | 106 | 107 | 110 => 1,
            3..=6 | 15 | 102 | 104 | 105 | 109 => 2,
            108 => 3,
            _ => return Err(self.unknown_cause()),
        };
        self.zero_from(used)?;
        let failure = match self.cause {
            1 => CoreAbiError::NullPointer,
            2 => CoreAbiError::Alignment { required: self.usize(0)? },
            3 => CoreAbiError::Length { actual: self.detail[0], maximum: self.detail[1] },
            4 => CoreAbiError::Version { expected: self.u32(0)?, actual: self.u32(1)? },
            5 => CoreAbiError::Kind { expected: self.u32(0)?, actual: self.u32(1)? },
            6 => CoreAbiError::TableSize { expected: self.detail[0], actual: self.detail[1] },
            7 => CoreAbiError::UnsupportedCapabilities { required: self.detail[0] },
            8 => CoreAbiError::Flag { actual: self.u32(0)? },
            9 => CoreAbiError::Reserved { actual: self.detail[0] },
            10 => CoreAbiError::Utf8,
            11 => CoreAbiError::EmbeddedNul,
            12 => CoreAbiError::InvalidHandle,
            13 => CoreAbiError::BorrowConflict,
            14 => CoreAbiError::ReentrantCall,
            15 => CoreAbiError::CallbackCount { expected: self.detail[0], actual: self.detail[1] },
            100 => CoreAbiError::Status(CoreStatusDecodeError::UnknownTag { tag: self.u32(0)? }),
            101 => CoreAbiError::Status(CoreStatusDecodeError::NonzeroSuccessCause { cause: self.u32(0)? }),
            102 => {
                let raw = self.u32(0)?;
                let operation = CoreOperation::from_code(raw).ok_or(CoreStatusDecodeError::UnknownOperation { raw })?;
                CoreAbiError::Status(CoreStatusDecodeError::IllegalReturn { operation, tag: self.u32(1)? })
            }
            103 => CoreAbiError::Status(CoreStatusDecodeError::UnknownOperation { raw: self.u32(0)? }),
            104 => CoreAbiError::Status(CoreStatusDecodeError::UnexpectedTag { expected: self.u32(0)?, actual: self.u32(1)? }),
            105 => CoreAbiError::Status(CoreStatusDecodeError::UnknownCause { tag: self.u32(0)?, cause: self.u32(1)? }),
            106 => CoreAbiError::Status(CoreStatusDecodeError::UnknownStorageCause { raw: self.detail[0] }),
            107 => CoreAbiError::Status(CoreStatusDecodeError::InvalidBusinessCode { raw: self.u32(0)? }),
            108 => CoreAbiError::Status(CoreStatusDecodeError::DetailOutOfRange { slot: self.u8(0)?, value: self.detail[1], maximum: self.detail[2] }),
            109 => CoreAbiError::Status(CoreStatusDecodeError::NonzeroUnusedDetail { slot: self.u8(0)?, value: self.detail[1] }),
            110 => CoreAbiError::Status(CoreStatusDecodeError::NoncanonicalNestedFailure { cause: self.u32(0)? }),
            _ => unreachable!(),
        };
        Ok(failure)
    }

    fn check_nested_status(self, nested: CoreStatusDecodeError) -> Result<(), CoreStatusDecodeError> {
        let canonical = match nested {
            CoreStatusDecodeError::UnknownTag { tag } => tag > 11,
            CoreStatusDecodeError::NonzeroSuccessCause { cause } => cause != 0,
            CoreStatusDecodeError::IllegalReturn { operation, tag } => tag <= 11 && !operation.permits(tag),
            CoreStatusDecodeError::UnknownOperation { raw } => CoreOperation::from_code(raw).is_none(),
            CoreStatusDecodeError::UnexpectedTag { expected, actual } => expected != actual,
            CoreStatusDecodeError::UnknownCause { tag, cause } => {
                let raw = Self { tag, cause, detail: [0; 3] };
                raw.decode_fields() == Err(CoreStatusDecodeError::UnknownCause { tag, cause })
            }
            CoreStatusDecodeError::UnknownStorageCause { raw } => !(1..=12).contains(&raw),
            CoreStatusDecodeError::InvalidBusinessCode { raw } => !(1..60000).contains(&raw),
            CoreStatusDecodeError::DetailOutOfRange { slot, value, maximum } => slot < 3 && value > maximum,
            CoreStatusDecodeError::NonzeroUnusedDetail { slot, value } => slot < 3 && value != 0,
            // Includes 110 itself: rejecting a malformed 110 record must
            // still produce a canonical, round-trippable failure report.
            CoreStatusDecodeError::NoncanonicalNestedFailure { cause } => (100..=110).contains(&cause),
        };
        if !canonical {
            return Err(CoreStatusDecodeError::NoncanonicalNestedFailure { cause: self.cause });
        }
        Ok(())
    }

    fn zero_from(self, first: u8) -> Result<(), CoreStatusDecodeError> {
        for slot in first..3 { self.require_zero(slot)?; }
        Ok(())
    }
}

fn execution(failure: CoreExecutionError) -> CoreError {
    CoreError::Transaction(CoreTxError::Execution(failure))
}

fn resource_status(tag: u32, source: ResourceError) -> SvStatus {
    let (cause, value) = match source {
        ResourceError::Allocation { requested } => (1, requested as u64),
        ResourceError::Os { code } => (2, u64::from(code as u32)),
    };
    SvStatus { tag, cause, detail: [value, 0, 0] }
}

impl From<&CoreError> for SvStatus {
    fn from(source: &CoreError) -> Self {
        match source {
            CoreError::Transaction(source) => source.into(),
            CoreError::Configuration(source) => source.into(),
            CoreError::Construction(CoreCreateError::Resource(source)) => resource_status(11, *source),
        }
    }
}

impl From<&ConfigureError> for SvStatus {
    fn from(source: &ConfigureError) -> Self {
        let (cause, value) = match *source {
            ConfigureError::Invalid { code } => (1, u64::from(code)),
            ConfigureError::UnsupportedCapabilities { required } => (2, required),
            ConfigureError::Resource(ResourceError::Allocation { requested }) => (3, requested as u64),
            ConfigureError::Resource(ResourceError::Os { code }) => (4, u64::from(code as u32)),
        };
        Self { tag: 10, cause, detail: [value, 0, 0] }
    }
}

impl From<&CoreTxError> for SvStatus {
    fn from(source: &CoreTxError) -> Self {
        match source {
            CoreTxError::Rejected(code) => Self::business_rejection(*code),
            CoreTxError::Execution(source) => source.into(),
        }
    }
}

impl From<&CoreExecutionError> for SvStatus {
    fn from(source: &CoreExecutionError) -> Self {
        let tag = match source {
            CoreExecutionError::Schema(source) => return source.into(),
            CoreExecutionError::Access(source) => return source.into(),
            CoreExecutionError::Host(HostError::Resource(source)) => return resource_status(4, *source),
            CoreExecutionError::ReferenceTimeUnavailable => 5,
            CoreExecutionError::TxPositionUnavailable => 6,
            CoreExecutionError::PluginPanicked => 7,
            CoreExecutionError::HostPanicked => 8,
            CoreExecutionError::AbiContract(source) => return source.into(),
        };
        Self { tag, cause: 0, detail: [0; 3] }
    }
}

impl From<&CoreAbiError> for SvStatus {
    fn from(source: &CoreAbiError) -> Self {
        let (cause, detail) = match *source {
            CoreAbiError::NullPointer => (1, [0; 3]),
            CoreAbiError::Alignment { required } => (2, [required as u64, 0, 0]),
            CoreAbiError::Length { actual, maximum } => (3, [actual, maximum, 0]),
            CoreAbiError::Version { expected, actual } => (4, [expected.into(), actual.into(), 0]),
            CoreAbiError::Kind { expected, actual } => (5, [expected.into(), actual.into(), 0]),
            CoreAbiError::TableSize { expected, actual } => (6, [expected, actual, 0]),
            CoreAbiError::UnsupportedCapabilities { required } => (7, [required, 0, 0]),
            CoreAbiError::Flag { actual } => (8, [actual.into(), 0, 0]),
            CoreAbiError::Reserved { actual } => (9, [actual, 0, 0]),
            CoreAbiError::Utf8 => (10, [0; 3]),
            CoreAbiError::EmbeddedNul => (11, [0; 3]),
            CoreAbiError::InvalidHandle => (12, [0; 3]),
            CoreAbiError::BorrowConflict => (13, [0; 3]),
            CoreAbiError::ReentrantCall => (14, [0; 3]),
            CoreAbiError::CallbackCount { expected, actual } => (15, [expected, actual, 0]),
            CoreAbiError::Status(source) => match source {
                CoreStatusDecodeError::UnknownTag { tag } => (100, [tag.into(), 0, 0]),
                CoreStatusDecodeError::NonzeroSuccessCause { cause } => (101, [cause.into(), 0, 0]),
                CoreStatusDecodeError::IllegalReturn { operation, tag } => (102, [operation.code().into(), tag.into(), 0]),
                CoreStatusDecodeError::UnknownOperation { raw } => (103, [raw.into(), 0, 0]),
                CoreStatusDecodeError::UnexpectedTag { expected, actual } => (104, [expected.into(), actual.into(), 0]),
                CoreStatusDecodeError::UnknownCause { tag, cause } => (105, [tag.into(), cause.into(), 0]),
                CoreStatusDecodeError::UnknownStorageCause { raw } => (106, [raw, 0, 0]),
                CoreStatusDecodeError::InvalidBusinessCode { raw } => (107, [raw.into(), 0, 0]),
                CoreStatusDecodeError::DetailOutOfRange { slot, value, maximum } => (108, [slot.into(), value, maximum]),
                CoreStatusDecodeError::NonzeroUnusedDetail { slot, value } => (109, [slot.into(), value, 0]),
                CoreStatusDecodeError::NoncanonicalNestedFailure { cause } => (110, [cause.into(), 0, 0]),
            },
        };
        Self { tag: 9, cause, detail }
    }
}
