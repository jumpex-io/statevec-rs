use crate::{BusinessRejectCode, v1::*};
use statevec_model::{RecordKey, RecordValueError, ReservedBytesZeroError};
use test_case::test_case;

const KEY: RecordKey = RecordKey { kind: 0x1234, sys_id: u64::MAX };

#[test]
fn status_has_the_c_layout_and_only_canonical_zero_is_success() {
    assert_eq!(std::mem::size_of::<SvStatus>(), 32);
    assert_eq!(std::mem::align_of::<SvStatus>(), std::mem::align_of::<u64>());
    assert_eq!(std::mem::offset_of!(SvStatus, tag), 0);
    assert_eq!(std::mem::offset_of!(SvStatus, cause), 4);
    assert_eq!(std::mem::offset_of!(SvStatus, detail), 8);
    assert!(SvStatus::SUCCESS.is_success());
    for status in [
        SvStatus { tag: 1, ..SvStatus::SUCCESS },
        SvStatus { cause: 1, ..SvStatus::SUCCESS },
        SvStatus { detail: [1, 0, 0], ..SvStatus::SUCCESS },
        SvStatus { detail: [0, 1, 0], ..SvStatus::SUCCESS },
        SvStatus { detail: [0, 0, 1], ..SvStatus::SUCCESS },
    ] {
        assert!(!status.is_success(), "noncanonical success: {status:?}");
    }
}

#[test_case(CoreSchemaError::UnsupportedKind { kind: u16::MAX }, 1, [65535, 0, 0]; "unsupported kind")]
#[test_case(CoreSchemaError::FieldAccess { field: 7, required: 64, actual: 1 }, 2, [7, 64, 1]; "field access")]
#[test_case(CoreSchemaError::Enum { field: 7, raw: 255 }, 3, [7, 255, 0]; "enum")]
#[test_case(CoreSchemaError::Boolean { field: 7, raw: 2 }, 4, [7, 2, 0]; "boolean")]
#[test_case(CoreSchemaError::FixedBytesLength { field: 7, length: 65, capacity: 64 }, 5, [7, 65, 64]; "fixed length")]
#[test_case(CoreSchemaError::FixedBytesPadding { field: 7, offset: 32, raw: 255 }, 6, [7, 32, 255]; "padding")]
#[test_case(CoreSchemaError::TrailingBytes { consumed: 64, actual: 65 }, 7, [64, 65, 0]; "trailing bytes")]
fn schema_causes_match_independent_status_records(failure: CoreSchemaError, cause: u32, detail: [u64; 3]) {
    let expected = SvStatus { tag: 2, cause, detail };

    assert_eq!(SvStatus::from(&failure), expected);
    assert_eq!(expected.decode_schema(), Ok(failure));
    assert_eq!(expected.decode_business_rejection(), Err(CoreStatusDecodeError::UnexpectedTag { expected: 1, actual: 2 }));
}

#[test_case(CoreStorageError::Page, 1; "page")]
#[test_case(CoreStorageError::Arena, 2; "arena")]
#[test_case(CoreStorageError::Slot, 3; "slot")]
#[test_case(CoreStorageError::Address, 4; "address")]
#[test_case(CoreStorageError::Kind, 5; "kind")]
#[test_case(CoreStorageError::SysId, 6; "sys id")]
#[test_case(CoreStorageError::Payload, 7; "payload")]
#[test_case(CoreStorageError::Capacity, 8; "capacity")]
#[test_case(CoreStorageError::Invariant, 9; "invariant")]
#[test_case(CoreStorageError::Snapshot, 10; "snapshot")]
#[test_case(CoreStorageError::Io, 11; "io")]
#[test_case(CoreStorageError::Internal, 12; "internal")]
fn storage_categories_do_not_require_diagnostic_strings(failure: CoreStorageError, code: u64) {
    let expected = SvStatus { tag: 3, cause: 1, detail: [code, 0, 0] };

    assert_eq!(SvStatus::from(&CoreAccessError::Storage(failure)), expected);
    assert_eq!(expected.decode_access(), Ok(CoreAccessError::Storage(failure)));
}

