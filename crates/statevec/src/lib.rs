// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

//! Facade crate for the stable StateVec public API.
//!
//! External users can depend on `statevec` and import the common surface with:
//!
//! ```rust
//! use statevec::prelude::*;
//! ```

pub use statevec_api as api;
pub use statevec_macros as macros;
pub use statevec_model as model;

#[doc(hidden)]
pub use statevec_api;
#[doc(hidden)]
pub use statevec_model;

pub use statevec_api::{
    BizInvariantReadContext, BusinessRejectCode, CanonicalIndexCount, CommittedStatus, InvariantReadContextExt,
    ReferenceTimeUnavailable, RejectedErrorCode, RuntimeApiProbe, RuntimeCommandEnvelope, RuntimeCommandRef,
    RuntimeHostContext, RuntimeHostContextExt, RuntimeHostError, RuntimePlugin, RuntimePluginError, RuntimePluginFactory,
    RuntimePluginLoadError, RuntimePluginUnloadError, STATEVEC_API_COMPAT_VERSION, STATEVEC_API_VERSION, TxContext,
    TxPositionUnavailable, TxReadContext, TxSysIdCreateContext, TxUkContext, TxWriteContext, TypedTxContext,
};
pub use statevec_macros::{EnumU8, command, command_dispatch, event, export_runtime_plugin, record, schema_module};
pub use statevec_model::{
    AccessError, Command, CommandDefinition, CommandKind, CommandSchema, EnumDecodeError, EnumDefinition,
    EnumU8 as EnumU8Schema, EnumVariantDefinition, Event, EventDefinition, EventFrame, EventKind, EventSchema,
    FieldDefinition, FieldType, FixedBytes, GeneratedCommandAccess, GeneratedEventAccess, GeneratedRecordAccess,
    KeyBuilder, KeyBytes, KeyEncodeFn, MAX_UNIQUE_KEYS_PER_RECORD, PayloadBuildError, PayloadFieldDefinition,
    RECORD_HEADER_SIZE, RecordDefinition, RecordKey, RecordKind, RecordSchema, SchemaFingerprint, SchemaIdentity,
    SchemaRegistry, StatevecCommandPayloadBuilder, SysId, TxSeq, UkCodec, UniqueKeyBytes, UniqueKeyDefinition, Version,
};
pub use statevec_model::{command as command_model, event as event_model, record as record_model};

/// Record-model compatibility namespace.
pub mod record {
    pub use statevec_model::record::*;
}

/// Command-model compatibility namespace.
pub mod command {
    pub use statevec_model::command::*;
}

/// Command-model compatibility namespace.
pub mod command_payload {
    pub use statevec_model::command::*;
}

/// Event-model compatibility namespace.
pub mod event {
    pub use statevec_model::event::*;
}

/// Schema registry compatibility namespace.
pub mod registry {
    pub use statevec_model::registry::*;
}

/// Common imports for StateVec domain crates.
///
/// This prelude explicitly re-exports the model types, runtime API traits, and
/// schema macros most domain modules need. It also brings `statevec_model` and
/// `statevec_api` into scope because the generated macro expansion currently
/// refers to those crate names.
pub mod prelude {
    pub use statevec_api;
    pub use statevec_model;

    pub use statevec_api::{
        BizInvariantReadContext, BusinessRejectCode, CanonicalIndexCount, InvariantReadContextExt,
        ReferenceTimeUnavailable, RuntimeCommandEnvelope, RuntimeCommandRef, RuntimeHostContext, RuntimeHostContextExt,
        RuntimeHostError, RuntimePlugin, RuntimePluginError, RuntimePluginFactory, RuntimePluginLoadError, RuntimePluginUnloadError,
        TxContext, TxPositionUnavailable, TxReadContext, TxSysIdCreateContext, TxUkContext, TxWriteContext, TypedTxContext,
    };
    pub use statevec_macros::{EnumU8, command, command_dispatch, event, export_runtime_plugin, record, schema_module};
    pub use statevec_model::{
        AccessError, Command, CommandDefinition, CommandSchema, EnumDecodeError, EnumDefinition,
        EnumU8 as EnumU8Schema, EnumVariantDefinition, Event, EventDefinition, EventFrame, EventSchema,
        FieldDefinition, FieldType, FixedBytes, GeneratedCommandAccess, GeneratedEventAccess, GeneratedRecordAccess,
        KeyBuilder, KeyBytes, KeyEncodeFn, MAX_UNIQUE_KEYS_PER_RECORD, PayloadBuildError, PayloadFieldDefinition,
        RECORD_HEADER_SIZE, RecordDefinition, RecordKey, RecordKind, RecordSchema, SchemaFingerprint, SchemaIdentity,
        SchemaRegistry, StatevecCommandPayloadBuilder, SysId, TxSeq, UkCodec, UniqueKeyBytes, UniqueKeyDefinition,
        Version,
    };
}

#[cfg(test)]
#[path = "ut_lib.rs"]
mod tests;
