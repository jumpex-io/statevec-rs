// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

use crate::{
    AccessError, Decimal, DecimalParseError, DecimalRounding, FixedBytes, KeyBuilder, KeyBytes, UniqueKeyBytes,
    read_decimal_le, read_fixed_bytes, read_var_bytes, write_decimal_le, write_fixed_bytes,
};

#[test_case::test_case(8; "single_u64")]
#[test_case::test_case(16; "two_u64s")]
#[test_case::test_case(17; "first_spilled_length")]
#[test_case::test_case(32; "long_composite")]
#[test_case::test_case(64; "canonical_index_limit")]
#[test_case::test_case(65; "unique_key_is_not_index_limited")]
fn encoded_keys_preserve_bytes_order_and_lookup_across_inline_boundary(len: usize) {
    let expected: Vec<u8> = (0..len).map(|index| index as u8).collect();
    let mut builder = KeyBuilder::new();
    builder.push_bytes(&expected[..8]);
    builder.push_bytes(&expected[8..]);
    let encoded = builder.finish();
    assert_eq!(encoded.as_slice(), expected);
    if len <= 16 {
        assert!(!encoded.spilled(), "common short keys must not allocate");
    }

    let key = UniqueKeyBytes { uk_id: 2, bytes: encoded };
    let lookup = UniqueKeyBytes { uk_id: 2, bytes: KeyBytes::from_slice(&expected) };
    let mut index = std::collections::HashMap::new();
    index.insert(key.clone(), 91_u64);
    assert_eq!(index.get(&lookup), Some(&91), "independent encoding must find the same key");
    let mut successor = expected.clone();
    *successor.last_mut().unwrap() += 1;
    assert!(key < UniqueKeyBytes { uk_id: 2, bytes: KeyBytes::from_slice(&successor) });
}

#[test]
#[cfg(target_pointer_width = "64")]
fn short_key_containers_keep_the_measured_memory_budget() {
    assert!(std::mem::size_of::<KeyBytes>() <= 32);
    assert!(std::mem::size_of::<UniqueKeyBytes>() <= 40);
    assert!(std::mem::size_of::<smallvec::SmallVec<[UniqueKeyBytes; 3]>>() <= 136);
}

#[test_case::test_case(0; "empty")]
#[test_case::test_case(65_535; "largest_encoded_length")]
fn fixed_bytes_preserves_representable_lengths_with_larger_capacity(len: usize) {
    let bytes = vec![0xa5; len];
    let value = FixedBytes::<65_536>::new(&bytes).expect("representable logical length");
    assert_eq!(value.len(), len);
    assert!(value.as_slice() == bytes, "construction must preserve all logical bytes");
    assert!(value.padded_slice()[len..].iter().all(|byte| *byte == 0));

    let mut encoded = vec![0xff; 2 + 65_536];
    write_fixed_bytes(&mut encoded, 0, &value);
    assert_eq!(u16::from_le_bytes(encoded[..2].try_into().unwrap()) as usize, len);
    let decoded = read_fixed_bytes::<65_536>(&encoded, 0).expect("encoded value");
    assert_eq!(decoded.len(), len);
    assert!(decoded.as_slice() == bytes, "wire roundtrip must preserve all logical bytes");
}

#[test_case::test_case(65_536; "first_unrepresentable_length")]
#[test_case::test_case(65_537; "wrapped_nonzero_length")]
fn fixed_bytes_rejects_unrepresentable_logical_length(len: usize) {
    let bytes = vec![0xa5; len];
    let error = FixedBytes::<65_537>::new(&bytes)
        .err()
        .expect("a length above u16::MAX must not wrap into a different value");
    assert_eq!(error, AccessError { required: 65_535, actual: len });
}

#[test]
fn fixed_bytes_preserves_capacity_rejection_and_maximum_array_conversion() {
    let error = FixedBytes::<2>::new(b"abc").err().expect("capacity overflow");
    assert_eq!(error, AccessError { required: 2, actual: 3 });

    let bytes = [0xa5; 65_535];
    let value = FixedBytes::<65_536>::from(&bytes);
    assert_eq!(value.len(), bytes.len());
    assert!(value.as_slice() == bytes, "array conversion must preserve the maximum legal length");
    assert_eq!(value.padded_slice()[65_535], 0);
}

