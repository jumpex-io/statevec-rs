// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

//! Auxiliary tests for command and event macro expansion.

#![allow(dead_code)]

use statevec_macros::{command, event};
use statevec_model::FixedBytes;
use statevec_model::command::Command;
use statevec_model::event::Event;

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

// Event with fixed fields
#[event(kind = 20)]
struct OrderPlaced {
    #[field(index = 1)]
    order_id: u64,
    #[field(index = 2)]
    amount: u32,
}

#[test]
fn test_fixed_command_roundtrip() {
    use statevec_model::GeneratedCommandAccess;

    let payload = FixedCmd::builder()
        .set_user_id(42u64)
        .set_amount(100u32)
        .set_flags(7u8)
        .build();

    let access = FixedCmd::wrap(&payload);
    assert_eq!(access.user_id(), 42u64);
    assert_eq!(access.amount(), 100u32);
    assert_eq!(access.flags(), 7u8);
}

#[test]
fn test_varbytes_command_roundtrip() {
    use statevec_model::GeneratedCommandAccess;

    let note_content = b"hello world";
    let payload = VarCmd::builder()
        .set_user_id(99u64)
        .set_note(note_content.to_vec())
        .set_amount(500u32)
        .build();

    let access = VarCmd::wrap(&payload);
    assert_eq!(access.user_id(), 99u64);
    assert_eq!(access.note(), note_content.as_ref());
    assert_eq!(access.amount(), 500u32);
}

#[test]
fn test_event_definition() {
    use statevec_model::EventSchema;

    let def = OrderPlaced::definition();
    assert_eq!(def.kind, 20);
    assert_eq!(def.name, "OrderPlaced");
    assert_eq!(def.fields.len(), 2);
    assert_eq!(def.fields[0].name, "order_id");
    assert_eq!(def.fields[1].name, "amount");
}

#[test]
fn test_command_and_event_auto_traits_work() {
    let fixed = FixedCmd {
        user_id: 42,
        amount: 100,
        flags: 7,
    };
    let fixed_cloned = fixed.clone();
    assert_eq!(fixed_cloned.user_id, 42);
    assert_eq!(fixed_cloned.amount, 100);
    assert_eq!(fixed_cloned.flags, 7);
    assert_eq!(
        fixed,
        FixedCmd {
            user_id: 42,
            amount: 100,
            flags: 7
        }
    );
    assert!(format!("{:?}", fixed).contains("FixedCmd"));

    let var = VarCmd {
        user_id: 7,
        note: b"note-a".to_vec(),
        amount: 11,
    };
    let var_cloned = var.clone();
    assert_eq!(var_cloned.user_id, 7);
    assert_eq!(var_cloned.note, b"note-a".to_vec());
    assert_eq!(var_cloned.amount, 11);
    assert_eq!(
        var,
        VarCmd {
            user_id: 7,
            note: b"note-a".to_vec(),
            amount: 11
        }
    );
    assert!(format!("{:?}", var).contains("VarCmd"));

    let evt = OrderPlaced {
        order_id: 1,
        amount: 999,
    };
    let evt_cloned = evt.clone();
    assert_eq!(evt_cloned.order_id, 1);
    assert_eq!(evt_cloned.amount, 999);
    assert_eq!(
        evt,
        OrderPlaced {
            order_id: 1,
            amount: 999
        }
    );
    assert!(format!("{:?}", evt).contains("OrderPlaced"));
}

#[test]
fn test_fixedbytes_command_roundtrip() {
    use statevec_model::GeneratedCommandAccess;

    let symbol = FixedBytes::<8>::new(b"BTC").unwrap();
    let payload = FixedBytesCmd::builder()
        .set_symbol(symbol)
        .set_venue_id(7u16)
        .build();

    let access = FixedBytesCmd::wrap(&payload);
    assert_eq!(access.symbol(), symbol);
    assert_eq!(access.venue_id(), 7u16);
}

#[test]
fn test_emit_typed_event() {
    use statevec_model::GeneratedEventAccess;

    let payload = OrderPlaced::builder()
        .set_order_id(1u64)
        .set_amount(999u32)
        .build();

    assert!(!payload.is_empty());

    // Verify round-trip
    let access = OrderPlaced::wrap(&payload);
    assert_eq!(access.order_id(), 1u64);
    assert_eq!(access.amount(), 999u32);
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
