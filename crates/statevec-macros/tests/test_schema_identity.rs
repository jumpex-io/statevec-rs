// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

//! Identity follows encoded schema facts, not Rust path spelling or declaration order.
use statevec_macros::{EnumU8, command, event, record, schema_module};
use statevec_model::{
    CommandSchema, EnumU8 as _, EventSchema, FixedBytes, GeneratedCommandAccess, RecordSchema, SchemaRegistry,
};

#[schema_module(version = "1.2")]
mod plain {
    use super::*;

    #[derive(EnumU8)]
    #[repr(u8)]
    pub enum Status {
        Open = 1,
        Closed = 2,
    }

    #[record(kind = 1, record_len = 64, uk(id = 0, fields = [id]))]
    pub struct Item {
        #[field(index = 1, immutable)]
        pub id: u64,
        #[field(index = 2)]
        pub label: FixedBytes<16>,
        #[field(index = 3, enum_u8)]
        pub status: Status,
    }

    #[command(kind = 1)]
    pub struct Set {
        #[field(index = 1)]
        pub value: u64,
    }

    #[event(kind = 1)]
    pub struct Changed {
        #[field(index = 1, enum_u8)]
        pub status: Status,
    }
}

#[schema_module(version = "1.2")]
mod qualified {
    use super::*;

    #[derive(statevec_macros::EnumU8)]
    #[repr(u8)]
    pub enum Status {
        Closed = 2,
        Open = 1,
    }

    #[statevec_macros::record(kind = 1, record_len = 64, uk(id = 0, fields = [id]))]
    pub struct Item {
        #[field(index = 3, enum_u8)]
        pub status: self::Status,
        #[field(index = 2)]
        pub label: statevec_model::FixedBytes<16>,
        #[field(index = 1, immutable)]
        pub id: core::primitive::u64,
    }

    #[statevec_macros::command(kind = 1)]
    pub struct Set {
        #[field(index = 1)]
        pub value: core::primitive::u64,
    }

    #[statevec_macros::event(kind = 1)]
    pub struct Changed {
        #[field(index = 1, enum_u8)]
        pub status: self::Status,
    }
}

#[schema_module(version = "1.2")]
mod qualified_changed {
    #[statevec_macros::record(kind = 1, record_len = 128)]
    pub struct Item {
        #[field(index = 1)]
        pub id: u32,
    }

    #[statevec_macros::command(kind = 1)]
    pub struct Set {
        #[field(index = 1)]
        pub value: u32,
    }

    #[statevec_macros::event(kind = 1)]
    pub struct Changed {
        #[field(index = 1)]
        pub value: u32,
    }
}

#[test]
fn qualified_schema_items_are_registered_with_the_module_version() {
    let registry = qualified::registry();
    assert_eq!(
        (registry.record_defs().count(), registry.command_defs().count(), registry.event_defs().count()),
        (1, 1, 1),
        "qualified schema attributes must not disappear"
    );
    let version = u16::from_le_bytes([1, 2]);
    assert_eq!(qualified::Item::definition().version, version, "qualified record inherits module version");
    assert_eq!(qualified::Set::definition().version, version, "qualified command inherits module version");
    assert_eq!(qualified::Changed::definition().version, version, "qualified event inherits module version");
}

#[test]
fn changed_qualified_layouts_change_each_schema_fingerprint() {
    let a = qualified::schema_identity();
    let b = qualified_changed::schema_identity();
    assert_ne!(a.record_schema_fingerprint, b.record_schema_fingerprint, "record changes must affect identity");
    assert_ne!(a.command_schema_fingerprint, b.command_schema_fingerprint, "command changes must affect identity");
    assert_ne!(a.event_schema_fingerprint, b.event_schema_fingerprint, "event changes must affect identity");
}

#[schema_module(version = "1.0")]
mod changed_enum {
    #[derive(statevec_macros::EnumU8)]
    #[repr(u8)]
    pub enum Status {
        Open = 2,
        Closed = 1,
    }
}

#[test]
fn qualified_enum_derive_registers_actual_discriminants() {
    assert_eq!(qualified::registry().enum_defs().count(), 1, "qualified EnumU8 must not disappear from the schema");
    assert_ne!(qualified::Status::Open.to_u8(), changed_enum::Status::Open.to_u8());
    assert_ne!(
        qualified::schema_identity().types_schema_fingerprint,
        changed_enum::schema_identity().types_schema_fingerprint,
        "changed enum meaning must change identity"
    );
}

#[test]
fn equivalent_rust_type_paths_preserve_record_command_and_event_identity() {
    let a = plain::Set::builder().set_value(42).build().unwrap();
    let b = qualified::Set::builder().set_value(42).build().unwrap();
    assert_eq!(a, b, "the command wire encoding is unchanged");
    let a = plain::schema_identity();
    let b = qualified::schema_identity();
    assert_eq!(a.record_schema_fingerprint, b.record_schema_fingerprint, "record type paths are not schema facts");
    assert_eq!(a.command_schema_fingerprint, b.command_schema_fingerprint, "payload type paths are not schema facts");
    assert_eq!(a.event_schema_fingerprint, b.event_schema_fingerprint, "enum references use their declared name");
    assert_eq!(plain::registry().to_canonical_idl_bytes(), qualified::registry().to_canonical_idl_bytes());
}

#[test]
fn explicit_enum_mapping_is_independent_of_variant_declaration_order() {
    // Use definitions directly so collection and ordering have separate oracles.
    let a = SchemaRegistry::new(statevec_model::Version::new(1, 0), &[], &[], &[], &[*plain::Status::definition()]);
    let b = SchemaRegistry::new(statevec_model::Version::new(1, 0), &[], &[], &[], &[*qualified::Status::definition()]);
    assert_eq!(plain::Status::Open.to_u8(), qualified::Status::Open.to_u8());
    assert_eq!(plain::Status::Closed.to_u8(), qualified::Status::Closed.to_u8());
    assert_eq!(a.identity(), b.identity(), "explicit enum values, not declaration order, define identity");
    assert_eq!(a.to_canonical_idl_bytes(), b.to_canonical_idl_bytes());
}

#[test]
fn imported_macro_schema_identity_is_recomputed_from_definitions() {
    let original = plain::registry();
    let imported = SchemaRegistry::from_idl_bytes(&original.to_canonical_idl_bytes()).unwrap();
    let recomputed = SchemaRegistry::new(
        imported.schema_version(),
        &imported.record_defs().copied().collect::<Vec<_>>(),
        &imported.command_defs().copied().collect::<Vec<_>>(),
        &imported.event_defs().copied().collect::<Vec<_>>(),
        &imported.enum_defs().copied().collect::<Vec<_>>(),
    );
    assert_eq!(original.identity(), imported.identity());
    assert_eq!(
        original.identity(),
        recomputed.identity(),
        "imported definitions must independently reproduce identity"
    );
    assert_eq!(original.to_canonical_idl_bytes(), recomputed.to_canonical_idl_bytes());
}