#[test]
fn read_var_bytes_rejects_overflowing_end_offset() {
    let err = read_var_bytes(&[], usize::MAX - 1).unwrap_err();
    assert_eq!(err.required, usize::MAX);
    assert_eq!(err.actual, 0);
}

#[test]
fn decimal_display_is_pinned() {
    assert_eq!(Decimal::<0>::from_mantissa(12345).to_string(), "12345");
    assert_eq!(Decimal::<2>::from_mantissa(12345).to_string(), "123.45");
    assert_eq!(Decimal::<2>::from_mantissa(-12345).to_string(), "-123.45");
    assert_eq!(Decimal::<6>::from_mantissa(123).to_string(), "0.000123");
    assert_eq!(Decimal::<4>::from_mantissa(1200).to_string(), "0.1200");
}

#[test]
fn decimal_parse_strict_literals() {
    assert_eq!("1.50".parse::<Decimal<2>>(), Ok(Decimal::<2>::from_mantissa(150)));
    assert_eq!("1.5".parse::<Decimal<2>>(), Ok(Decimal::<2>::from_mantissa(150)));
    assert_eq!("1".parse::<Decimal<2>>(), Ok(Decimal::<2>::from_mantissa(100)));
    assert_eq!("-0.01".parse::<Decimal<2>>(), Ok(Decimal::<2>::from_mantissa(-1)));
    assert_eq!("0.000150".parse::<Decimal<6>>(), Ok(Decimal::<6>::from_mantissa(150)));
    assert_eq!("123".parse::<Decimal<0>>(), Ok(Decimal::<0>::from_mantissa(123)));
}

#[test]
fn decimal_parse_rejects_non_canonical_or_lossy_literals() {
    assert_eq!("".parse::<Decimal<2>>(), Err(DecimalParseError::Empty));
    assert_eq!("-".parse::<Decimal<2>>(), Err(DecimalParseError::InvalidFormat));
    assert_eq!("+1.00".parse::<Decimal<2>>(), Err(DecimalParseError::InvalidFormat));
    assert_eq!(" 1.00".parse::<Decimal<2>>(), Err(DecimalParseError::InvalidFormat));
    assert_eq!("1.".parse::<Decimal<2>>(), Err(DecimalParseError::InvalidFormat));
    assert_eq!(".1".parse::<Decimal<2>>(), Err(DecimalParseError::InvalidFormat));
    assert_eq!("1,000.00".parse::<Decimal<2>>(), Err(DecimalParseError::InvalidFormat));
    assert_eq!("1e2".parse::<Decimal<2>>(), Err(DecimalParseError::InvalidFormat));
    assert_eq!("1.234".parse::<Decimal<2>>(), Err(DecimalParseError::TooManyFractionalDigits { scale: 2, actual: 3 }));
}

#[test]
fn decimal_parse_overflow_and_invalid_scale_fail_closed() {
    assert_eq!(i128::MAX.to_string().parse::<Decimal<0>>(), Ok(Decimal::<0>::from_mantissa(i128::MAX)));
    assert_eq!(i128::MIN.to_string().parse::<Decimal<0>>(), Ok(Decimal::<0>::from_mantissa(i128::MIN)));
    let overflow = format!("{}0", i128::MAX);
    assert_eq!(overflow.parse::<Decimal<0>>(), Err(DecimalParseError::Overflow));
    assert_eq!(
        "1".parse::<Decimal<39>>(),
        Err(DecimalParseError::InvalidScale { scale: 39, max: crate::MAX_DECIMAL_SCALE })
    );
}

#[test]
fn decimal_read_write_round_trips_raw_mantissa() {
    let value = Decimal::<6>::from_mantissa(-123_456_789);
    let mut buf = [0u8; 16];
    write_decimal_le::<6>(&mut buf, 0, value);

    let decoded = read_decimal_le::<6>(&buf, 0).unwrap();
    assert_eq!(decoded, value);
    assert_eq!(i128::from_le_bytes(buf), value.mantissa());
}

