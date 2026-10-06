// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

use statevec_macros::{EnumU8, command, event, record, schema_module};
use statevec_model::EnumU8 as _;

mod types {
    use super::*;

    #[derive(EnumU8)]
    #[repr(u8)]
    pub enum Status {
        Open = 1,
        Closed = 2,
    }

    #[derive(EnumU8)]
    #[repr(u8)]
    pub enum Action {
        Close = 1,
    }

    #[derive(EnumU8)]
    #[repr(u8)]
    pub enum Change {
        Closed = 1,
    }
}

#[schema_module(version = "1.0")]
mod schema {
    use super::types::Status as StatusAlias;
    use super::{command, event, record, types::*};

    #[record(kind = 1, record_len = 64)]
    pub struct Item {
        #[field(index = 1, enum_u8)]
        pub status: Status,
        #[field(index = 2, enum_u8)]
        pub original_status: StatusAlias,
    }

    #[command(kind = 1)]
    pub struct Set {
        #[field(index = 1, enum_u8)]
        pub action: Action,
    }

    #[event(kind = 1)]
    pub struct Changed {
        #[field(index = 1, repeated, max = 3, enum_u8)]
        pub changes: Vec<Change>,
    }
}

#[schema_module(version = "1.0")]
mod conflicting {
    use super::{record, types::Status as ImportedStatus};

    #[derive(statevec_macros::EnumU8)]
    #[repr(u8)]
    pub enum Status {
        Open = 2,
        Closed = 1,
    }

    #[record(kind = 1, record_len = 64)]
    pub struct Item {
        #[field(index = 1, enum_u8)]
        pub status: ImportedStatus,
    }
}

#[test]
fn referenced_record_command_and_repeated_event_enums_are_registered_once() {
    let registry = schema::registry();
    let definitions: Vec<_> = registry.enum_defs().copied().collect();
    assert_eq!(definitions.len(), 3, "every referenced enum must be registered; aliases must not duplicate it");
    for expected in [types::Status::DEFINITION, types::Action::DEFINITION, types::Change::DEFINITION] {
        assert!(definitions.contains(expected), "missing referenced enum {}", expected.name);
    }
    let imported = statevec_model::SchemaRegistry::from_idl_bytes(&registry.to_canonical_idl_bytes()).unwrap();
    assert_eq!(registry.identity(), imported.identity(), "referenced enum meaning must survive IDL export");
}

#[test]
#[should_panic(expected = "duplicate enum definition registered: name=Status")]
fn conflicting_imported_and_local_enum_names_are_not_silently_deduplicated() {
    let _ = conflicting::registry();
}
