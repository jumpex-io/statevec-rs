use crate::{BusinessRejectCode, v1::*};
use test_case::test_case;

#[test_case(ConfigureError::Invalid { code: u32::MAX }, 1, 0xffff_ffff; "invalid configuration")]
#[test_case(ConfigureError::UnsupportedCapabilities { required: u64::MAX }, 2, u64::MAX; "required capabilities")]
#[test_case(ConfigureError::Resource(ResourceError::Allocation { requested: 4096 }), 3, 4096; "configuration allocation")]
#[test_case(ConfigureError::Resource(ResourceError::Os { code: i32::MIN }), 4, 0x8000_0000; "signed os error")]
fn configuration_codes_are_not_business_codes(failure: ConfigureError, cause: u32, value: u64) {
    let expected = SvStatus { tag: 10, cause, detail: [value, 0, 0] };

    assert_eq!(SvStatus::from(&failure), expected);
    assert_eq!(expected.decode_for(CoreOperation::Configure), Ok(Some(CoreError::Configuration(failure))));
    assert_eq!(expected.decode_for(CoreOperation::RunTx), Err(CoreStatusDecodeError::IllegalReturn { operation: CoreOperation::RunTx, tag: 10 }));
}

#[test_case(CoreError::Construction(CoreCreateError::Resource(ResourceError::Allocation { requested: 4096 })), CoreOperation::CreateInstance, 11, 1, 4096; "create allocation")]
#[test_case(CoreError::Construction(CoreCreateError::Resource(ResourceError::Os { code: -1 })), CoreOperation::CreateInstance, 11, 2, 0xffff_ffff; "create os")]
#[test_case(CoreError::Transaction(CoreTxError::Execution(CoreExecutionError::Host(HostError::Resource(ResourceError::Allocation { requested: 4096 })))), CoreOperation::HostCallback, 4, 1, 4096; "host allocation")]
#[test_case(CoreError::Transaction(CoreTxError::Execution(CoreExecutionError::Host(HostError::Resource(ResourceError::Os { code: i32::MAX })))), CoreOperation::HostCallback, 4, 2, 0x7fff_ffff; "host os")]
fn resource_status_preserves_the_operation_and_signed_error(failure: CoreError, operation: CoreOperation, tag: u32, cause: u32, value: u64) {
    let expected = SvStatus { tag, cause, detail: [value, 0, 0] };

    assert_eq!(SvStatus::from(&failure), expected);
    assert_eq!(expected.decode_for(operation), Ok(Some(failure)));
}

#[test_case(CoreExecutionError::ReferenceTimeUnavailable, 5; "reference time")]
#[test_case(CoreExecutionError::TxPositionUnavailable, 6; "position")]
#[test_case(CoreExecutionError::PluginPanicked, 7; "plugin panic")]
#[test_case(CoreExecutionError::HostPanicked, 8; "host panic")]
fn execution_categories_are_distinct_and_have_no_payload(failure: CoreExecutionError, tag: u32) {
    let expected = SvStatus { tag, cause: 0, detail: [0; 3] };

    assert_eq!(SvStatus::from(&failure), expected);
    assert_eq!(expected.decode_for(CoreOperation::RunTx), Ok(Some(CoreError::Transaction(CoreTxError::Execution(failure)))));
    assert_eq!(SvStatus { cause: 1, ..expected }.decode_for(CoreOperation::RunTx), Err(CoreStatusDecodeError::UnknownCause { tag, cause: 1 }));
    for slot in 0..3 {
        let mut noncanonical = expected;
        noncanonical.detail[slot] = 1;
        assert_eq!(noncanonical.decode_for(CoreOperation::RunTx), Err(CoreStatusDecodeError::NonzeroUnusedDetail { slot: slot as u8, value: 1 }));
    }
}

