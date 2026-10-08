// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

//! Decimal key bytes must agree across generated and IDL-imported encoders.

use statevec_macros::{EnumU8, record};
use statevec_model::{
    Decimal, EnumU8 as _, FixedBytes, GeneratedRecordAccess, RecordSchema, SchemaRegistry, Version,
};

#[derive(EnumU8)]
#[repr(u8)]
pub enum Side {
    Buy = 1,
    Sell = 2,
}

#[record(kind = 20, record_len = 128,
    uk(id = 0, fields = [symbol_id, side, price]),
    index(id = 0, name = "by_price", fields = [symbol_id, price]))]
pub struct PriceLevel {
    #[field(index = 1, immutable)]
    pub symbol_id: u64,
    #[field(index = 2, immutable, enum_u8)]
    pub side: Side,
    #[field(index = 3, immutable)]
    pub price: Decimal<8>,
    #[field(index = 4)]
    pub quantity: Decimal<8>,
}

#[record(kind = 21, record_len = 128,
    index(id = 0, name = "by_decimal", fields = [prefix, value]))]
pub struct IndexBoundary {
    #[field(index = 1)]
    pub prefix: FixedBytes<48>,
    #[field(index = 2)]
    pub value: Decimal<8>,
}

#[test]
fn decimal_composite_keys_preserve_full_signed_order_and_idl_equivalence() {
    let registry = SchemaRegistry::new(
        Version::new(1, 0),
        &[PriceLevel::definition().to_owned()],
        &[],
        &[],
        &[Side::DEFINITION.to_owned()],
    );
    let imported = SchemaRegistry::from_idl_json(&registry.to_idl_json()).unwrap();
    let values = [
        i128::MIN,
        i128::MIN + 1,
        -100_000_000,
        -1,
        0,
        1,
        100_000_000,
        i128::MAX - 1,
        i128::MAX,
    ];
    let mut previous = None;
    for mantissa in values {
        let value = Decimal::<8>::from_mantissa(mantissa);
        let mut bytes = vec![0; PriceLevel::DATA_LEN];
        PriceLevel::wrap_new(&mut bytes)
            .init_symbol_id(7)
            .init_side(Side::Buy)
            .init_price(value);
        let generated = PriceLevel::uk(7, Side::Buy, value);
        assert_eq!(generated.len(), 25);
        let mut expected = 7u64.to_be_bytes().to_vec();
        expected.push(1);
        expected.extend_from_slice(&((mantissa as u128) ^ (1u128 << 127)).to_be_bytes());
        assert_eq!(generated.as_slice(), expected);
        assert_eq!(
            registry.encode_uk(PriceLevel::KIND, &bytes),
            Some(generated.clone())
        );
        assert_eq!(
            imported.encode_uk(PriceLevel::KIND, &bytes),
            Some(generated.clone())
        );
        assert_eq!(
            imported.encode_canonical_index(PriceLevel::KIND, 0, &bytes),
            Some(PriceLevel::index_by_price(7, value))
        );
        if let Some(previous) = previous {
            assert!(previous < generated);
        }
        previous = Some(generated);
        assert!(imported.encode_uk(PriceLevel::KIND, &bytes[..24]).is_none());
    }
    assert!(
        PriceLevel::uk(7, Side::Buy, Decimal::from_mantissa(i128::MAX))
            < PriceLevel::uk(7, Side::Sell, Decimal::from_mantissa(i128::MIN))
    );
    assert!(
        PriceLevel::uk(7, Side::Sell, Decimal::from_mantissa(i128::MAX))
            < PriceLevel::uk(8, Side::Buy, Decimal::from_mantissa(i128::MIN))
    );
    assert_eq!(imported.identity(), registry.identity());
    assert_eq!(
        imported.try_get(PriceLevel::KIND).unwrap().fields[2].decimal_scale,
        Some(8)
    );
}

#[test]
fn same_scale_decimal_literals_have_one_key_representation() {
    let a = "1.5".parse::<Decimal<8>>().unwrap();
    let b = "1.50000000".parse::<Decimal<8>>().unwrap();
    assert_eq!(
        PriceLevel::uk(1, Side::Buy, a),
        PriceLevel::uk(1, Side::Buy, b)
    );
    let a = "-0".parse::<Decimal<8>>().unwrap();
    let b = "0.00000000".parse::<Decimal<8>>().unwrap();
    assert_eq!(
        PriceLevel::uk(1, Side::Buy, a),
        PriceLevel::uk(1, Side::Buy, b)
    );
}

#[test]
fn decimal_canonical_index_counts_all_sixteen_bytes_at_the_boundary() {
    let registry = SchemaRegistry::new(
        Version::new(1, 0),
        &[IndexBoundary::definition().to_owned()],
        &[],
        &[],
        &[],
    );
    let imported = SchemaRegistry::from_idl_json(&registry.to_idl_json()).unwrap();
    let prefix = FixedBytes::<48>::new(&[0x5a; 48]).unwrap();
    let value = Decimal::<8>::from_mantissa(-1);
    let mut data = vec![0; IndexBoundary::DATA_LEN];
    IndexBoundary::wrap_new(&mut data)
        .set_prefix(&prefix)
        .set_value(value);
    let generated = IndexBoundary::index_by_decimal(&prefix, value);
    assert_eq!(generated.len(), 64);
    assert_eq!(
        imported.encode_canonical_index(IndexBoundary::KIND, 0, &data),
        Some(generated)
    );
    assert!(
        imported
            .encode_canonical_index(IndexBoundary::KIND, 0, &data[..65])
            .is_none()
    );
}
