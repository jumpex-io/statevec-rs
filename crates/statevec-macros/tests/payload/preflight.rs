//! Same declarations as generated access, including fields a handler may not read.
use super::*;
use statevec_model::{CommandSchemaFailure as Failure, GeneratedCommandAccess};

// The generated check must not resolve Result to the caller's application alias.
type Result<T> = std::result::Result<T, ()>;

mod application_names {
    use statevec_macros::{EnumU8, command};
    use statevec_model::GeneratedCommandAccess;

    #[derive(EnumU8)]
    #[repr(u8)]
    enum Failure {
        Rejected = 1,
    }

    #[command(kind = 42)]
    struct ReportFailure {
        #[field(index = 1, enum_u8)]
        cause: Failure,
    }

    #[test]
    fn generated_preflight_does_not_shadow_the_applications_failure_type() {
        let payload = ReportFailure::builder().set_cause(Failure::Rejected).build().unwrap();
        assert_eq!(ReportFailure::wrap(&payload).cause(), Failure::Rejected);
        assert_eq!(ReportFailure::validate_payload(&payload), Ok(()));
        assert!(matches!(
            ReportFailure::validate_payload(&[99]),
            Err(statevec_model::CommandSchemaFailure::Enum { field: 1, .. })
        ));
    }
}

struct Runtime;
impl Runtime {
    fn ignored<Tx: statevec_api::TypedTxContext + ?Sized>(
        &self,
        _: &mut Tx,
        _: CheckedAccess<'_>,
    ) -> std::result::Result<(), statevec_api::RuntimeHostError>
    where
        statevec_api::RuntimeHostError: From<Tx::Error>,
    {
        panic!("static preflight must not invoke the business handler")
    }
}

statevec_macros::command_dispatch! {
    fn try_dispatch;
    runtime = Runtime;
    error = statevec_api::RuntimeHostError;
    Checked => Runtime::ignored
}

#[command(kind = 41)]
struct Checked {
    #[field(index = 1)]
    enabled: bool,
    #[field(index = 2, enum_u8)]
    status: FillStatus,
    #[field(index = 3)]
    tag: FixedBytes<4>,
    #[field(index = 4)]
    body: VarBytes,
    #[field(index = 5)]
    tail: u64,
}

fn payload() -> Vec<u8> {
    Checked::builder()
        .set_enabled(true)
        .set_status(FillStatus::Accepted)
        .set_tag(FixedBytes::new(b"AB").unwrap())
        .set_body(vec![7, 8, 9])
        .set_tail(37)
        .build()
        .unwrap()
}

#[test]
fn preflight_checks_every_boundary_and_requires_exact_consumption() {
    let payload = payload();
    assert_eq!(Runtime.preflight_try_dispatch(41, &payload), Ok(()));
    assert_eq!(Runtime.preflight_try_dispatch(40, &payload), Err(Failure::UnsupportedKind { kind: 40 }));
    assert_eq!(Checked::validate_payload(&payload), Ok(()));
    for cut in 0..payload.len() {
        assert!(
            matches!(Checked::validate_payload(&payload[..cut]), Err(Failure::FieldAccess { .. })),
            "must reject complete prefix before any generated getter: cut={cut}"
        );
    }
    let mut extra = payload.clone();
    extra.push(0);
    assert_eq!(
        Checked::validate_payload(&extra),
        Err(Failure::TrailingBytes { consumed: payload.len(), actual: extra.len() })
    );
    // Fixed-only layouts have the same exact length rule.
    let fixed = FixedCmd::builder().set_user_id(1).set_amount(2).set_flags(0).build().unwrap();
    assert_eq!(FixedCmd::validate_payload(&fixed), Ok(()));
    let mut extra = fixed.clone();
    extra.push(0);
    assert!(matches!(FixedCmd::validate_payload(&extra), Err(Failure::TrailingBytes { .. })));
}

