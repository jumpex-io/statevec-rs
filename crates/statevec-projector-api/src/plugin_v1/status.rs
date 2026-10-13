use super::{
    ConfigureFailure, HostFailure, ResourceFailure, SvStatus, ProjectorAbiFailure as A,
    ProjectorCallFailure as C, ProjectorFailure as F, ProjectorOperation as O,
    ProjectorIoStage, ProjectorIoKind, RebuildSourceFailure, ProjectorStatusDecodeFailure as D,
};

/// Decode representation before interpreting the return for this Projector call.
/// Known Core-only tags are refused without decoding their private payload.
pub fn decode_call(status: SvStatus, operation: O) -> Result<Option<C>, A> {
    if matches!(status.tag, 1 | 2 | 3 | 5 | 6) {
        return Err(A::Status(illegal(operation, status)));
    }
    let failure = decode_failure(status)?;
    if !operation.permits(status.tag, status.cause, subcategory(status)) {
        return Err(A::Status(illegal(operation, status)));
    }
    Ok(failure)
}

/// Validate a wire value, not its use as an invocation or completion slot.
pub fn decode_failure(status: SvStatus) -> Result<Option<C>, A> {
    let result = fields(Raw(status))?;
    if let Some(C::Failure(F::AbiContract(A::Status(nested)))) = result {
        check_nested(status.cause, nested)?;
    }
    Ok(result)
}

fn subcategory(s: SvStatus) -> u32 {
    if s.tag == 12 && matches!(s.cause, 2 | 5) { s.detail[0] as u32 } else { 0 }
}
fn illegal(operation: O, s: SvStatus) -> D {
    D::IllegalReturn {
        operation, tag: s.tag, cause: s.cause,
        stage: if s.tag == 12 && s.cause == 2 { ProjectorIoStage::from_code(subcategory(s)) } else { None },
        rebuild_reason: if s.tag == 12 && s.cause == 5 { RebuildSourceFailure::from_code(subcategory(s)) } else { None },
    }
}

#[derive(Clone, Copy)]
struct Raw(SvStatus);
impl Raw {
    fn zeros(self, first: u8) -> Result<(), A> {
        for slot in first..3 {
            let value = self.0.detail[slot as usize];
            if value != 0 { return Err(A::Status(D::NonzeroUnusedDetail { slot, value })); }
        }
        Ok(())
    }
    fn bounded(self, slot: u8, maximum: u64) -> Result<u64, A> {
        let value = self.0.detail[slot as usize];
        if value > maximum { Err(A::Status(D::DetailOutOfRange { slot, value, maximum })) } else { Ok(value) }
    }
    fn u32(self, slot: u8) -> Result<u32, A> { self.bounded(slot, u32::MAX.into()).map(|n| n as u32) }
    fn usize(self, slot: u8) -> Result<usize, A> { self.bounded(slot, usize::MAX as u64).map(|n| n as usize) }
    fn unknown(self) -> A { A::Status(D::UnknownCause { tag: self.0.tag, cause: self.0.cause }) }
    fn resource(self) -> Result<ResourceFailure, A> {
        match self.0.cause {
            1 => { self.zeros(1)?; Ok(ResourceFailure::Allocation { requested: self.usize(0)? }) }
            2 => { self.zeros(1)?; Ok(ResourceFailure::Os { code: self.u32(0)? as i32 }) }
            _ => Err(self.unknown()),
        }
    }
}
fn fields(r: Raw) -> Result<Option<C>, A> {
    let s = r.0;
    let failure = match s.tag {
        0 => {
            if s.cause != 0 { return Err(A::Status(D::NonzeroSuccessCause { cause: s.cause })); }
            r.zeros(0)?; return Ok(None);
        }
        4 => C::Failure(F::Host(HostFailure::Resource(r.resource()?))),
        7 | 8 => {
            if s.cause != 0 { return Err(r.unknown()); }
            r.zeros(0)?;
            C::Failure(if s.tag == 7 { F::PluginPanicked } else { F::HostPanicked })
        }
        9 => C::Failure(F::AbiContract(abi(r)?)),
        10 => {
            let value = match s.cause {
                1 => ConfigureFailure::Invalid { code: r.u32(0)? },
                2 => ConfigureFailure::UnsupportedCapabilities { required: s.detail[0] },
                3 => ConfigureFailure::Resource(ResourceFailure::Allocation { requested: r.usize(0)? }),
                4 => ConfigureFailure::Resource(ResourceFailure::Os { code: r.u32(0)? as i32 }),
                _ => return Err(r.unknown()),
            };
            r.zeros(1)?; C::Configuration(value)
        }
        11 => C::Failure(F::Construction(r.resource()?)),
        12 => match s.cause {
            1 => { r.zeros(1)?; C::Failure(F::Application { code: r.u32(0)? }) }
            2 => {
                let raw_stage = r.u32(0)?;
                let raw_kind = r.u32(1)?;
                let stage = ProjectorIoStage::from_code(raw_stage).ok_or(A::Flag { actual: raw_stage })?;
                let kind = ProjectorIoKind::from_code(raw_kind).ok_or(A::Flag { actual: raw_kind })?;
                let os = r.bounded(2, (1u64 << 33) - 1)?;
                let os_code = if os & (1u64 << 32) != 0 { Some(os as u32 as i32) } else {
                    if os != 0 { return Err(A::Reserved { actual: os }); }
                    None
                };
                C::Failure(F::Io { stage, kind, os_code })
            }
            3 | 4 => {
                r.zeros(0)?;
                if s.cause == 3 { C::WorkerJoinPanicked } else { C::ServiceJoinPanicked }
            }
            5 => {
                r.zeros(1)?;
                let raw = r.u32(0)?;
                C::Failure(F::RebuildSource(RebuildSourceFailure::from_code(raw).ok_or(A::Flag { actual: raw })?))
            }
            _ => return Err(r.unknown()),
        },
        1 | 2 | 3 | 5 | 6 => return Err(A::Status(D::UnexpectedTag { expected: 12, actual: s.tag })),
        tag => return Err(A::Status(D::UnknownTag { tag })),
    };
    Ok(Some(failure))
}

