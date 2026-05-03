// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

//! Auxiliary tests for proc-macro expansion and compile-fail coverage.

use statevec_macros::{EnumU8, record};
use statevec_model::{FixedBytes, GeneratedRecordAccess, PkCodec, RecordSchema};
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

#[record(kind = 2, record_len = 64, pk(fields = [order_id]))]
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

#[test]
fn single_pk() {
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

    let pk = <Order as PkCodec>::encode_pk_from_bytes(&buf);
    assert_eq!(pk.len(), 8);
    assert_eq!(pk, Order::pk(1001));

    // offset consts: data-relative (no header prefix), fields packed from 0
    assert_eq!(OrderAccess::ORDER_ID_OFFSET, 0);
    assert_eq!(OrderAccess::ACCOUNT_ID_OFFSET, 8);
    assert_eq!(OrderAccess::STATUS_OFFSET, 16);
    assert_eq!(OrderAccess::CLIENT_ID_OFFSET, 17);

    let def = Order::definition();
    assert_eq!(def.kind, 2);
    assert_eq!(def.data_size, 48);
    assert_eq!(def.fields.len(), 4);
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

#[record(kind = 3, record_len = 64, pk(fields = [account_id, ccy]))]
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

#[record(kind = 1, record_len = 128, pk(fields = [a,b]))]
pub struct PKWithFixedBytes {
    #[field(index = 1, immutable = true)]
    pub a: u64,
    #[field(index = 2, immutable = true)]
    pub b: FixedBytes<32>,
}

#[test]
fn test_composite_pk() {
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

    let pk = <AccountCcyStat as PkCodec>::encode_pk_from_bytes(&buf);
    assert_eq!(pk.len(), 12);
    assert_eq!(pk, AccountCcyStat::pk(998, &ccy));
    assert_eq!(ccy.pk_bytes().as_slice(), b"USD\0");
    assert_eq!(&pk[8..12], b"USD\0");

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
