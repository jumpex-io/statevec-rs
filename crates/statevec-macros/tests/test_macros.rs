// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

//! Auxiliary tests for proc-macro expansion and compile-fail coverage.

use statevec_macros::{EnumU8, record};
use statevec_model::{Decimal, FixedBytes, GeneratedRecordAccess, RecordSchema, SchemaRegistry, UkCodec, Version};
use std::collections::HashMap;

#[test]
fn compile_fail() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/compile_fail/**/*.rs");
}

#[derive(EnumU8)]
#[repr(u8)]
pub enum OrderStatus {
    Init = 0,
    Open = 1,
    Filled = 2,
    Cancelled = 3,
}

#[record(kind = 2, record_len = 64, uk(id = 0, fields = [order_id]))]
pub struct Order {
    #[field(index = 1, immutable = true)]
    pub order_id: u64,

    #[field(index = 2)]
    pub account_id: u64,

    #[field(index = 3, enum_u8)]
    pub status: OrderStatus,

    #[field(index = 4)]
    pub client_id: FixedBytes<16>,
}

#[record(kind = 61439, record_len = 64)]
pub struct MaxUserKindRecord {
    #[field(index = 1)]
    pub value: u64,
}

#[record(kind = 6, record_len = 32768)]
pub struct MaxLengthRecord {
    #[field(index = 1)]
    pub value: FixedBytes<32742>,
}

#[test]
fn maximum_record_length_preserves_complete_layout_and_access() {
    assert_eq!(MaxLengthRecord::RECORD_LEN, 32768);
    assert_eq!(MaxLengthRecord::RECORD_LEN, statevec_model::MAX_RECORD_LEN);
    assert_eq!(MaxLengthRecord::definition().data_size as usize + statevec_model::RECORD_HEADER_SIZE, 32768);
    let value = FixedBytes::<32742>::new(&[7; 32742]).unwrap();
    let mut data = vec![0; MaxLengthRecord::DATA_LEN];
    MaxLengthRecord::wrap_new(&mut data).set_value(&value);
    assert_eq!(MaxLengthRecordAccess::new(&data).value(), value);
}

#[record(kind = 5, record_len = 64)]
pub struct DecimalAccount {
    #[field(index = 1)]
    pub account_id: u64,

    #[field(index = 2)]
    pub balance: Decimal<2>,
}

#[test]
fn single_uk() {
    let mut buf = [0u8; OrderAccess::LEN];
    let client_id = FixedBytes::<16>::new(b"abc-001").unwrap();

    let mut new_builder = <Order as GeneratedRecordAccess>::wrap_new(&mut buf);
    new_builder.init_order_id(1001);
    new_builder.set_account_id(42);
    new_builder.set_status(OrderStatus::Open);
    new_builder.set_client_id(&client_id);

    let mut update_builder = <Order as GeneratedRecordAccess>::wrap_update(&mut buf);
    update_builder.set_account_id(43);
    update_builder.set_status(OrderStatus::Filled);

    let ro = OrderAccess::new(&buf);
    assert_eq!(ro.order_id(), 1001);
    assert_eq!(ro.account_id(), 43);
    assert_eq!(ro.status(), OrderStatus::Filled);
    assert_eq!(ro.client_id().as_slice(), b"abc-001");

    let uk = <Order as UkCodec>::encode_uk_from_bytes(&buf);
    assert_eq!(uk.len(), 8);
    assert_eq!(uk, Order::uk(1001));

    // offset consts: data-relative (no header prefix), fields packed from 0
    assert_eq!(OrderAccess::ORDER_ID_OFFSET, 0);
    assert_eq!(OrderAccess::ACCOUNT_ID_OFFSET, 8);
    assert_eq!(OrderAccess::STATUS_OFFSET, 16);
    assert_eq!(OrderAccess::CLIENT_ID_OFFSET, 17);

    let def = Order::definition();
    assert_eq!(def.kind, 2);
    assert_eq!(def.data_size, 40);
    assert_eq!(def.fields.len(), 4);
}

