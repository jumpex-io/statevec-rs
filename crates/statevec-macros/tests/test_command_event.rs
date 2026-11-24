// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

//! Auxiliary tests for command and event macro expansion.

#![allow(dead_code)]

use statevec_macros::{command, event};
use statevec_model::command::Command;
use statevec_model::event::Event;
use statevec_model::{Decimal, FixedBytes};

#[path = "payload/checked_values.rs"]
mod checked_values;

#[path = "payload/preflight.rs"]
mod preflight;

// A placeholder type for VarBytes fields in the declaration struct.
// The macro recognizes the name "VarBytes" and generates Vec<u8> builder / &[u8] access.
struct VarBytes;

// Fixed-only command
#[command(kind = 10)]
struct FixedCmd {
    #[field(index = 1)]
    user_id: u64,
    #[field(index = 2)]
    amount: u32,
    #[field(index = 3)]
    flags: u8,
}

// VarBytes command: fixed before and after VarBytes
#[command(kind = 11)]
struct VarCmd {
    #[field(index = 1)]
    user_id: u64,
    #[field(index = 2)]
    note: VarBytes,
    #[field(index = 3)]
    amount: u32,
}

#[command(kind = 12)]
struct FixedBytesCmd {
    #[field(index = 1)]
    symbol: FixedBytes<8>,
    #[field(index = 2)]
    venue_id: u16,
}

#[command(kind = 14)]
struct WideFixedBytesCmd {
    #[field(index = 1)]
    body: FixedBytes<65_536>,
}

#[command(kind = 13)]
struct DecimalCmd {
    #[field(index = 1)]
    account_id: u64,
    #[field(index = 2)]
    amount: Decimal<6>,
}

// Event with fixed fields
#[event(kind = 20)]
struct OrderPlaced {
    #[field(index = 1)]
    order_id: u64,
    #[field(index = 2)]
    amount: u32,
}

#[event(kind = 21, inline_response = true)]
struct OrderAccepted {
    #[field(index = 1)]
    order_id: u64,
}

#[event(kind = 22)]
struct OrdersFilled {
    #[field(index = 1, repeated, max = 3)]
    order_ids: Vec<u64>,
    #[field(index = 2)]
    amount: u32,
}

#[derive(statevec_macros::EnumU8)]
#[repr(u8)]
enum FillStatus {
    Accepted = 1,
    Rejected = 2,
}

#[test]
fn variable_payload_rejects_every_truncated_prefix_including_the_fixed_tail() {
    use statevec_model::{AccessError, GeneratedCommandAccess};

    for note in [vec![], vec![7, 8, 9]] {
        let payload = VarCmd::builder()
            .set_user_id(42)
            .set_note(note.clone())
            .set_amount(17)
            .build()
            .unwrap();
        let access = VarCmdAccess::try_new(&payload).unwrap();
        assert_eq!((access.user_id(), access.note(), access.amount()), (42, note.as_slice(), 17));
        for cut in 0..payload.len() {
            assert!(
                VarCmdAccess::try_new(&payload[..cut]).is_err(),
                "variable payload accepted cut {cut}/{}",
                payload.len()
            );
        }
        let cut = payload.len() - 1;
        assert_eq!(
            VarCmdAccess::try_new(&payload[..cut]).err(),
            Some(AccessError { required: payload.len(), actual: cut })
        );
    }
}

#[test]
fn repeated_payload_rejects_every_truncated_prefix_including_the_fixed_tail() {
    use statevec_model::{AccessError, GeneratedEventAccess};

    for ids in [vec![], vec![11, 12, 13]] {
        let payload = OrdersFilled::builder()
            .set_order_ids(ids.clone())
            .set_amount(17)
            .build()
            .unwrap();
        let access = OrdersFilledAccess::try_new(&payload).unwrap();
        assert_eq!(access.order_ids().iter().collect::<Vec<_>>(), ids);
        assert_eq!(access.amount(), 17);
        for cut in 0..payload.len() {
            assert!(
                OrdersFilledAccess::try_new(&payload[..cut]).is_err(),
                "repeated payload accepted cut {cut}/{}",
                payload.len()
            );
        }
        let cut = payload.len() - 1;
        assert_eq!(
            OrdersFilledAccess::try_new(&payload[..cut]).err(),
            Some(AccessError { required: payload.len(), actual: cut })
        );
    }
}

#[event(kind = 23)]
struct StatusBatch {
    #[field(index = 1, repeated, max = 3, enum_u8)]
    statuses: Vec<FillStatus>,
}

#[test]
fn test_fixed_command_roundtrip() {
    use statevec_model::GeneratedCommandAccess;

    let payload = FixedCmd::builder()
        .set_user_id(42u64)
        .set_amount(100u32)
        .set_flags(7u8)
        .build()
        .expect("fixed command payload build");

    let access = FixedCmd::wrap(&payload);
    assert_eq!(access.user_id(), 42u64);
    assert_eq!(access.amount(), 100u32);
    assert_eq!(access.flags(), 7u8);
}

