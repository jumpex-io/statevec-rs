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
    BizInvariantReadContext, CommittedReceipt, CommittedResultQuery, CommittedResultRecord,
    CommittedStatus, InvariantReadContextExt, QueueCodecError, QueueConsumer,
    QueueConsumerResume, QueueProducer, QueueReceipt, QueueRecord, RejectedErrorCode,
    RuntimeCommandEnvelope, RuntimeCommandRef, RuntimeHostContext, RuntimeHostContextExt,
    RuntimeHostError, RuntimePlugin, RuntimePluginError, RuntimePluginFactory,
    RuntimePluginLoadError, RuntimePluginUnloadError, STATEVEC_API_COMPAT_VERSION,
    STATEVEC_API_VERSION, TxContext, TxPkContext, TxReadContext, TxSysIdCreateContext,
    TxWriteContext, TypedTxContext, decode_committed_result_record, decode_submit_request,
    encode_committed_result_record, encode_submit_request,
};
pub use statevec_macros::{
    EnumU8, command, command_dispatch, event, export_runtime_plugin, record, schema_module,
};
pub use statevec_model::{
    AccessError, Command, CommandDefinition, CommandSchema, EnumDecodeError, EnumDefinition,
    EnumU8 as EnumU8Schema, EnumVariantDefinition, Event, EventDefinition, EventFrame,
    EventSchema, FieldDefinition, FieldType, FixedBytes, GeneratedCommandAccess,
    GeneratedEventAccess, GeneratedRecordAccess, PayloadFieldDefinition, PkBuilder, PkBytes,
    PkCodec, PkEncodeFn, RECORD_HEADER_SIZE, RecordDefinition, RecordKey, RecordKind,
    RecordSchema, SchemaFingerprint, SchemaIdentity, SchemaRegistry, SysId, TxSeq, Version,
};
pub use statevec_model::{command as command_model, event as event_model, record as record_model};

/// Record-model compatibility namespace.
pub mod record {
    pub use statevec_model::record::*;
}

/// Command-model compatibility namespace.
pub mod command_payload {
    pub use statevec_model::command::*;
}

/// Event-model compatibility namespace.
pub mod event {
    pub use statevec_model::event::*;
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
        BizInvariantReadContext, InvariantReadContextExt, RuntimeCommandEnvelope,
        RuntimeCommandRef, RuntimeHostContext, RuntimeHostContextExt, RuntimeHostError,
        RuntimePlugin, RuntimePluginError, RuntimePluginFactory, RuntimePluginLoadError,
        RuntimePluginUnloadError, TxContext, TxPkContext, TxReadContext, TxSysIdCreateContext,
        TxWriteContext, TypedTxContext,
    };
    pub use statevec_macros::{
        EnumU8, command, command_dispatch, event, export_runtime_plugin, record, schema_module,
    };
    pub use statevec_model::{
        AccessError, Command, CommandDefinition, CommandSchema, EnumDecodeError, EnumDefinition,
        EnumU8 as EnumU8Schema, EnumVariantDefinition, Event, EventDefinition, EventFrame,
        EventSchema, FieldDefinition, FieldType, FixedBytes, GeneratedCommandAccess,
        GeneratedEventAccess, GeneratedRecordAccess, PayloadFieldDefinition, PkBuilder, PkBytes,
        PkCodec, PkEncodeFn, RECORD_HEADER_SIZE, RecordDefinition, RecordKey, RecordKind,
        RecordSchema, SchemaFingerprint, SchemaIdentity, SchemaRegistry, SysId, TxSeq, Version,
    };
}

#[cfg(test)]
mod tests {
    use super::prelude::*;

    #[derive(EnumU8)]
    #[repr(u8)]
    enum Side {
        Buy = 1,
        Sell = 2,
    }

    #[schema_module(version = "1.0")]
    mod schema {
        use super::*;

        #[record(kind = 1, record_len = 64, pk(fields = [id]))]
        pub struct Order {
            #[field(index = 1, immutable)]
            pub id: u64,
            #[field(index = 2, enum_u8)]
            pub side: Side,
        }

        #[command(kind = 1)]
        pub struct PlaceOrder {
            #[field(index = 1)]
            pub id: u64,
        }

        #[event(kind = 1)]
        pub struct OrderPlaced {
            #[field(index = 1)]
            pub id: u64,
        }
    }

    #[test]
    fn prelude_exposes_public_model_api_and_macros() {
        assert_eq!(schema::SCHEMA_VERSION, Version::new(1, 0));
        let identity: SchemaIdentity = schema::schema_identity();
        assert_eq!(identity.schema_version, schema::SCHEMA_VERSION);
    }
}