#[test]
fn max_user_record_kind_is_allowed() {
    assert_eq!(MaxUserKindRecord::KIND, statevec_model::USER_KIND_MAX);
}

#[test]
fn decimal_record_roundtrip_and_schema_metadata() {
    let mut buf = [0u8; DecimalAccountAccess::LEN];
    let mut builder = <DecimalAccount as GeneratedRecordAccess>::wrap_new(&mut buf);
    builder.set_account_id(7);
    builder.set_balance(Decimal::<2>::from_mantissa(12345));

    let access = DecimalAccountAccess::new(&buf);
    assert_eq!(access.account_id(), 7);
    assert_eq!(access.balance(), Decimal::<2>::from_mantissa(12345));
    assert_eq!(access.balance().to_string(), "123.45");

    let def = DecimalAccount::definition();
    assert_eq!(def.fields[1].ty, statevec_model::FieldType::Decimal);
    assert_eq!(def.fields[1].len, 16);
    assert_eq!(def.fields[1].decimal_scale, Some(2));
    assert_eq!(def.fields[1].rust_type_name, "Decimal < 2 >");
}

#[test]
#[allow(clippy::clone_on_copy)]
fn enum_u8_auto_traits_work() {
    let status = OrderStatus::Open;
    let copied = status;
    let cloned = status.clone();
    assert_eq!(copied, OrderStatus::Open);
    assert_eq!(cloned, OrderStatus::Open);
    assert_eq!(format!("{:?}", status), "Open");

    let mut map = HashMap::new();
    map.insert(OrderStatus::Init, "init");
    map.insert(OrderStatus::Open, "open");
    assert_eq!(map.get(&OrderStatus::Init), Some(&"init"));
    assert_eq!(map.get(&OrderStatus::Open), Some(&"open"));
}

#[test]
fn record_auto_traits_work() {
    let order = Order {
        order_id: 7,
        account_id: 42,
        status: OrderStatus::Filled,
        client_id: FixedBytes::<16>::new(b"abc-001").unwrap(),
    };
    let cloned = order.clone();
    let debug = format!("{:?}", order);
    let same = Order {
        order_id: 7,
        account_id: 42,
        status: OrderStatus::Filled,
        client_id: FixedBytes::<16>::new(b"abc-001").unwrap(),
    };

    assert_eq!(cloned.order_id, 7);
    assert_eq!(cloned.account_id, 42);
    assert_eq!(cloned.status, OrderStatus::Filled);
    assert_eq!(cloned.client_id.as_slice(), b"abc-001");
    assert_eq!(order, same);
    assert!(debug.contains("Order"));
}

#[record(kind = 3, record_len = 64, uk(id = 0, fields = [account_id, ccy]))]
pub struct AccountCcyStat {
    #[field(index = 1, immutable = true)]
    pub account_id: u64,

    #[field(index = 2, immutable = true)]
    pub ccy: FixedBytes<4>,

    #[field(index = 3)]
    pub balance: u64,

    #[field(index = 4)]
    pub nonce: u8,
}

#[record(kind = 1, record_len = 128, uk(id = 0, fields = [a,b]))]
pub struct PKWithFixedBytes {
    #[field(index = 1, immutable = true)]
    pub a: u64,
    #[field(index = 2, immutable = true)]
    pub b: FixedBytes<32>,
}