#[test_case(CoreAccessError::SchemaMissing { kind: 0x1234 }, 2, [0x1234, 0, 0]; "schema missing")]
#[test_case(CoreAccessError::UnknownUniqueKey { kind: 0x1234, unique_key_id: 255 }, 3, [0x1234, 255, 0]; "unknown uk")]
#[test_case(CoreAccessError::MissingRecord { key: KEY }, 4, [0x1234, u64::MAX, 0]; "missing record")]
#[test_case(CoreAccessError::SysIdAlreadyUsed { sys_id: u64::MAX }, 5, [u64::MAX, 0, 0]; "used id")]
#[test_case(CoreAccessError::RevisionOverflow { key: KEY }, 6, [0x1234, u64::MAX, 0]; "revision overflow")]
#[test_case(CoreAccessError::ImmutableFieldUpdate { key: KEY, field_index: u32::MAX }, 7, [0x1234, u64::MAX, 0xffff_ffff]; "immutable field")]
#[test_case(CoreAccessError::UniqueKeyUpdate { key: KEY }, 8, [0x1234, u64::MAX, 0]; "uk update")]
#[test_case(CoreAccessError::ReservedBytesInvariant(ReservedBytesZeroError::RecordDataTooShort { kind: 0x1234, required: 64, actual: 1 }), 9, [0x1234, 64, 1]; "short record")]
#[test_case(CoreAccessError::ReservedBytesInvariant(ReservedBytesZeroError::PayloadTailNonZero { kind: 0x1234, data_size: 64, payload_len: 65 }), 10, [0x1234, 64, 65]; "payload tail")]
#[test_case(CoreAccessError::ReservedBytesInvariant(ReservedBytesZeroError::ReservedSpanOutOfBounds { kind: 0x1234, field_index: 0x5678_9abc, offset: 0x8765_4321, len: 0xfedc_ba98, data_size: 64 }), 11, [0x5678_9abc_1234, 0xfedc_ba98_8765_4321, 64]; "span out of bounds")]
#[test_case(CoreAccessError::ReservedBytesInvariant(ReservedBytesZeroError::ReservedSpanNonZero { kind: 0x1234, field_index: 0x5678_9abc, offset: 0x8765_4321, len: 0xfedc_ba98 }), 12, [0x5678_9abc_1234, 0xfedc_ba98_8765_4321, 0]; "nonzero span")]
#[test_case(CoreAccessError::UniqueKeyConflict { kind: 0x1234, unique_key_id: 0xab, sys_id: u64::MAX, existing_sys_id: 17 }, 13, [0xab_1234, u64::MAX, 17]; "uk conflict")]
#[test_case(CoreAccessError::SysIdOutOfRange { sys_id: u64::MAX }, 14, [u64::MAX, 0, 0]; "sys id exhausted")]
#[test_case(CoreAccessError::RecordValues(RecordValueError::PayloadTooShort { kind: 0x1234, required: 64, actual: 1 }), 15, [0x1234, 64, 1]; "record values short")]
#[test_case(CoreAccessError::RecordValues(RecordValueError::FieldLayout { kind: 0x1234, field_index: 0x5678_9abc, offset: 0x8765_4321, len: 0xfedc_ba98 }), 16, [0x5678_9abc_1234, 0xfedc_ba98_8765_4321, 0]; "record field layout")]
#[test_case(CoreAccessError::RecordValues(RecordValueError::MissingDefinition { kind: 0x1234, field_index: 0x5678_9abc }), 17, [0x5678_9abc_1234, 0, 0]; "record enum definition")]
#[test_case(CoreAccessError::RecordValues(RecordValueError::InvalidValue { kind: 0x1234, field_index: 0x5678_9abc, value: 255 }), 18, [0x5678_9abc_1234, 255, 0]; "record enum value")]
#[test_case(CoreAccessError::RecordValues(RecordValueError::FixedBytesLength { kind: 0x1234, field_index: 0x5678_9abc, len: 65535, capacity: 64 }), 19, [0x5678_9abc_1234, 65535, 64]; "record fixed bytes length")]
fn access_causes_preserve_fields_without_truncation(failure: CoreAccessError, cause: u32, detail: [u64; 3]) {
    let expected = SvStatus { tag: 3, cause, detail };

    assert_eq!(SvStatus::from(&failure), expected);
    assert_eq!(expected.decode_access(), Ok(failure));
    assert_eq!(expected.decode_business_rejection(), Err(CoreStatusDecodeError::UnexpectedTag { expected: 1, actual: 3 }));
}

