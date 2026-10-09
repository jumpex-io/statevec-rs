use crate::{BusinessRejectCode, plugin_v3::*};
use test_case::test_case;

#[test_case(ConfigureFailure::Invalid { code: u32::MAX }, 1, 0xffff_ffff; "invalid configuration")]
#[test_case(ConfigureFailure::UnsupportedCapabilities { required: u64::MAX }, 2, u64::MAX; "required capabilities")]
#[test_case(ConfigureFailure::Resource(ResourceFailure::Allocation { requested: 4096 }), 3, 4096; "configuration allocation")]
#[test_case(ConfigureFailure::Resource(ResourceFailure::Os { code: i32::MIN }), 4, 0x8000_0000; "signed os error")]
fn configuration_codes_are_not_business_codes(failure: ConfigureFailure, cause: u32, value: u64) {
    let expected = SvStatus { tag: 10, cause, detail: [value, 0, 0] };

    assert_eq!(SvStatus::from(&failure), expected);
    assert_eq!(expected.decode_for(CoreOperation::Configure), Ok(Some(CoreFailure::Configuration(failure))));
    assert_eq!(expected.decode_for(CoreOperation::RunTx), Err(StatusDecodeFailure::IllegalReturn { operation: CoreOperation::RunTx, tag: 10 }));
}

#[test_case(CoreFailure::Construction(CreateFailure::Resource(ResourceFailure::Allocation { requested: 4096 })), CoreOperation::CreateInstance, 11, 1, 4096; "create allocation")]
#[test_case(CoreFailure::Construction(CreateFailure::Resource(ResourceFailure::Os { code: -1 })), CoreOperation::CreateInstance, 11, 2, 0xffff_ffff; "create os")]
#[test_case(CoreFailure::Transaction(TxFailure::Execution(ExecutionFailure::Host(HostFailure::Resource(ResourceFailure::Allocation { requested: 4096 })))), CoreOperation::HostCallback, 4, 1, 4096; "host allocation")]
#[test_case(CoreFailure::Transaction(TxFailure::Execution(ExecutionFailure::Host(HostFailure::Resource(ResourceFailure::Os { code: i32::MAX })))), CoreOperation::HostCallback, 4, 2, 0x7fff_ffff; "host os")]
fn resource_status_preserves_the_operation_and_signed_error(failure: CoreFailure, operation: CoreOperation, tag: u32, cause: u32, value: u64) {
    let expected = SvStatus { tag, cause, detail: [value, 0, 0] };

    assert_eq!(SvStatus::from(&failure), expected);
    assert_eq!(expected.decode_for(operation), Ok(Some(failure)));
}

#[test_case(ExecutionFailure::ReferenceTimeUnavailable, 5; "reference time")]
#[test_case(ExecutionFailure::TxPositionUnavailable, 6; "position")]
#[test_case(ExecutionFailure::PluginPanicked, 7; "plugin panic")]
#[test_case(ExecutionFailure::HostPanicked, 8; "host panic")]
fn execution_categories_are_distinct_and_have_no_payload(failure: ExecutionFailure, tag: u32) {
    let expected = SvStatus { tag, cause: 0, detail: [0; 3] };

    assert_eq!(SvStatus::from(&failure), expected);
    assert_eq!(expected.decode_for(CoreOperation::RunTx), Ok(Some(CoreFailure::Transaction(TxFailure::Execution(failure)))));
    assert_eq!(SvStatus { cause: 1, ..expected }.decode_for(CoreOperation::RunTx), Err(StatusDecodeFailure::UnknownCause { tag, cause: 1 }));
    for slot in 0..3 {
        let mut noncanonical = expected;
        noncanonical.detail[slot] = 1;
        assert_eq!(noncanonical.decode_for(CoreOperation::RunTx), Err(StatusDecodeFailure::NonzeroUnusedDetail { slot: slot as u8, value: 1 }));
    }
}

