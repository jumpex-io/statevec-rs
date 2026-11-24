use super::*;
use statevec_model::{EnumU8, GeneratedCommandAccess, GeneratedEventAccess};
use test_case::test_case;

#[command(kind = 31)]
struct EnumHead {
    #[field(index = 1, enum_u8)]
    status: FillStatus,
    #[field(index = 2)]
    body: VarBytes,
}

#[event(kind = 32)]
struct EnumTail {
    #[field(index = 1)]
    body: VarBytes,
    #[field(index = 2, enum_u8)]
    status: FillStatus,
}

#[command(kind = 33)]
struct BytesHead {
    #[field(index = 1)]
    symbol: FixedBytes<4>,
    #[field(index = 2)]
    body: VarBytes,
}

#[event(kind = 34)]
struct BytesTail {
    #[field(index = 1)]
    body: VarBytes,
    #[field(index = 2)]
    symbol: FixedBytes<4>,
}

#[statevec_macros::record(kind = 35, record_len = 64)]
struct CheckedRecord {
    #[field(index = 1, enum_u8)]
    status: FillStatus,
    #[field(index = 2)]
    symbol: FixedBytes<4>,
}

#[test_case(true; "record_enum_value")]
#[test_case(false; "record_fixed_bytes_length")]
fn readonly_record_checked_constructor_also_rejects_invalid_values(invalid_enum: bool) {
    let mut bytes = vec![0; CheckedRecordAccess::LEN];
    bytes[0] = if invalid_enum { 99 } else { 1 };
    bytes[1] = if invalid_enum { 0 } else { 5 };
    assert_eq!(
        CheckedRecordAccess::try_new(&bytes).err(),
        Some(if invalid_enum {
            statevec_model::AccessError { required: 0, actual: 99 }
        } else {
            statevec_model::AccessError { required: 5, actual: 4 }
        }),
        "readonly record try_new must validate getter inputs"
    );

    bytes[0] = 1;
    bytes[1] = 4;
    bytes[3..7].copy_from_slice(b"ABCD");
    let view = CheckedRecordAccess::try_new(&bytes).unwrap();
    assert_eq!(view.status(), FillStatus::Accepted);
    assert_eq!(view.status_raw(), 1);
    assert_eq!(view.symbol().as_slice(), b"ABCD");
}

#[test_case(true; "enum_before_variable_field")]
#[test_case(false; "enum_after_variable_field")]
fn checked_constructor_rejects_complete_payload_with_invalid_enum(head: bool) {
    // Length-valid, value-invalid: the original constructors accepted these
    // exact bytes and panicked only when status() was called.
    let error =
        if head { EnumHeadAccess::try_new(&[99, 0, 0]).err() } else { EnumTailAccess::try_new(&[0, 0, 99]).err() };
    assert_eq!(
        error,
        Some(statevec_model::AccessError { required: 0, actual: 99 }),
        "a checked constructor cannot defer invalid enum rejection to its getter"
    );
}

#[test_case(true; "fixed_bytes_before_variable_field")]
#[test_case(false; "fixed_bytes_after_variable_field")]
fn checked_constructor_rejects_complete_payload_with_oversized_fixed_bytes(head: bool) {
    let error = if head {
        BytesHeadAccess::try_new(&[5, 0, 1, 2, 3, 4, 0, 0]).err()
    } else {
        BytesTailAccess::try_new(&[0, 0, 5, 0, 1, 2, 3, 4]).err()
    };
    assert_eq!(
        error,
        Some(statevec_model::AccessError { required: 5, actual: 4 }),
        "a checked constructor cannot admit a FixedBytes length above its capacity"
    );
}

#[test_case(&[]; "empty_variable_field")]
#[test_case(&[7, 8, 9]; "nonempty_variable_field")]
fn legal_values_read_all_getters_and_every_truncated_prefix_is_rejected(body: &[u8]) {
    for status in [FillStatus::Accepted, FillStatus::Rejected] {
        let raw = status.to_u8();
        let bytes = EnumHead::builder().set_status(status).set_body(body.to_vec()).build().unwrap();
        let view = EnumHeadAccess::try_new(&bytes).unwrap();
        assert_eq!(view.status().to_u8(), raw);
        assert_eq!(view.body(), body);
        for cut in 0..bytes.len() {
            assert!(EnumHeadAccess::try_new(&bytes[..cut]).is_err(), "head enum cut={cut}");
        }
        let bytes = EnumTail::builder()
            .set_status(FillStatus::try_from_u8(raw).unwrap())
            .set_body(body.to_vec())
            .build()
            .unwrap();
        let view = EnumTailAccess::try_new(&bytes).unwrap();
        assert_eq!(view.status().to_u8(), raw);
        assert_eq!(view.body(), body);
        for cut in 0..bytes.len() {
            assert!(EnumTailAccess::try_new(&bytes[..cut]).is_err(), "tail enum cut={cut}");
        }
    }
    for symbol in [&[][..], &[1, 2, 3, 4][..]] {
        let bytes = BytesHead::builder()
            .set_symbol(FixedBytes::new(symbol).unwrap())
            .set_body(body.to_vec())
            .build()
            .unwrap();
        let view = BytesHeadAccess::try_new(&bytes).unwrap();
        assert_eq!(view.symbol().as_slice(), symbol);
        assert_eq!(view.body(), body);
        for cut in 0..bytes.len() {
            assert!(BytesHeadAccess::try_new(&bytes[..cut]).is_err(), "head bytes cut={cut}");
        }
        let bytes = BytesTail::builder()
            .set_symbol(FixedBytes::new(symbol).unwrap())
            .set_body(body.to_vec())
            .build()
            .unwrap();
        let view = BytesTailAccess::try_new(&bytes).unwrap();
        assert_eq!(view.symbol().as_slice(), symbol);
        assert_eq!(view.body(), body);
        for cut in 0..bytes.len() {
            assert!(BytesTailAccess::try_new(&bytes[..cut]).is_err(), "tail bytes cut={cut}");
        }
    }
}