#[test]
fn test_composite_uk() {
    let mut buf = [0u8; AccountCcyStatAccess::LEN];
    let ccy = FixedBytes::from(b"USD");
    let mut builder = <AccountCcyStat as GeneratedRecordAccess>::wrap_new(&mut buf);
    builder.init_account_id(998);
    builder.init_ccy(&ccy);
    builder.set_balance(1_000_000);

    let ro = AccountCcyStatAccess::new(&buf);
    assert_eq!(ro.account_id(), 998);
    assert_eq!(ro.ccy(), FixedBytes::from(b"USD"));
    assert_eq!(ro.balance(), 1_000_000);

    let uk = <AccountCcyStat as UkCodec>::encode_uk_from_bytes(&buf);
    assert_eq!(uk.len(), 12);
    assert_eq!(uk, AccountCcyStat::uk(998, &ccy));
    assert_eq!(ccy.key_bytes().as_slice(), b"USD\0");
    assert_eq!(&uk[8..12], b"USD\0");

    // data-relative offsets
    assert_eq!(AccountCcyStatAccess::ACCOUNT_ID_OFFSET, 0);
    assert_eq!(AccountCcyStatAccess::CCY_OFFSET, 8);
    assert_eq!(AccountCcyStatAccess::BALANCE_OFFSET, 14);
}

#[record(kind = 10, record_len = 64)]
pub struct Asset {
    #[field(index = 1)]
    pub id: u64,

    #[field(index = 2, reserved)]
    pub _reserved: u32,

    #[field(index = 3)]
    pub name: FixedBytes<16>,
}

#[test]
fn reserved_field() {
    let mut buf = [0u8; AssetAccess::LEN];
    let name = FixedBytes::<16>::new(b"BTC").unwrap();

    let mut builder = <Asset as GeneratedRecordAccess>::wrap_new(&mut buf);
    builder.set_id(1);
    builder.set_name(&name);

    let ro = AssetAccess::new(&buf);
    assert_eq!(ro.id(), 1);
    assert_eq!(ro.name().as_slice(), b"BTC");

    // reserved field occupies space: id(8) + _reserved(4) = 12, so name starts at 12
    assert_eq!(AssetAccess::ID_OFFSET, 0);
    assert_eq!(AssetAccess::NAME_OFFSET, 12);

    // FIELD_COUNT and definition exclude reserved
    let def = Asset::definition();
    assert_eq!(def.fields.len(), 2);
    assert_eq!(<Asset as RecordSchema>::FIELD_COUNT, 2);
}

#[record(
    kind = 11,
    record_len = 128,
    uk(id = 0, name = "by_account_client", fields = [account_id, client_id]),
    uk(id = 1, name = "by_external_id", fields = [external_id]),
    uk(id = 2, name = "by_account_external", fields = [account_id, external_id])
)]
pub struct MultiUkOrder {
    #[field(index = 1, immutable)]
    pub account_id: u64,

    #[field(index = 2, immutable)]
    pub client_id: FixedBytes<16>,

    #[field(index = 3, immutable)]
    pub external_id: u64,

    #[field(index = 4)]
    pub amount: u64,
}

#[test]
fn multiple_named_uk_definitions_generate_stable_metadata_and_helpers() {
    let mut buf = [0u8; MultiUkOrderAccess::LEN];
    let client_id = FixedBytes::<16>::new(b"client-1").unwrap();
    let mut builder = <MultiUkOrder as GeneratedRecordAccess>::wrap_new(&mut buf);
    builder.init_account_id(42);
    builder.init_client_id(&client_id);
    builder.init_external_id(9001);
    builder.set_amount(100);

    let def = MultiUkOrder::definition();
    assert_eq!(def.unique_keys.len(), 3);
    assert_eq!(def.unique_keys[0].id, 0);
    assert_eq!(def.unique_keys[0].name, "by_account_client");
    assert_eq!(def.unique_keys[0].fields, &["account_id", "client_id"]);
    assert_eq!(def.unique_keys[1].id, 1);
    assert_eq!(def.unique_keys[1].name, "by_external_id");
    assert_eq!(def.unique_keys[1].fields, &["external_id"]);
    assert_eq!(def.unique_keys[2].id, 2);
    assert_eq!(def.unique_keys[2].name, "by_account_external");
    assert_eq!(def.unique_keys[2].fields, &["account_id", "external_id"]);
    assert_eq!(MultiUkOrder::BY_ACCOUNT_CLIENT_UK_ID, 0);
    assert_eq!(MultiUkOrder::BY_EXTERNAL_ID_UK_ID, 1);
    assert_eq!(MultiUkOrder::BY_ACCOUNT_EXTERNAL_UK_ID, 2);

    let first = <MultiUkOrder as UkCodec>::encode_uk_from_bytes(&buf);
    assert_eq!(first, MultiUkOrder::uk(42, &client_id));
    assert_eq!(first, MultiUkOrder::uk_by_account_client(42, &client_id));
    assert_eq!(MultiUkOrder::uk_by_external_id(9001).as_slice(), 9001u64.to_be_bytes().as_slice());
    assert_eq!(MultiUkOrder::uk_by_account_external(42, 9001).len(), 16);
}