fn abi(r: Raw) -> Result<A, A> {
    let s = r.0;
    let used = match s.cause {
        1 | 10..=14 => 0,
        2 | 7..=9 | 16 | 100 | 101 | 103 | 110 => 1,
        3..=6 | 15 | 104 | 105 | 109 => 2,
        102 | 108 => 3,
        _ => return Err(r.unknown()),
    };
    r.zeros(used)?;
    Ok(match s.cause {
        1 => A::NullPointer,
        2 => A::Alignment { required: r.usize(0)? },
        3 => A::Length { actual: s.detail[0], maximum: s.detail[1] },
        4 => A::Version { expected: r.u32(0)?, actual: r.u32(1)? },
        5 => A::Kind { expected: r.u32(0)?, actual: r.u32(1)? },
        6 => A::TableSize { expected: s.detail[0], actual: s.detail[1] },
        7 => A::UnsupportedCapabilities { required: s.detail[0] },
        8 => A::Flag { actual: r.u32(0)? },
        9 => A::Reserved { actual: s.detail[0] },
        10 => A::Utf8,
        11 => A::EmbeddedNul,
        12 => A::InvalidHandle,
        13 => A::BorrowConflict,
        14 => A::ReentrantCall,
        15 => A::CallbackCount { expected: s.detail[0], actual: s.detail[1] },
        16 => {
            let scalar = r.u32(0)?;
            if !(1..=31).contains(&scalar) && !(127..=159).contains(&scalar) { return Err(A::Flag { actual: scalar }); }
            A::ControlCharacter { scalar }
        }
        100 => A::Status(D::UnknownTag { tag: r.u32(0)? }),
        101 => A::Status(D::NonzeroSuccessCause { cause: r.u32(0)? }),
        102 => {
            let raw = r.u32(0)?;
            let operation = O::from_code(raw).ok_or(A::Status(D::UnknownOperation { raw }))?;
            let tag = s.detail[1] as u32;
            let cause = (s.detail[1] >> 32) as u32;
            let (stage, rebuild_reason) = match (tag, cause) {
                (12, 2) => {
                    let raw = r.u32(2)?;
                    (Some(ProjectorIoStage::from_code(raw).ok_or(A::Flag { actual: raw })?), None)
                }
                (12, 5) => {
                    let raw = r.u32(2)?;
                    (None, Some(RebuildSourceFailure::from_code(raw).ok_or(A::Flag { actual: raw })?))
                }
                _ => { r.zeros(2)?; (None, None) }
            };
            A::Status(D::IllegalReturn { operation, tag, cause, stage, rebuild_reason })
        }
        103 => A::Status(D::UnknownOperation { raw: r.u32(0)? }),
        104 => A::Status(D::UnexpectedTag { expected: r.u32(0)?, actual: r.u32(1)? }),
        105 => A::Status(D::UnknownCause { tag: r.u32(0)?, cause: r.u32(1)? }),
        108 => A::Status(D::DetailOutOfRange { slot: r.bounded(0, u8::MAX.into())? as u8, value: s.detail[1], maximum: s.detail[2] }),
        109 => A::Status(D::NonzeroUnusedDetail { slot: r.bounded(0, u8::MAX.into())? as u8, value: s.detail[1] }),
        110 => A::Status(D::NoncanonicalNestedFailure { cause: r.u32(0)? }),
        _ => unreachable!("closed cause match"),
    })
}
fn check_nested(outer: u32, nested: D) -> Result<(), A> {
    let canonical = match nested {
        D::UnknownTag { tag } => tag > 12,
        D::NonzeroSuccessCause { cause } => cause != 0,
        D::IllegalReturn { operation, tag, cause, stage, rebuild_reason } => {
            let shape = match (tag, cause) {
                (12, 2) => stage.is_some() && rebuild_reason.is_none(),
                (12, 5) => stage.is_none() && rebuild_reason.is_some(),
                _ => stage.is_none() && rebuild_reason.is_none(),
            };
            let subcategory = stage.map_or(0, ProjectorIoStage::code) | rebuild_reason.map_or(0, RebuildSourceFailure::code);
            let raw = SvStatus { tag, cause, detail: [subcategory.into(), 0, 0] };
            // Core-only payloads are not interpreted. Every other cause is checked
            // through the direct decoder, not a duplicate known-cause range.
            let structural = matches!(tag, 1 | 2 | 3 | 5 | 6) || fields(Raw(raw)).is_ok();
            shape && tag <= 12 && structural && !operation.permits(tag, cause, subcategory)
        }
        D::UnknownOperation { raw } => O::from_code(raw).is_none(),
        D::UnexpectedTag { expected, actual } => expected == 12 && matches!(actual, 1 | 2 | 3 | 5 | 6),
        D::UnknownCause { tag, cause } => {
            fields(Raw(SvStatus { tag, cause, detail: [0; 3] })) == Err(A::Status(D::UnknownCause { tag, cause }))
        }
        D::DetailOutOfRange { slot, value, maximum } => value > maximum && match slot {
            0 => maximum == u8::MAX as u64 || maximum == u32::MAX as u64 || maximum == usize::MAX as u64,
            1 => maximum == u32::MAX as u64,
            2 => maximum == u32::MAX as u64 || maximum == (1u64 << 33) - 1,
            _ => false,
        },
        D::NonzeroUnusedDetail { slot, value } => slot < 3 && value != 0,
        D::NoncanonicalNestedFailure { cause } => matches!(cause, 100..=105 | 108..=110),
    };
    if canonical { Ok(()) } else { Err(A::Status(D::NoncanonicalNestedFailure { cause: outer })) }
}