#[test_case(1; "first")]
#[test_case(59999; "last")]
fn only_business_range_is_a_business_rejection(raw: u16) {
    let code = BusinessRejectCode::new(raw).unwrap();
    let expected = SvStatus { tag: 1, cause: u32::from(raw), detail: [0; 3] };

    assert_eq!(SvStatus::business_rejection(code), expected);
    assert_eq!(expected.decode_business_rejection(), Ok(code));
    assert_eq!(expected.decode_schema(), Err(CoreStatusDecodeError::UnexpectedTag { expected: 2, actual: 1 }));
}

#[test_case(0; "zero is success")]
#[test_case(60000; "first reserved")]
#[test_case(65535; "last reserved")]
#[test_case(65537; "must not narrow to valid one")]
#[test_case(u32::MAX; "maximum")]
fn invalid_business_code_is_not_narrowed(raw: u32) {
    let status = SvStatus { tag: 1, cause: raw, detail: [0; 3] };

    assert_eq!(status.decode_business_rejection(), Err(CoreStatusDecodeError::InvalidBusinessCode { raw }));
}

#[derive(Clone, Copy, Debug)]
enum Decoder { Business, Schema, Access }

fn decode(status: SvStatus, decoder: Decoder) -> Result<(), CoreStatusDecodeError> {
    match decoder {
        Decoder::Business => status.decode_business_rejection().map(|_| ()),
        Decoder::Schema => status.decode_schema().map(|_| ()),
        Decoder::Access => status.decode_access().map(|_| ()),
    }
}

#[test_case(Decoder::Schema, 2, 1, 0, 65535; "kind width")]
#[test_case(Decoder::Schema, 2, 2, 0, 0xffff_ffff; "schema field width")]
#[test_case(Decoder::Schema, 2, 3, 1, 255; "enum width")]
#[test_case(Decoder::Schema, 2, 6, 2, 255; "padding byte width")]
#[test_case(Decoder::Access, 3, 3, 1, 255; "uk width")]
#[test_case(Decoder::Access, 3, 7, 2, 0xffff_ffff; "immutable field width")]
#[test_case(Decoder::Access, 3, 11, 0, 0xffff_ffff_ffff; "packed kind field high bits")]
#[test_case(Decoder::Access, 3, 11, 2, 0xffff_ffff; "span data size width")]
#[test_case(Decoder::Access, 3, 13, 0, 0xff_ffff; "packed kind uk high bits")]
#[test_case(Decoder::Access, 3, 15, 0, 65535; "record kind width")]
#[test_case(Decoder::Access, 3, 16, 0, 0xffff_ffff_ffff; "record layout packed high bits")]
#[test_case(Decoder::Access, 3, 17, 0, 0xffff_ffff_ffff; "record definition packed high bits")]
#[test_case(Decoder::Access, 3, 18, 0, 0xffff_ffff_ffff; "record value packed high bits")]
#[test_case(Decoder::Access, 3, 18, 1, 255; "record enum byte width")]
#[test_case(Decoder::Access, 3, 19, 0, 0xffff_ffff_ffff; "record bytes packed high bits")]
#[test_case(Decoder::Access, 3, 19, 1, 65535; "record bytes length width")]
#[test_case(Decoder::Access, 3, 19, 2, 0xffff_ffff; "record bytes capacity width")]
fn checked_detail_width_precedes_narrowing(decoder: Decoder, tag: u32, cause: u32, slot: u8, maximum: u64) {
    let mut status = SvStatus { tag, cause, detail: [0; 3] };
    status.detail[usize::from(slot)] = maximum;
    assert_eq!(decode(status, decoder), Ok(()), "last representable value");

    status.detail[usize::from(slot)] = maximum + 1;
    assert_eq!(decode(status, decoder), Err(CoreStatusDecodeError::DetailOutOfRange { slot, value: maximum + 1, maximum }));
}