#[test]
fn test_command_builder_trait_and_missing_field_error() {
    use statevec_model::{CommandSchema, GeneratedCommandAccess, PayloadBuildError, StatevecCommandPayloadBuilder};

    assert_eq!(<FixedCmdBuilder as StatevecCommandPayloadBuilder>::COMMAND_KIND, FixedCmd::KIND);

    let payload = FixedCmd::builder()
        .set_user_id(42u64)
        .set_amount(100u32)
        .set_flags(7u8)
        .build_payload()
        .expect("fixed command payload build through trait");
    assert_eq!(FixedCmd::wrap(&payload).user_id(), 42u64);

    let err = FixedCmd::builder()
        .set_user_id(42u64)
        .set_amount(100u32)
        .build()
        .expect_err("missing command field should be typed error");
    assert_eq!(err, PayloadBuildError::MissingField("flags"));
}

#[test]
fn test_varbytes_command_roundtrip() {
    use statevec_model::GeneratedCommandAccess;

    let note_content = b"hello world";
    let payload = VarCmd::builder()
        .set_user_id(99u64)
        .set_note(note_content.to_vec())
        .set_amount(500u32)
        .build()
        .expect("varbytes command payload build");

    let access = VarCmd::wrap(&payload);
    assert_eq!(access.user_id(), 99u64);
    assert_eq!(access.note(), note_content.as_ref());
    assert_eq!(access.amount(), 500u32);
}

#[test]
fn test_varbytes_command_rejects_oversized_field() {
    use statevec_model::GeneratedCommandAccess;

    let oversized = vec![0u8; usize::from(u16::MAX) + 1];
    let err = VarCmd::builder()
        .set_user_id(99u64)
        .set_note(oversized)
        .set_amount(500u32)
        .build()
        .expect_err("oversized command VarBytes should be typed error");
    assert!(matches!(
        err,
        statevec_model::PayloadBuildError::VarBytesTooLong { field: "note", len } if len == usize::from(u16::MAX) + 1
    ));
}

#[test]
fn test_event_definition() {
    use statevec_model::EventSchema;

    let def = OrderPlaced::definition();
    assert_eq!(def.kind, 20);
    assert_eq!(def.name, "OrderPlaced");
    assert!(!def.inline_response);
    assert_eq!(def.fields.len(), 2);
    assert_eq!(def.fields[0].name, "order_id");
    assert_eq!(def.fields[1].name, "amount");

    let inline_def = OrderAccepted::definition();
    assert_eq!(inline_def.kind, 21);
    assert!(inline_def.inline_response);
}

#[test]
fn test_command_and_event_auto_traits_work() {
    let fixed = FixedCmd { user_id: 42, amount: 100, flags: 7 };
    let fixed_cloned = fixed.clone();
    assert_eq!(fixed_cloned.user_id, 42);
    assert_eq!(fixed_cloned.amount, 100);
    assert_eq!(fixed_cloned.flags, 7);
    assert_eq!(fixed, FixedCmd { user_id: 42, amount: 100, flags: 7 });
    assert!(format!("{:?}", fixed).contains("FixedCmd"));

    let var = VarCmd { user_id: 7, note: b"note-a".to_vec(), amount: 11 };
    let var_cloned = var.clone();
    assert_eq!(var_cloned.user_id, 7);
    assert_eq!(var_cloned.note, b"note-a".to_vec());
    assert_eq!(var_cloned.amount, 11);
    assert_eq!(var, VarCmd { user_id: 7, note: b"note-a".to_vec(), amount: 11 });
    assert!(format!("{:?}", var).contains("VarCmd"));

    let evt = OrderPlaced { order_id: 1, amount: 999 };
    let evt_cloned = evt.clone();
    assert_eq!(evt_cloned.order_id, 1);
    assert_eq!(evt_cloned.amount, 999);
    assert_eq!(evt, OrderPlaced { order_id: 1, amount: 999 });
    assert!(format!("{:?}", evt).contains("OrderPlaced"));
}

#[test]
fn test_fixedbytes_command_roundtrip() {
    use statevec_model::GeneratedCommandAccess;

    let symbol = FixedBytes::<8>::new(b"BTC").unwrap();
    let payload = FixedBytesCmd::builder()
        .set_symbol(symbol)
        .set_venue_id(7u16)
        .build()
        .expect("fixed-bytes command payload build");

    let access = FixedBytesCmd::wrap(&payload);
    assert_eq!(access.symbol(), symbol);
    assert_eq!(access.venue_id(), 7u16);
}

#[test]
fn fixedbytes_command_preserves_maximum_logical_length() {
    use statevec_model::GeneratedCommandAccess;

    let bytes = vec![0xa5; 65_535];
    let body = FixedBytes::<65_536>::new(&bytes).expect("maximum encoded length");
    let payload = WideFixedBytesCmd::builder().set_body(body).build().expect("command payload");
    let access = WideFixedBytesCmd::wrap(&payload);
    assert_eq!(access.body().len(), bytes.len());
    assert!(access.body().as_slice() == bytes, "generated access must preserve all logical bytes");
}