fn resource(tag: u32, source: ResourceFailure) -> SvStatus {
    let (cause, value) = match source {
        ResourceFailure::Allocation { requested } => (1, requested as u64),
        ResourceFailure::Os { code } => (2, u64::from(code as u32)),
    };
    SvStatus { tag, cause, detail: [value, 0, 0] }
}
impl From<&ProjectorCallFailure> for SvStatus {
    fn from(source: &ProjectorCallFailure) -> Self {
        match source {
            C::Configuration(cause) => cause.into(),
            C::Failure(cause) => cause.into(),
            C::WorkerJoinPanicked => Self { tag: 12, cause: 3, detail: [0; 3] },
            C::ServiceJoinPanicked => Self { tag: 12, cause: 4, detail: [0; 3] },
        }
    }
}
use super::ProjectorCallFailure;
impl From<&F> for SvStatus {
    fn from(source: &F) -> Self {
        match *source {
            F::Application { code } => Self { tag: 12, cause: 1, detail: [code.into(), 0, 0] },
            F::Io { stage, kind, os_code } => Self { tag: 12, cause: 2, detail: [stage.code().into(), kind.code().into(), os_code.map_or(0, |code| (1u64 << 32) | u64::from(code as u32))] },
            F::Construction(cause) => resource(11, cause),
            F::Host(HostFailure::Resource(cause)) => resource(4, cause),
            F::PluginPanicked => Self { tag: 7, cause: 0, detail: [0; 3] },
            F::HostPanicked => Self { tag: 8, cause: 0, detail: [0; 3] },
            F::AbiContract(cause) => (&cause).into(),
            F::RebuildSource(reason) => Self { tag: 12, cause: 5, detail: [reason.code().into(), 0, 0] },
        }
    }
}
impl From<&A> for SvStatus {
    fn from(source: &A) -> Self {
        let (cause, detail) = match *source {
            A::NullPointer => (1, [0; 3]),
            A::Alignment { required } => (2, [required as u64, 0, 0]),
            A::Length { actual, maximum } => (3, [actual, maximum, 0]),
            A::Version { expected, actual } => (4, [expected.into(), actual.into(), 0]),
            A::Kind { expected, actual } => (5, [expected.into(), actual.into(), 0]),
            A::TableSize { expected, actual } => (6, [expected, actual, 0]),
            A::UnsupportedCapabilities { required } => (7, [required, 0, 0]),
            A::Flag { actual } => (8, [actual.into(), 0, 0]),
            A::Reserved { actual } => (9, [actual, 0, 0]),
            A::Utf8 => (10, [0; 3]), A::EmbeddedNul => (11, [0; 3]),
            A::InvalidHandle => (12, [0; 3]), A::BorrowConflict => (13, [0; 3]), A::ReentrantCall => (14, [0; 3]),
            A::CallbackCount { expected, actual } => (15, [expected, actual, 0]),
            A::ControlCharacter { scalar } => (16, [scalar.into(), 0, 0]),
            A::Status(source) => match source {
                D::UnknownTag { tag } => (100, [tag.into(), 0, 0]),
                D::NonzeroSuccessCause { cause } => (101, [cause.into(), 0, 0]),
                D::IllegalReturn { operation, tag, cause, stage, rebuild_reason } => (
                    102, [operation.code().into(), u64::from(tag) | (u64::from(cause) << 32),
                    u64::from(stage.map_or(0, ProjectorIoStage::code)) | u64::from(rebuild_reason.map_or(0, RebuildSourceFailure::code))]),
                D::UnknownOperation { raw } => (103, [raw.into(), 0, 0]),
                D::UnexpectedTag { expected, actual } => (104, [expected.into(), actual.into(), 0]),
                D::UnknownCause { tag, cause } => (105, [tag.into(), cause.into(), 0]),
                D::DetailOutOfRange { slot, value, maximum } => (108, [slot.into(), value, maximum]),
                D::NonzeroUnusedDetail { slot, value } => (109, [slot.into(), value, 0]),
                D::NoncanonicalNestedFailure { cause } => (110, [cause.into(), 0, 0]),
            },
        };
        // Public Rust diagnostic enums can be constructed inconsistently. Do
        // not emit a malformed completion (or OR unrelated stage/reason bits
        // into a plausible but different claim). Report that author error in
        // the existing one-level noncanonical vocabulary instead.
        if let A::Status(nested) = *source {
            if check_nested(cause, nested).is_err() {
                return Self { tag: 9, cause: 110, detail: [cause.into(), 0, 0] };
            }
        }
        let status = Self { tag: 9, cause, detail };
        match decode_failure(status) {
            Ok(_) => status,
            Err(rejection) => (&rejection).into(),
        }
    }
}