#[test_case(Decoder::Business, 1, 1, 0; "business slot zero")]
#[test_case(Decoder::Business, 1, 1, 1; "business slot one")]
#[test_case(Decoder::Business, 1, 1, 2; "business slot two")]
#[test_case(Decoder::Schema, 2, 1, 1; "schema kind spare")]
#[test_case(Decoder::Schema, 2, 3, 2; "schema enum spare")]
#[test_case(Decoder::Schema, 2, 7, 2; "schema trailing spare")]
#[test_case(Decoder::Access, 3, 2, 1; "access schema spare")]
#[test_case(Decoder::Access, 3, 5, 2; "access sysid spare")]
#[test_case(Decoder::Access, 3, 12, 2; "packed span spare")]
#[test_case(Decoder::Access, 3, 14, 1; "exhausted sysid first spare")]
#[test_case(Decoder::Access, 3, 14, 2; "exhausted sysid second spare")]
#[test_case(Decoder::Access, 3, 16, 2; "record layout spare")]
#[test_case(Decoder::Access, 3, 17, 1; "record definition first spare")]
#[test_case(Decoder::Access, 3, 17, 2; "record definition second spare")]
#[test_case(Decoder::Access, 3, 18, 2; "record value spare")]
fn unused_details_must_be_zero(decoder: Decoder, tag: u32, cause: u32, slot: u8) {
    let mut status = SvStatus { tag, cause, detail: [0; 3] };
    status.detail[usize::from(slot)] = 1;

    assert_eq!(decode(status, decoder), Err(CoreStatusDecodeError::NonzeroUnusedDetail { slot, value: 1 }));
}

#[test_case(Decoder::Schema, 2, 0; "zero schema cause")]
#[test_case(Decoder::Schema, 2, 8; "unknown schema cause")]
#[test_case(Decoder::Access, 3, 0; "zero access cause")]
#[test_case(Decoder::Access, 3, 20; "unknown access cause")]
fn unknown_causes_never_become_a_known_failure(decoder: Decoder, tag: u32, cause: u32) {
    assert_eq!(decode(SvStatus { tag, cause, detail: [0; 3] }, decoder), Err(CoreStatusDecodeError::UnknownCause { tag, cause }));
}

#[test_case(0; "zero storage cause")]
#[test_case(13; "unknown storage cause")]
#[test_case(u64::MAX; "wide storage cause")]
fn unknown_storage_categories_are_not_internal_fallbacks(raw: u64) {
    assert_eq!(SvStatus { tag: 3, cause: 1, detail: [raw, 0, 0] }.decode_access(), Err(CoreStatusDecodeError::UnknownStorageCause { raw }));
}

#[test_case(0; "success is not an error")]
#[test_case(4; "host mapping still separate")]
#[test_case(7; "panic is not a schema refusal")]
#[test_case(9; "abi contract is not an access refusal")]
#[test_case(10; "configuration is not transaction failure")]
#[test_case(u32::MAX; "unknown tag")]
fn other_categories_require_their_own_decoder(tag: u32) {
    let status = SvStatus { tag, cause: 1, detail: [0; 3] };
    for (decoder, expected) in [(Decoder::Business, 1), (Decoder::Schema, 2), (Decoder::Access, 3)] {
        assert_eq!(decode(status, decoder), Err(CoreStatusDecodeError::UnexpectedTag { expected, actual: tag }));
    }
}
