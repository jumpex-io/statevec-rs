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
/// Canonical validation of reserved record spans.
pub mod reserved_bytes;
/// Immutable core-runtime release identity.
pub mod runtime_release;
/// Engine-owned system schema metadata framing.
pub mod system_schema_metadata;
#[cfg(test)]
mod ut_idl;
#[cfg(test)]
mod ut_model;
#[cfg(test)]
mod ut_registry;

/// Backward-compatible re-export: `statevec_model::record::*` still works.
pub mod record {
    pub use crate::model::{
        AccessError, CanonicalIndexDefinition, Decimal, DecimalParseError, DecimalRounding,
        DecimalScaleCompatibilityError, EnumDecodeError, EnumDefinition, EnumU8, EnumVariantDefinition,
        FieldDefinition, FieldType, FixedBytes, GeneratedRecordAccess, INVALID_KIND, KeyBuilder, KeyBytes, KeyEncodeFn,
        MAX_CANONICAL_INDEX_FIELDS, MAX_CANONICAL_INDEX_KEY_BYTES, MAX_CANONICAL_INDEXES_PER_RECORD, MAX_DECIMAL_SCALE,
        MAX_UNIQUE_KEYS_PER_RECORD, RECORD_HEADER_SIZE, RecordDefinition, RecordKey, RecordKind, RecordSchema,
        SYSTEM_KIND_MAX, SYSTEM_KIND_MIN, SYSTEM_RUNTIME_MANIFEST_RECORD_KIND, SYSTEM_SCHEMA_RECORD_KIND,
        SemanticCompatibilityError, SemanticTag, SemanticTagParseError, SysId, TxSeq, USER_KIND_MAX, USER_KIND_MIN,
        UkCodec, UniqueKeyBytes, UniqueKeyDefinition, Version, read_bool, read_decimal_le, read_fixed_bytes,
        read_i32_le, read_i64_le, read_u8, read_u16_le, read_u32_le, read_u64_le, read_u128_le,
        validate_decimal_scale_compat, validate_semantic_compat, write_bool, write_decimal_le, write_fixed_bytes,
        write_i32_le, write_i64_le, write_u8, write_u16_le, write_u32_le, write_u64_le, write_u128_le,
    };
}

/// Backward-compatible re-export: `statevec_model::command::*` still works.
pub mod command {
    pub use crate::model::{
        Command, CommandDefinition, CommandKind, CommandSchema, GeneratedCommandAccess, INVALID_KIND,
        PayloadBuildError, PayloadFieldDefinition, RepeatedCompatibilityError, RepeatedElement, RepeatedScalar,
        SYSTEM_KIND_MAX, SYSTEM_KIND_MIN, SYSTEM_RUNTIME_MANIFEST_CUTOVER_COMMAND_KIND,
        SYSTEM_SCHEMA_CUTOVER_COMMAND_KIND, SemanticCompatibilityError, SemanticTag, SemanticTagParseError,
        StatevecCommandPayloadBuilder, USER_KIND_MAX, USER_KIND_MIN, Version, read_repeated_scalar, read_var_bytes,
        validate_repeated_payload_compat, validate_repeated_scalar, validate_semantic_compat, write_repeated_scalar,
        write_var_bytes,
    };
}

/// Backward-compatible re-export: `statevec_model::event::*` still works.
pub mod event {
    pub use crate::model::{
        Event, EventDefinition, EventFrame, EventKind, EventSchema, GeneratedEventAccess, INVALID_KIND,
        PayloadBuildError, PayloadFieldDefinition, RepeatedCompatibilityError, RepeatedElement, RepeatedScalar,
        SYSTEM_KIND_MAX, SYSTEM_KIND_MIN, SYSTEM_RUNTIME_MANIFEST_CUTOVER_EVENT_KIND, SYSTEM_SCHEMA_CUTOVER_EVENT_KIND,
        SemanticCompatibilityError, SemanticTag, SemanticTagParseError, TxSeq, USER_KIND_MAX, USER_KIND_MIN, Version,
        read_repeated_scalar, validate_repeated_payload_compat, validate_repeated_scalar, validate_semantic_compat,
        write_repeated_scalar,
    };
}

pub use model::*;

/// Stable schema identity and registry types.
pub use registry::{SchemaFingerprint, SchemaIdentity, SchemaRegistry};
pub use reserved_bytes::{EngineSchemaInvariantProfile, ReservedBytesZeroError, validate_reserved_bytes_zero};
pub use runtime_release::CoreRuntimeReleaseId;
pub use system_schema_metadata::{
    SYSTEM_SCHEMA_ENCODING_CANONICAL_JSON, SYSTEM_SCHEMA_ENCODING_CANONICAL_JSON_NAME, SYSTEM_SCHEMA_METADATA_MAGIC,
    SYSTEM_SCHEMA_METADATA_VERSION, SystemSchemaMetadata, SystemSchemaMetadataError, encode_system_schema_metadata,
    encode_system_schema_metadata_with_runtime_manifest, parse_system_schema_metadata,
};