#[test]
fn decimal_checked_same_scale_arithmetic() {
    let lhs = Decimal::<2>::from_mantissa(150);
    let rhs = Decimal::<2>::from_mantissa(25);

    assert_eq!(lhs.checked_add(rhs), Some(Decimal::<2>::from_mantissa(175)));
    assert_eq!(lhs.checked_sub(rhs), Some(Decimal::<2>::from_mantissa(125)));
    assert_eq!(Decimal::<2>::from_mantissa(i128::MAX).checked_add(rhs), None);
}

#[test]
fn decimal_rescale_expands_scale_without_rounding() {
    let value = Decimal::<2>::from_mantissa(123);

    assert_eq!(value.rescale::<4>(DecimalRounding::Truncate), Some(Decimal::<4>::from_mantissa(12_300)));
    assert_eq!(
        Decimal::<0>::from_mantissa(-7).rescale::<3>(DecimalRounding::HalfEven),
        Some(Decimal::<3>::from_mantissa(-7000))
    );
}

#[test]
fn decimal_rescale_rounding_positive_values() {
    let low = Decimal::<2>::from_mantissa(124);
    let half_odd = Decimal::<2>::from_mantissa(125);
    let half_even = Decimal::<2>::from_mantissa(145);
    let high = Decimal::<2>::from_mantissa(126);

    assert_eq!(low.rescale::<1>(DecimalRounding::Truncate), Some(Decimal::<1>::from_mantissa(12)));
    assert_eq!(low.rescale::<1>(DecimalRounding::Floor), Some(Decimal::<1>::from_mantissa(12)));
    assert_eq!(low.rescale::<1>(DecimalRounding::Ceil), Some(Decimal::<1>::from_mantissa(13)));
    assert_eq!(low.rescale::<1>(DecimalRounding::HalfUp), Some(Decimal::<1>::from_mantissa(12)));
    assert_eq!(half_odd.rescale::<1>(DecimalRounding::HalfUp), Some(Decimal::<1>::from_mantissa(13)));
    assert_eq!(half_odd.rescale::<1>(DecimalRounding::HalfEven), Some(Decimal::<1>::from_mantissa(12)));
    assert_eq!(half_even.rescale::<1>(DecimalRounding::HalfEven), Some(Decimal::<1>::from_mantissa(14)));
    assert_eq!(high.rescale::<1>(DecimalRounding::HalfEven), Some(Decimal::<1>::from_mantissa(13)));
}

#[test]
fn decimal_rescale_rounding_negative_values() {
    let low = Decimal::<2>::from_mantissa(-124);
    let half_odd = Decimal::<2>::from_mantissa(-125);
    let half_even = Decimal::<2>::from_mantissa(-145);
    let high = Decimal::<2>::from_mantissa(-126);

    assert_eq!(low.rescale::<1>(DecimalRounding::Truncate), Some(Decimal::<1>::from_mantissa(-12)));
    assert_eq!(low.rescale::<1>(DecimalRounding::Floor), Some(Decimal::<1>::from_mantissa(-13)));
    assert_eq!(low.rescale::<1>(DecimalRounding::Ceil), Some(Decimal::<1>::from_mantissa(-12)));
    assert_eq!(low.rescale::<1>(DecimalRounding::HalfUp), Some(Decimal::<1>::from_mantissa(-12)));
    assert_eq!(half_odd.rescale::<1>(DecimalRounding::HalfUp), Some(Decimal::<1>::from_mantissa(-13)));
    assert_eq!(half_odd.rescale::<1>(DecimalRounding::HalfEven), Some(Decimal::<1>::from_mantissa(-12)));
    assert_eq!(half_even.rescale::<1>(DecimalRounding::HalfEven), Some(Decimal::<1>::from_mantissa(-14)));
    assert_eq!(high.rescale::<1>(DecimalRounding::HalfEven), Some(Decimal::<1>::from_mantissa(-13)));
}

#[test]
fn decimal_rescale_overflow_and_invalid_scale_fail_closed() {
    assert_eq!(Decimal::<0>::from_mantissa(i128::MAX).rescale::<1>(DecimalRounding::Truncate), None);
    assert_eq!(Decimal::<39>::from_mantissa(1).rescale::<2>(DecimalRounding::Truncate), None);
    assert_eq!(Decimal::<2>::from_mantissa(1).rescale::<39>(DecimalRounding::Truncate), None);
}
