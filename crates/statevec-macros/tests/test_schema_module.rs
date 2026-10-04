// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

//! Auxiliary tests for schema-module macro expansion.

use statevec_macros::{EnumU8, command, event, record, schema_module};
use statevec_model::{
    CommandSchema, EventSchema, FixedBytes, GeneratedCommandAccess, GeneratedEventAccess, GeneratedRecordAccess,
    RecordSchema, SchemaIdentity, Version,
};

#[schema_module(version = "1.0")]
pub mod v1_0 {
    use super::*;

    #[derive(EnumU8)]
    #[repr(u8)]
    pub enum TestStatus {
        Pending = 1,
        Active = 2,
    }

    #[record(kind = 1, record_len = 64, uk(id = 0, fields = [asset_id]))]
    pub struct Asset {
        #[field(index = 1, immutable)]
        pub asset_id: u64,
        #[field(index = 2)]
        pub precision: u8,
    }

    #[command(kind = 1)]
    pub struct AddAsset {
        #[field(index = 1)]
        pub asset_id: u64,
        #[field(index = 2, enum_u8)]
        pub status: TestStatus,
    }

    #[event(kind = 1, inline_response = true)]
    pub struct AssetCreated {
        #[field(index = 1)]
        pub asset_id: u64,
    }
}

#[schema_module(version = "1.1")]
pub mod v1_1 {
    use super::*;

    #[derive(EnumU8)]
    #[repr(u8)]
    pub enum TestStatus {
        Pending = 1,
        Active = 2,
    }

    #[record(kind = 1, record_len = 64, uk(id = 0, fields = [asset_id]))]
    pub struct Asset {
        #[field(index = 1, immutable)]
        pub asset_id: u64,
        #[field(index = 2)]
        pub precision: u8,
    }

    #[command(kind = 1)]
    pub struct AddAsset {
        #[field(index = 1)]
        pub asset_id: u64,
        #[field(index = 2, enum_u8)]
        pub status: TestStatus,
    }

    #[event(kind = 1)]
    pub struct AssetCreated {
        #[field(index = 1)]
        pub asset_id: u64,
    }
}

#[schema_module(version = "1.2")]
pub mod v1_fixed {
    use super::*;

    #[record(kind = 1, record_len = 64, uk(id = 0, fields = [asset_id]))]
    pub struct AssetWithSymbol {
        #[field(index = 1, immutable)]
        pub asset_id: u64,
        #[field(index = 2)]
        pub symbol: FixedBytes<16>,
    }
}

#[test]
fn schema_module_builds_registry_and_idl() {
    assert_eq!(v1_0::SCHEMA_VERSION, Version::new(1, 0));

    let registry = v1_0::registry();
    assert_eq!(registry.schema_version(), Version::new(1, 0));
    assert_eq!(registry.try_get(v1_0::Asset::KIND).unwrap().name, "Asset");
    assert_eq!(registry.try_get_command(v1_0::AddAsset::KIND).unwrap().name, "AddAsset");
    assert_eq!(registry.try_get_event(v1_0::AssetCreated::KIND).unwrap().name, "AssetCreated");
    assert!(registry.try_get_event(v1_0::AssetCreated::KIND).unwrap().inline_response);
    assert_eq!(registry.try_get(v1_0::Asset::KIND).unwrap().version, 1);
    assert_eq!(registry.try_get_command(v1_0::AddAsset::KIND).unwrap().version, 1);
    assert_eq!(registry.try_get_event(v1_0::AssetCreated::KIND).unwrap().version, 1);

    let identity: SchemaIdentity = v1_0::schema_identity();
    assert_eq!(identity.schema_version, Version::new(1, 0));
    assert_eq!(identity.record_schema_fingerprint, registry.record_schema_fingerprint());

    let json = v1_0::idl_json();
    assert!(json.contains("\"schemaVersion\""));
    assert!(json.contains("\"main\": 1"));
    assert!(json.contains("\"minor\": 0"));
    assert!(json.contains("\"name\": \"Asset\""));
    assert!(json.contains("\"name\": \"AddAsset\""));
    assert!(json.contains("\"name\": \"AssetCreated\""));
    assert!(json.contains("\"inlineResponse\": true"));

    let mut buf = [0u8; 64];
    let mut builder: v1_0::record::NewAssetBuilder<'_> = <v1_0::Asset as GeneratedRecordAccess>::wrap_new(&mut buf);
    builder.init_asset_id(7).set_precision(8);
    let acc = v1_0::record::AssetAccess::new(&buf);
    assert_eq!(acc.asset_id(), 7);

    let command = <v1_0::AddAsset as GeneratedCommandAccess>::builder()
        .set_asset_id(7)
        .set_status(v1_0::TestStatus::Active)
        .build()
        .expect("command payload build");
    let access = v1_0::command::AddAssetAccess::new(&command);
    assert_eq!(access.asset_id(), 7);

    let payload = <v1_0::AssetCreated as GeneratedEventAccess>::builder()
        .set_asset_id(9)
        .build()
        .expect("event payload build");
    let event_access = v1_0::event::AssetCreatedAccess::new(&payload);
    assert_eq!(event_access.asset_id(), 9);
}

#[test]
fn schema_modules_can_coexist_with_different_schema_versions() {
    let v1_0_identity = v1_0::schema_identity();
    let v1_1_identity = v1_1::schema_identity();

    assert_ne!(v1_0_identity, v1_1_identity);
    assert_eq!(v1_0_identity.types_schema_fingerprint, v1_1_identity.types_schema_fingerprint);
    assert_ne!(v1_0_identity.record_schema_fingerprint, v1_1_identity.record_schema_fingerprint);
    assert_ne!(v1_0_identity.command_schema_fingerprint, v1_1_identity.command_schema_fingerprint);
    assert_ne!(v1_0_identity.event_schema_fingerprint, v1_1_identity.event_schema_fingerprint);
    assert_eq!(v1_1_identity.schema_version, Version::new(1, 1));
    assert_eq!(v1_1::registry().try_get(v1_1::Asset::KIND).unwrap().version, 257);
    assert_eq!(v1_1::registry().try_get_command(v1_1::AddAsset::KIND).unwrap().version, 257);
    assert_eq!(v1_1::registry().try_get_event(v1_1::AssetCreated::KIND).unwrap().version, 257);
}

#[test]
fn fixed_bytes_field_definition_uses_real_rust_type_name() {
    let registry = v1_fixed::registry();
    let def = registry.try_get(v1_fixed::AssetWithSymbol::KIND).unwrap();
    let symbol = def.field_by_name("symbol").expect("symbol field missing");

    assert_eq!(symbol.rust_type_name, "FixedBytes < 16 >");
    assert!(!symbol.rust_type_name.contains("stringify!"));

    let json = v1_fixed::idl_json();
    assert!(json.contains("\"fixedBytes\": 16"));
    assert!(!json.contains("stringify!("));
}
