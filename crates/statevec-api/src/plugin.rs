// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

//! Core plugin contracts and typed error transport.
//!
//! Implement [`CoreModule`], [`ConfiguredCore`] and [`CoreRuntime`], then call
//! [`export_core_plugin!`](crate::export_core_plugin) in the final loadable library.
//! The discovery table declares its binary ABI in [`ffi::SvHeader::abi_major`].

#[doc(hidden)]
#[path = "plugin/export.rs"]
pub mod __export;
mod author;
mod borrow;
mod candidate;
mod core_failure;
mod core_status;
mod failure;
pub mod ffi;
mod migration;
mod status;
mod tx;

pub use author::{
    CandidateAfterChange, CandidateChange, CandidateRead, ConfiguredCore, CoreConfig, CoreMetadata, CoreModule,
    CoreRuntime, TxInvariantHook,
};
pub use migration::{MigrationTx, SourceRecords, TargetRecords};
pub use statevec_model::CoreVersion;
pub use tx::PluginTx;

pub use core_failure::{
    ConfigureError, CoreAbiError, CoreCreateError, CoreError, CoreExecutionError, CoreOperation, CoreTxError,
    HostError, ResourceError,
};
pub use failure::{CoreAccessError, CoreSchemaError, CoreStorageError};
pub use status::{CoreStatusDecodeError, SvStatus};

#[cfg(test)]
#[path = "ut_plugin_v3_status.rs"]
mod tests;

#[cfg(test)]
#[path = "ut_plugin_v3_core_status.rs"]
mod core_tests;

#[cfg(test)]
#[path = "ut_plugin_v3_layout.rs"]
mod layout_tests;
