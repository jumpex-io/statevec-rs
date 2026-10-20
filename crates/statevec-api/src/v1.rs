// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

//! Core plugin contracts and typed error transport.
//!
//! Authors implement CoreModule, ConfiguredCore and CoreRuntime, and use
//! `statevec_api::export_core_plugin_v4!` to build a loadable library. `v1`
//! names the safe Rust API; the macro exports Core C ABI 4. Neither the
//! Rust module name nor the Cargo version changes the C ABI. A status
//! does not prove speculation was discarded or authorize instance reuse. This
//! API does not bridge the legacy plugin ABI or supply a durable host.

mod failure;
mod status;
mod core_failure;
mod core_status;
mod author;
mod borrow;
mod candidate;
mod tx;
mod migration;
#[doc(hidden)]
#[path = "v1/export.rs"]
pub mod __export;
pub mod ffi;

pub use author::{
    CoreConfig, CoreMetadata, CoreModule, ConfiguredCore, CoreRuntime,
    TxInvariantHook, CandidateRead, CandidateChange, CandidateAfterChange,
};
pub use tx::PluginTx;
pub use migration::{MigrationTx, SourceRecords, TargetRecords};
pub use statevec_model::CoreVersion;

pub use failure::{CoreAccessError, CoreSchemaError, CoreStorageError};
pub use status::{CoreStatusDecodeError, SvStatus};
pub use core_failure::{CoreAbiError, ConfigureError, CoreError, CoreOperation, CoreCreateError, CoreExecutionError, HostError, ResourceError, CoreTxError};

#[cfg(test)]
#[path = "ut_plugin_v3_status.rs"]
mod tests;

#[cfg(test)]
#[path = "ut_plugin_v3_core_status.rs"]
mod core_tests;

#[cfg(test)]
#[path = "ut_plugin_v3_layout.rs"]
mod layout_tests;