#[test_case(CoreAbiError::NullPointer, 1, [0, 0, 0]; "null")]
#[test_case(CoreAbiError::Alignment { required: 8 }, 2, [8, 0, 0]; "alignment")]
#[test_case(CoreAbiError::Length { actual: u64::MAX, maximum: 16 }, 3, [u64::MAX, 16, 0]; "length")]
#[test_case(CoreAbiError::Version { expected: 3, actual: u32::MAX }, 4, [3, 0xffff_ffff, 0]; "version")]
#[test_case(CoreAbiError::Kind { expected: 1, actual: 2 }, 5, [1, 2, 0]; "kind")]
#[test_case(CoreAbiError::TableSize { expected: 128, actual: 64 }, 6, [128, 64, 0]; "table size")]
#[test_case(CoreAbiError::UnsupportedCapabilities { required: 1 << 63 }, 7, [1 << 63, 0, 0]; "capabilities")]
#[test_case(CoreAbiError::Flag { actual: 2 }, 8, [2, 0, 0]; "flag")]
#[test_case(CoreAbiError::Reserved { actual: u64::MAX }, 9, [u64::MAX, 0, 0]; "reserved")]
#[test_case(CoreAbiError::Utf8, 10, [0, 0, 0]; "utf8")]
#[test_case(CoreAbiError::EmbeddedNul, 11, [0, 0, 0]; "nul")]
#[test_case(CoreAbiError::InvalidHandle, 12, [0, 0, 0]; "handle")]
#[test_case(CoreAbiError::BorrowConflict, 13, [0, 0, 0]; "borrow")]
#[test_case(CoreAbiError::ReentrantCall, 14, [0, 0, 0]; "reentry")]
#[test_case(CoreAbiError::CallbackCount { expected: 1, actual: 2 }, 15, [1, 2, 0]; "callback count")]
#[test_case(CoreAbiError::Status(CoreStatusDecodeError::UnknownTag { tag: 99 }), 100, [99, 0, 0]; "unknown tag")]
#[test_case(CoreAbiError::Status(CoreStatusDecodeError::NonzeroSuccessCause { cause: 7 }), 101, [7, 0, 0]; "nonzero success")]
#[test_case(CoreAbiError::Status(CoreStatusDecodeError::IllegalReturn { operation: CoreOperation::PreflightBatch, tag: 1 }), 102, [4, 1, 0]; "illegal return")]
#[test_case(CoreAbiError::Status(CoreStatusDecodeError::UnknownOperation { raw: 99 }), 103, [99, 0, 0]; "unknown operation")]
#[test_case(CoreAbiError::Status(CoreStatusDecodeError::UnexpectedTag { expected: 1, actual: 7 }), 104, [1, 7, 0]; "wrong tag")]
#[test_case(CoreAbiError::Status(CoreStatusDecodeError::UnknownCause { tag: 10, cause: 99 }), 105, [10, 99, 0]; "unknown cause")]
#[test_case(CoreAbiError::Status(CoreStatusDecodeError::UnknownStorageCause { raw: u64::MAX }), 106, [u64::MAX, 0, 0]; "unknown storage")]
#[test_case(CoreAbiError::Status(CoreStatusDecodeError::InvalidBusinessCode { raw: 60000 }), 107, [60000, 0, 0]; "invalid business")]
#[test_case(CoreAbiError::Status(CoreStatusDecodeError::DetailOutOfRange { slot: 2, value: 256, maximum: 255 }), 108, [2, 256, 255]; "detail range")]
#[test_case(CoreAbiError::Status(CoreStatusDecodeError::NonzeroUnusedDetail { slot: 1, value: u64::MAX }), 109, [1, u64::MAX, 0]; "unused detail")]
#[test_case(CoreAbiError::Status(CoreStatusDecodeError::NoncanonicalNestedFailure { cause: 108 }), 110, [108, 0, 0]; "contradictory nested failure")]
#[test_case(CoreAbiError::Status(CoreStatusDecodeError::NoncanonicalNestedFailure { cause: 100 }), 110, [100, 0, 0]; "first nested diagnostic")]
#[test_case(CoreAbiError::Status(CoreStatusDecodeError::NoncanonicalNestedFailure { cause: 110 }), 110, [110, 0, 0]; "contradictory diagnostic report")]
fn abi_causes_match_independent_records(failure: CoreAbiError, cause: u32, detail: [u64; 3]) {
    let expected = SvStatus { tag: 9, cause, detail };

    assert_eq!(SvStatus::from(&failure), expected);
    assert_eq!(expected.decode_for(CoreOperation::RunTx), Ok(Some(CoreError::Transaction(CoreTxError::Execution(CoreExecutionError::AbiContract(failure))))));
}