#[test_case(AbiFailure::NullPointer, 1, [0, 0, 0]; "null")]
#[test_case(AbiFailure::Alignment { required: 8 }, 2, [8, 0, 0]; "alignment")]
#[test_case(AbiFailure::Length { actual: u64::MAX, maximum: 16 }, 3, [u64::MAX, 16, 0]; "length")]
#[test_case(AbiFailure::Version { expected: 3, actual: u32::MAX }, 4, [3, 0xffff_ffff, 0]; "version")]
#[test_case(AbiFailure::Kind { expected: 1, actual: 2 }, 5, [1, 2, 0]; "kind")]
#[test_case(AbiFailure::TableSize { expected: 128, actual: 64 }, 6, [128, 64, 0]; "table size")]
#[test_case(AbiFailure::UnsupportedCapabilities { required: 1 << 63 }, 7, [1 << 63, 0, 0]; "capabilities")]
#[test_case(AbiFailure::Flag { actual: 2 }, 8, [2, 0, 0]; "flag")]
#[test_case(AbiFailure::Reserved { actual: u64::MAX }, 9, [u64::MAX, 0, 0]; "reserved")]
#[test_case(AbiFailure::Utf8, 10, [0, 0, 0]; "utf8")]
#[test_case(AbiFailure::EmbeddedNul, 11, [0, 0, 0]; "nul")]
#[test_case(AbiFailure::InvalidHandle, 12, [0, 0, 0]; "handle")]
#[test_case(AbiFailure::BorrowConflict, 13, [0, 0, 0]; "borrow")]
#[test_case(AbiFailure::ReentrantCall, 14, [0, 0, 0]; "reentry")]
#[test_case(AbiFailure::CallbackCount { expected: 1, actual: 2 }, 15, [1, 2, 0]; "callback count")]
#[test_case(AbiFailure::Status(StatusDecodeFailure::UnknownTag { tag: 99 }), 100, [99, 0, 0]; "unknown tag")]
#[test_case(AbiFailure::Status(StatusDecodeFailure::NonzeroSuccessCause { cause: 7 }), 101, [7, 0, 0]; "nonzero success")]
#[test_case(AbiFailure::Status(StatusDecodeFailure::IllegalReturn { operation: CoreOperation::PreflightBatch, tag: 1 }), 102, [4, 1, 0]; "illegal return")]
#[test_case(AbiFailure::Status(StatusDecodeFailure::UnknownOperation { raw: 99 }), 103, [99, 0, 0]; "unknown operation")]
#[test_case(AbiFailure::Status(StatusDecodeFailure::UnexpectedTag { expected: 1, actual: 7 }), 104, [1, 7, 0]; "wrong tag")]
#[test_case(AbiFailure::Status(StatusDecodeFailure::UnknownCause { tag: 10, cause: 99 }), 105, [10, 99, 0]; "unknown cause")]
#[test_case(AbiFailure::Status(StatusDecodeFailure::UnknownStorageCause { raw: u64::MAX }), 106, [u64::MAX, 0, 0]; "unknown storage")]
#[test_case(AbiFailure::Status(StatusDecodeFailure::InvalidBusinessCode { raw: 60000 }), 107, [60000, 0, 0]; "invalid business")]
#[test_case(AbiFailure::Status(StatusDecodeFailure::DetailOutOfRange { slot: 2, value: 256, maximum: 255 }), 108, [2, 256, 255]; "detail range")]
#[test_case(AbiFailure::Status(StatusDecodeFailure::NonzeroUnusedDetail { slot: 1, value: u64::MAX }), 109, [1, u64::MAX, 0]; "unused detail")]
fn abi_causes_match_independent_records(failure: AbiFailure, cause: u32, detail: [u64; 3]) {
    let expected = SvStatus { tag: 9, cause, detail };

    assert_eq!(SvStatus::from(&failure), expected);
    assert_eq!(expected.decode_for(CoreOperation::RunTx), Ok(Some(CoreFailure::Transaction(TxFailure::Execution(ExecutionFailure::AbiContract(failure))))));
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
            assert_eq!(result, Err(StatusDecodeFailure::IllegalReturn { operation, tag: wire.tag }));
        } else if wire == SvStatus::SUCCESS {
            assert_eq!(result, Ok(None));
        } else {
            let failure = result.unwrap().expect("nonzero status must retain a cause");
            assert_eq!(SvStatus::from(&failure), wire);
        }
    }
}

#[test_case(SvStatus { cause: 1, ..SvStatus::SUCCESS }, StatusDecodeFailure::NonzeroSuccessCause { cause: 1 }; "success cause")]
#[test_case(SvStatus { detail: [0, 0, 1], ..SvStatus::SUCCESS }, StatusDecodeFailure::NonzeroUnusedDetail { slot: 2, value: 1 }; "success detail")]
#[test_case(SvStatus { tag: 12, cause: 1, detail: [0; 3] }, StatusDecodeFailure::UnknownTag { tag: 12 }; "projector not core")]
#[test_case(SvStatus { tag: u32::MAX, cause: 0, detail: [0; 3] }, StatusDecodeFailure::UnknownTag { tag: u32::MAX }; "unknown tag")]
#[test_case(SvStatus { tag: 9, cause: 102, detail: [0, 1, 0] }, StatusDecodeFailure::UnknownOperation { raw: 0 }; "nested unknown operation")]
#[test_case(SvStatus { tag: 9, cause: 16, detail: [0; 3] }, StatusDecodeFailure::UnknownCause { tag: 9, cause: 16 }; "unknown abi cause")]
#[test_case(SvStatus { tag: 9, cause: 4, detail: [3, 1 << 32, 0] }, StatusDecodeFailure::DetailOutOfRange { slot: 1, value: 1 << 32, maximum: 0xffff_ffff }; "version width")]
#[test_case(SvStatus { tag: 9, cause: 109, detail: [256, 1, 0] }, StatusDecodeFailure::DetailOutOfRange { slot: 0, value: 256, maximum: 255 }; "slot width")]
#[test_case(SvStatus { tag: 4, cause: 2, detail: [1 << 32, 0, 0] }, StatusDecodeFailure::DetailOutOfRange { slot: 0, value: 1 << 32, maximum: 0xffff_ffff }; "os width")]
#[test_case(SvStatus { tag: 4, cause: 3, detail: [0; 3] }, StatusDecodeFailure::UnknownCause { tag: 4, cause: 3 }; "unknown host cause")]
#[test_case(SvStatus { tag: 9, cause: 13, detail: [1, 0, 0] }, StatusDecodeFailure::NonzeroUnusedDetail { slot: 0, value: 1 }; "borrow spare")]
fn malformed_records_are_not_reclassified(wire: SvStatus, expected: StatusDecodeFailure) {
    assert_eq!(wire.decode_for(CoreOperation::RunTx), Err(expected));
}

#[test]
fn ordinary_business_error_remains_a_typed_business_code() {
    let code = BusinessRejectCode::new(42).unwrap();
    let wire = SvStatus { tag: 1, cause: 42, detail: [0; 3] };

    assert_eq!(wire.decode_for(CoreOperation::RunTx), Ok(Some(CoreFailure::Transaction(TxFailure::Rejected(code)))));
    assert_eq!(SvStatus::from(&TxFailure::Rejected(code)), wire);
}