#[test]
fn test_decimal_command_roundtrip() {
    use statevec_model::{CommandSchema, GeneratedCommandAccess};

    let amount = Decimal::<6>::from_mantissa(-123_456_789);
    let payload = DecimalCmd::builder()
        .set_account_id(88)
        .set_amount(amount)
        .build()
        .expect("decimal command payload build");

    let access = DecimalCmd::wrap(&payload);
    assert_eq!(access.account_id(), 88);
    assert_eq!(access.amount(), amount);
    assert_eq!(access.amount().to_string(), "-123.456789");

    let def = DecimalCmd::definition();
    assert_eq!(def.fields[1].ty, statevec_model::FieldType::Decimal);
    assert_eq!(def.fields[1].decimal_scale, Some(6));
    assert_eq!(def.fields[1].fixed_size, Some(16));
}

#[test]
fn test_emit_typed_event() {
    use statevec_model::GeneratedEventAccess;

    let payload = OrderPlaced::builder()
        .set_order_id(1u64)
        .set_amount(999u32)
        .build()
        .expect("event payload build");

    assert!(!payload.is_empty());

    // Verify round-trip
    let access = OrderPlaced::wrap(&payload);
    assert_eq!(access.order_id(), 1u64);
    assert_eq!(access.amount(), 999u32);
}

#[test]
fn test_repeated_event_roundtrip_and_bounds() {
    use statevec_model::{EventSchema, GeneratedEventAccess};

    let payload = OrdersFilled::builder()
        .set_order_ids(vec![10, 11])
        .set_amount(999u32)
        .build()
        .expect("repeated event payload build");

    let access = OrdersFilled::wrap(&payload);
    let order_ids = access.order_ids();
    assert_eq!(order_ids.len(), 2);
    assert_eq!(order_ids.get(0), 10);
    assert_eq!(order_ids.get(1), 11);
    assert_eq!(order_ids.iter().collect::<Vec<_>>(), vec![10, 11]);
    assert_eq!(access.amount(), 999u32);

    let err = OrdersFilled::builder()
        .set_order_ids(vec![1, 2, 3, 4])
        .set_amount(1)
        .build()
        .expect_err("over-cap repeated event should be rejected");
    assert!(matches!(err, statevec_model::PayloadBuildError::RepeatedTooLong { field: "order_ids", len: 4, max: 3 }));

    let mut raw = Vec::new();
    raw.extend_from_slice(&4u16.to_le_bytes());
    for id in [1u64, 2, 3, 4] {
        raw.extend_from_slice(&id.to_le_bytes());
    }
    raw.extend_from_slice(&1u32.to_le_bytes());
    assert!(OrdersFilledAccess::try_new(&raw).is_err());

    let def = OrdersFilled::definition();
    assert!(def.fields[0].repeated);
    assert_eq!(def.fields[0].element_count_max, 3);
}

#[test]
fn test_repeated_enum_event_rejects_unknown_variant_in_try_new() {
    use statevec_model::GeneratedEventAccess;

    let payload = StatusBatch::builder()
        .set_statuses(vec![FillStatus::Accepted, FillStatus::Rejected])
        .build()
        .expect("repeated enum event payload build");
    let access = StatusBatch::wrap(&payload);
    assert_eq!(access.statuses().iter().collect::<Vec<_>>(), vec![FillStatus::Accepted, FillStatus::Rejected]);

    let raw = [2u8, 0, 1, 99];
    assert!(StatusBatchAccess::try_new(&raw).is_err());
}

#[test]
fn test_command_semantic_roundtrip_with_envelope() {
    let envelope = Command::new(9, 42, 1234, vec![1, 2, 3]);

    let semantic = envelope.clone();
    assert_eq!(semantic.command_kind(), 9);
    assert_eq!(semantic.ext_seq(), 42);
    assert_eq!(semantic.ref_ext_time_us(), 1234);
    assert_eq!(semantic.payload(), &[1, 2, 3]);

    let encoded = semantic;
    assert_eq!(encoded.command_kind(), 9);
    assert_eq!(encoded.ext_seq(), 42);
    assert_eq!(encoded.ref_ext_time_us(), 1234);
    assert_eq!(encoded.payload_len(), 3);
    assert_eq!(encoded.payload(), &[1, 2, 3]);
    assert_eq!(encoded.ingress_dedupe_key(), 42);
}

#[test]
fn test_event_semantic_roundtrip_with_frame() {
    let event = Event::new(20, 3, vec![4, 5, 6]);
    let frame = event.into_frame(99);
    let (tx_seq, semantic) = frame.into_parts();

    assert_eq!(tx_seq, 99);
    assert_eq!(semantic.event_kind(), 20);
    assert_eq!(semantic.event_seq(), 3);
    assert_eq!(semantic.payload(), &[4, 5, 6]);
}