#[test_case(100, [9, 0, 0]; "known tag")]
#[test_case(101, [0, 0, 0]; "zero success cause")]
#[test_case(102, [5, 7, 0]; "permitted panic")]
#[test_case(102, [5, 99, 0]; "unknown tag is not illegal return")]
#[test_case(103, [9, 0, 0]; "known operation")]
#[test_case(104, [2, 2, 0]; "matching tags")]
#[test_case(105, [3, 1, 0]; "known storage cause")]
#[test_case(105, [99, 1, 0]; "unknown tag has no cause vocabulary")]
#[test_case(105, [9, 105, 0]; "known nested unknown cause")]
#[test_case(105, [9, 110, 0]; "known nested diagnostic cause")]
#[test_case(106, [12, 0, 0]; "known storage category")]
#[test_case(107, [1, 0, 0]; "valid business code")]
#[test_case(108, [3, 256, 255]; "nonexistent detail slot")]
#[test_case(108, [2, 255, 255]; "in range detail")]
#[test_case(109, [3, 1, 0]; "nonexistent unused slot")]
#[test_case(109, [1, 0, 0]; "zero unused detail")]
#[test_case(110, [0, 0, 0]; "zero is not a nested diagnostic")]
#[test_case(110, [99, 0, 0]; "before nested diagnostic vocabulary")]
#[test_case(110, [111, 0, 0]; "after nested diagnostic vocabulary")]
#[test_case(110, [u32::MAX as u64, 0, 0]; "unknown nested diagnostic")]
fn contradictory_nested_failure_is_an_abi_refusal(cause: u32, detail: [u64; 3]) {
    let wire = SvStatus { tag: 9, cause, detail };
    let failure = wire.decode_for(CoreOperation::RunTx).expect_err("contradictory nested failure is not canonical");
    assert_eq!(failure, CoreStatusDecodeError::NoncanonicalNestedFailure { cause });
    let report = SvStatus::from(&CoreAbiError::Status(failure));
    assert_eq!(report.decode_for(CoreOperation::RunTx), Ok(Some(CoreError::Transaction(
        CoreTxError::Execution(CoreExecutionError::AbiContract(CoreAbiError::Status(failure)))
    ))), "the rejection itself must round-trip");
}

#[test]
fn nested_unknown_cause_agrees_with_the_actual_decoder() {
    // Exercise both public decoding paths across the current cause vocabulary
    // and its gaps, including diagnostics about other nested diagnostics.
    for tag in (0..=12).chain([u32::MAX]) {
        for cause in (0..=111).chain([u32::MAX]) {
            let raw = SvStatus { tag, cause, detail: [0; 3] };
            let failure = CoreStatusDecodeError::UnknownCause { tag, cause };
            let report = SvStatus::from(&CoreAbiError::Status(failure));
            let actual = report.decode_for(CoreOperation::RunTx);
            let expected = if raw.decode_for(CoreOperation::RunTx) == Err(failure) {
                Ok(Some(CoreError::Transaction(CoreTxError::Execution(
                    CoreExecutionError::AbiContract(CoreAbiError::Status(failure)),
                ))))
            } else {
                Err(CoreStatusDecodeError::NoncanonicalNestedFailure { cause: 105 })
            };
            assert_eq!(actual, expected, "tag={tag}, cause={cause}");
        }
    }
}

// Independent wire records, one canonical representative of every Core tag.
const RETURNS: [SvStatus; 12] = [
    SvStatus::SUCCESS,
    SvStatus { tag: 1, cause: 1, detail: [0; 3] },
    SvStatus { tag: 2, cause: 1, detail: [0; 3] },
    SvStatus { tag: 3, cause: 2, detail: [0; 3] },
    SvStatus { tag: 4, cause: 1, detail: [0; 3] },
    SvStatus { tag: 5, cause: 0, detail: [0; 3] },
    SvStatus { tag: 6, cause: 0, detail: [0; 3] },
    SvStatus { tag: 7, cause: 0, detail: [0; 3] },
    SvStatus { tag: 8, cause: 0, detail: [0; 3] },
    SvStatus { tag: 9, cause: 13, detail: [0; 3] },
    SvStatus { tag: 10, cause: 1, detail: [0; 3] },
    SvStatus { tag: 11, cause: 1, detail: [0; 3] },
];

