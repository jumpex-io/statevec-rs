// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

//! Core schema and wire-model types for StateVec.
//!
//! This crate contains the stable schema model shared by domain plugins and
//! runtime hosts: record layouts, command and event payload definitions,
//! generated accessor traits, schema registries, schema fingerprints, and IDL
//! JSON conversion.

/// Schema IDL JSON generation and parsing.
pub mod idl;
/// Core record, command, event, field, and accessor model types.
pub mod model;
/// Schema registry and stable schema fingerprint calculation.
pub mod registry;
#[cfg(test)]
mod ut_idl;
#[cfg(test)]
mod ut_model;
#[cfg(test)]
mod ut_registry;

/// Backward-compatible re-export: `statevec_model::record::*` still works.
pub mod record {
    pub use crate::model::{
        AccessError, EnumDecodeError, EnumDefinition, EnumU8, EnumVariantDefinition,
        FieldDefinition, FieldType, FixedBytes, GeneratedRecordAccess, PkBuilder, PkBytes, PkCodec,
        PkEncodeFn, RECORD_HEADER_SIZE, RecordDefinition, RecordKey, RecordKind, RecordSchema,
        SysId, TxSeq, Version, read_bool, read_fixed_bytes, read_i32_le, read_i64_le, read_u8,
        read_u16_le, read_u32_le, read_u64_le, read_u128_le, write_bool, write_fixed_bytes,
        write_i32_le, write_i64_le, write_u8, write_u16_le, write_u32_le, write_u64_le,
        write_u128_le,
    };
}

/// Backward-compatible re-export: `statevec_model::command::*` still works.
pub mod command {
    pub use crate::model::{
        Command, CommandDefinition, CommandSchema, GeneratedCommandAccess, PayloadFieldDefinition,
        Version, read_var_bytes, write_var_bytes,
    };
}

/// Backward-compatible re-export: `statevec_model::event::*` still works.
pub mod event {
    pub use crate::model::{
        Event, EventDefinition, EventFrame, EventSchema, GeneratedEventAccess,
        PayloadFieldDefinition, TxSeq, Version,
    };
}

pub use model::*;

/// Stable schema identity and registry types.
pub use registry::{SchemaFingerprint, SchemaIdentity, SchemaRegistry};
