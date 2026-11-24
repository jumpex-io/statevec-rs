// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

//! Facade crate for the stable StateVec public API.
//!
//! External users can depend on `statevec` and import the common surface with:
//!
//! ```rust
//! use statevec::prelude::*;
//! ```

extern crate self as statevec;

pub use statevec_api as api;
pub use statevec_macros as macros;
pub use statevec_model as model;

#[doc(hidden)]
pub mod __private {
    pub use statevec_api as api;
    pub use statevec_model as model;
}

pub use statevec_api::{
    BusinessInvariantReadContext, BusinessRejectCode, CanonicalIndexCount, InvariantReadContextExt,
    ReferenceTimeUnavailable, RuntimeCommandEnvelope, RuntimeCommandRef,
    RuntimeHostContext, RuntimeHostContextExt, RuntimeHostError, RuntimePlugin, RuntimePluginError, RuntimePluginUnloadError, TxPositionUnavailable, TxReadContext, TxSysIdCreateContext, TxUniqueKeyContext, TxWriteContext, TypedTxContext,
};
pub use statevec_macros::{EnumU8, command, command_dispatch, event, record, schema_module};
pub use statevec_model::{
    AccessError, Command, CommandDefinition, CommandKind, CommandSchema, EnumDecodeError, EnumDefinition,
    EnumU8 as EnumU8Schema, EnumVariantDefinition, Event, EventDefinition, EventFrame, EventKind, EventSchema,
    FieldDefinition, FieldType, FixedBytes, GeneratedCommandAccess, GeneratedEventAccess, GeneratedRecordAccess,
    KeyBuilder, KeyBytes, KeyEncodeFn, MAX_UNIQUE_KEYS_PER_RECORD, PayloadBuildError, PayloadFieldDefinition,
    RECORD_HEADER_SIZE, RecordDefinition, RecordKey, RecordKind, RecordSchema, SchemaFingerprint, SchemaIdentity,
    SchemaRegistry, StatevecCommandPayloadBuilder, SysId, TxSeq, UniqueKeyCodec, UniqueKeyBytes, UniqueKeyDefinition, Version,
};
/// Types used by record definitions and accessors.
pub mod record {
    pub use statevec_model::record::{Decimal, RecordKind, RecordSchema, SysId};
}

/// Typed access to generated event payloads.
pub mod event {
    pub use statevec_model::event::GeneratedEventAccess;
}

/// Common imports for StateVec domain crates.
pub mod prelude {
    pub use statevec_api::{
        BusinessInvariantReadContext, BusinessRejectCode, CanonicalIndexCount, InvariantReadContextExt,
        ReferenceTimeUnavailable, RuntimeCommandEnvelope, RuntimeCommandRef, RuntimeHostContext, RuntimeHostContextExt,
        RuntimeHostError, RuntimePlugin, RuntimePluginError, RuntimePluginUnloadError,
        TxPositionUnavailable, TxReadContext, TxSysIdCreateContext, TxUniqueKeyContext, TxWriteContext, TypedTxContext,
    };
    pub use statevec_macros::{EnumU8, command, command_dispatch, event, record, schema_module};
    pub use statevec_model::{
        AccessError, Command, CommandDefinition, CommandSchema, EnumDecodeError, EnumDefinition,
        EnumU8 as EnumU8Schema, EnumVariantDefinition, Event, EventDefinition, EventFrame, EventSchema,
        FieldDefinition, FieldType, FixedBytes, GeneratedCommandAccess, GeneratedEventAccess, GeneratedRecordAccess,
        KeyBuilder, KeyBytes, KeyEncodeFn, MAX_UNIQUE_KEYS_PER_RECORD, PayloadBuildError, PayloadFieldDefinition,
        RECORD_HEADER_SIZE, RecordDefinition, RecordKey, RecordKind, RecordSchema, SchemaFingerprint, SchemaIdentity,
        SchemaRegistry, StatevecCommandPayloadBuilder, SysId, TxSeq, UniqueKeyCodec, UniqueKeyBytes, UniqueKeyDefinition,
        Version,
    };
}

#[cfg(test)]
#[path = "ut_lib.rs"]
mod tests;