#[test_case(CoreOperation::Configure, &[0, 7, 9, 10]; "configure")]
#[test_case(CoreOperation::Metadata, &[0, 7, 9]; "metadata")]
#[test_case(CoreOperation::CreateInstance, &[0, 7, 9, 11]; "create")]
#[test_case(CoreOperation::PreflightBatch, &[0, 2, 3, 4, 5, 6, 7, 8, 9]; "preflight")]
#[test_case(CoreOperation::RunTx, &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9]; "run")]
#[test_case(CoreOperation::ValidateTxInvariants, &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9]; "rules")]
#[test_case(CoreOperation::DestroyInstance, &[0, 7, 9]; "destroy instance")]
#[test_case(CoreOperation::DestroyConfigured, &[0, 7, 9]; "destroy configured")]
#[test_case(CoreOperation::HostCallback, &[0, 2, 3, 4, 5, 6, 7, 8, 9]; "host callback")]
fn operation_selects_legal_returns(operation: CoreOperation, accepted: &[u32]) {
    for wire in RETURNS {
        let result = wire.decode_for(operation);
        if !accepted.contains(&wire.tag) {
            assert_eq!(result, Err(CoreStatusDecodeError::IllegalReturn { operation, tag: wire.tag }));
        } else if wire == SvStatus::SUCCESS {
            assert_eq!(result, Ok(None));
        } else {
            let failure = result.unwrap().expect("nonzero status must retain a cause");
            assert_eq!(SvStatus::from(&failure), wire);
        }
    }
}

#[test_case(SvStatus { cause: 1, ..SvStatus::SUCCESS }, CoreStatusDecodeError::NonzeroSuccessCause { cause: 1 }; "success cause")]
#[test_case(SvStatus { detail: [0, 0, 1], ..SvStatus::SUCCESS }, CoreStatusDecodeError::NonzeroUnusedDetail { slot: 2, value: 1 }; "success detail")]
#[test_case(SvStatus { tag: 12, cause: 1, detail: [0; 3] }, CoreStatusDecodeError::UnknownTag { tag: 12 }; "projector not core")]
#[test_case(SvStatus { tag: u32::MAX, cause: 0, detail: [0; 3] }, CoreStatusDecodeError::UnknownTag { tag: u32::MAX }; "unknown tag")]
#[test_case(SvStatus { tag: 9, cause: 102, detail: [0, 1, 0] }, CoreStatusDecodeError::UnknownOperation { raw: 0 }; "nested unknown operation")]
#[test_case(SvStatus { tag: 9, cause: 16, detail: [0; 3] }, CoreStatusDecodeError::UnknownCause { tag: 9, cause: 16 }; "unknown abi cause")]
#[test_case(SvStatus { tag: 9, cause: 4, detail: [3, 1 << 32, 0] }, CoreStatusDecodeError::DetailOutOfRange { slot: 1, value: 1 << 32, maximum: 0xffff_ffff }; "version width")]
#[test_case(SvStatus { tag: 9, cause: 109, detail: [256, 1, 0] }, CoreStatusDecodeError::DetailOutOfRange { slot: 0, value: 256, maximum: 255 }; "slot width")]
#[test_case(SvStatus { tag: 4, cause: 2, detail: [1 << 32, 0, 0] }, CoreStatusDecodeError::DetailOutOfRange { slot: 0, value: 1 << 32, maximum: 0xffff_ffff }; "os width")]
#[test_case(SvStatus { tag: 4, cause: 3, detail: [0; 3] }, CoreStatusDecodeError::UnknownCause { tag: 4, cause: 3 }; "unknown host cause")]
#[test_case(SvStatus { tag: 9, cause: 13, detail: [1, 0, 0] }, CoreStatusDecodeError::NonzeroUnusedDetail { slot: 0, value: 1 }; "borrow spare")]
fn malformed_records_are_not_reclassified(wire: SvStatus, expected: CoreStatusDecodeError) {
    assert_eq!(wire.decode_for(CoreOperation::RunTx), Err(expected));
}

#[test]
fn ordinary_business_error_remains_a_typed_business_code() {
    let code = BusinessRejectCode::new(42).unwrap();
    let wire = SvStatus { tag: 1, cause: 42, detail: [0; 3] };

    assert_eq!(wire.decode_for(CoreOperation::RunTx), Ok(Some(CoreError::Transaction(CoreTxError::Rejected(code)))));
    assert_eq!(SvStatus::from(&CoreTxError::Rejected(code)), wire);
}