#[record(
    kind = 12,
    record_len = 128,
    uk(id = 0, fields = [order_id]),
    index(id = 0, name = "by_account_status_created", fields = [account_id, status, created_seq]),
    index(id = 1, name = "by_market_client", fields = [market, client_id])
)]
pub struct IndexedOrder {
    #[field(index = 1, immutable)]
    pub order_id: u64,

    #[field(index = 2)]
    pub account_id: u64,

    #[field(index = 3, enum_u8)]
    pub status: OrderStatus,

    #[field(index = 4)]
    pub created_seq: u64,

    #[field(index = 5)]
    pub market: FixedBytes<4>,

    #[field(index = 6)]
    pub client_id: FixedBytes<8>,
}

#[test]
fn canonical_indexes_generate_metadata_and_key_helpers() {
    let mut buf = [0u8; IndexedOrderAccess::LEN];
    let market = FixedBytes::<4>::new(b"BTC").unwrap();
    let client_id = FixedBytes::<8>::new(b"client").unwrap();
    let mut builder = <IndexedOrder as GeneratedRecordAccess>::wrap_new(&mut buf);
    builder.init_order_id(7001);
    builder.set_account_id(42);
    builder.set_status(OrderStatus::Open);
    builder.set_created_seq(123);
    builder.set_market(&market);
    builder.set_client_id(&client_id);

    let def = IndexedOrder::definition();
    assert_eq!(def.canonical_indexes.len(), 2);
    assert_eq!(def.canonical_indexes[0].id, 0);
    assert_eq!(def.canonical_indexes[0].name, "by_account_status_created");
    assert_eq!(def.canonical_indexes[0].fields, &["account_id", "status", "created_seq"]);
    assert_eq!(def.canonical_indexes[1].id, 1);
    assert_eq!(def.canonical_indexes[1].name, "by_market_client");
    assert_eq!(def.canonical_indexes[1].fields, &["market", "client_id"]);
    assert_eq!(IndexedOrder::BY_ACCOUNT_STATUS_CREATED_INDEX_ID, 0);
    assert_eq!(IndexedOrder::BY_MARKET_CLIENT_INDEX_ID, 1);

    let full = IndexedOrder::index_by_account_status_created(42, OrderStatus::Open, 123);
    assert_eq!(full.len(), 17);
    assert_eq!(IndexedOrder::index_by_account_status_created_prefix1(42).len(), 8);
    assert_eq!(IndexedOrder::index_by_account_status_created_prefix2(42, OrderStatus::Open).len(), 9);
    assert_eq!(IndexedOrder::index_by_market_client(&market, &client_id).len(), 12);

    let registry = SchemaRegistry::with_records(Version::new(1, 0), &[*IndexedOrder::definition()]);
    assert_eq!(registry.encode_canonical_index(IndexedOrder::KIND, 0, &buf).unwrap(), full);
    assert_eq!(
        registry.encode_canonical_index(IndexedOrder::KIND, 1, &buf).unwrap(),
        IndexedOrder::index_by_market_client(&market, &client_id)
    );
}