#[test]
fn preflight_rejects_noncanonical_values_even_when_the_handler_would_ignore_them() {
    let mut bytes = payload();
    bytes[0] = 2;
    assert_eq!(Runtime.preflight_try_dispatch(41, &bytes), Err(Failure::Boolean { field: 1, raw: 2 }));
    assert_eq!(Checked::validate_payload(&bytes), Err(Failure::Boolean { field: 1, raw: 2 }));
    bytes[0] = 1;
    bytes[1] = 99;
    assert!(matches!(Checked::validate_payload(&bytes), Err(Failure::Enum { field: 2, .. })));
    bytes[1] = 1;
    bytes[2] = 5;
    assert_eq!(Checked::validate_payload(&bytes), Err(Failure::FixedBytesLength { field: 3, length: 5, capacity: 4 }));
    bytes[2] = 2;
    bytes[8..10].copy_from_slice(&u16::MAX.to_le_bytes());
    assert!(matches!(Checked::validate_payload(&bytes), Err(Failure::FieldAccess { field: 4, .. })));
}

#[test_case::test_case(b"", 0; "empty_first_padding_byte")]
#[test_case::test_case(b"A", 1; "short_first_padding_byte")]
#[test_case::test_case(b"A", 3; "short_last_padding_byte")]
#[test_case::test_case(b"ABC", 3; "only_padding_byte")]
fn preflight_rejects_nonzero_fixed_bytes_padding(logical: &[u8], offset: usize) {
    const TAG_LENGTH: usize = 2; // enabled + status
    const TAG_DATA: usize = TAG_LENGTH + 2;
    let mut bytes = payload();
    bytes[TAG_LENGTH..TAG_DATA].copy_from_slice(&(logical.len() as u16).to_le_bytes());
    bytes[TAG_DATA..TAG_DATA + 4].fill(0);
    bytes[TAG_DATA..TAG_DATA + logical.len()].copy_from_slice(logical);
    assert_eq!(Runtime.preflight_try_dispatch(41, &bytes), Ok(()));

    bytes[TAG_DATA + offset] = b'X';
    let original = bytes.clone();
    let expected = Err(Failure::FixedBytesPadding { field: 3, offset, raw: b'X' });
    assert_eq!(Runtime.preflight_try_dispatch(41, &bytes), expected, "dispatch preflight must reject padding");
    assert_eq!(Checked::validate_payload(&bytes), expected);
    assert_eq!(bytes, original, "validation must not normalize the original intent");

    bytes[TAG_DATA + offset] = 0;
    assert_eq!(Checked::validate_payload(&bytes), Ok(()));
    assert_eq!(Checked::wrap(&bytes).tag(), FixedBytes::<4>::new(logical).unwrap());
}

#[test_case::test_case(b""; "empty")]
#[test_case::test_case(b"A"; "short")]
#[test_case::test_case(b"A\0"; "logical_trailing_zero_is_not_padding")]
#[test_case::test_case(b"ABCDEFGH"; "full_capacity")]
fn preflight_preserves_canonical_fixed_bytes(logical: &[u8]) {
    let value = FixedBytes::<8>::new(logical).unwrap();
    let bytes = FixedBytesCmd::builder().set_symbol(value).set_venue_id(7).build().unwrap();
    assert_eq!(FixedBytesCmd::validate_payload(&bytes), Ok(()));
    assert_eq!(FixedBytesCmd::wrap(&bytes).symbol(), value);
}

#[command(kind = 43)]
struct FixedAfterBody {
    #[field(index = 1)]
    body: VarBytes,
    #[field(index = 2)]
    tag: FixedBytes<4>,
}

#[test]
fn padding_check_follows_variable_field_and_reports_first_invalid_byte() {
    let body = vec![0xff; 3];
    let tag_start = 2 + body.len() + 2; // body length + body + tag length
    let mut bytes = FixedAfterBody::builder()
        .set_body(body)
        .set_tag(FixedBytes::new(b"A").unwrap())
        .build()
        .unwrap();
    assert_eq!(FixedAfterBody::validate_payload(&bytes), Ok(()));

    bytes[tag_start + 1] = b'X';
    bytes[tag_start + 3] = b'Y';
    assert_eq!(
        FixedAfterBody::validate_payload(&bytes),
        Err(Failure::FixedBytesPadding { field: 2, offset: 1, raw: b'X' })
    );
}
