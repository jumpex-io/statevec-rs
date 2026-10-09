// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

//! Fixed-version Core V3 authoring, exports and typed failure transport.
//!
//! Authors implement CoreModule, ConfiguredCore and CoreRuntime, and use
//! `statevec_api::export_core_plugin_v3!` to build a loadable library. The host's
//! native execution/recovery integration is still under construction. A status
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
#[doc(hidden)]
#[path = "plugin_v3/export.rs"]
pub mod __export;
pub mod ffi;

pub use author::{
    CoreConfig, CoreMetadata, CoreModule, ConfiguredCore, CoreRuntime,
    TxInvariantHook, CandidateRead, CandidateChange, CandidateAfterChange,
};
pub use tx::PluginTx;

pub use failure::{AccessFailure, SchemaFailure, StorageFailure};
pub use status::{StatusDecodeFailure, SvStatus};
pub use core_failure::{AbiFailure, ConfigureFailure, CoreFailure, CoreOperation, CreateFailure, ExecutionFailure, HostFailure, ResourceFailure, TxFailure};

#[cfg(test)]
#[path = "ut_plugin_v3_status.rs"]
mod tests;

#[cfg(test)]
#[path = "ut_plugin_v3_core_status.rs"]
mod core_tests;

#[cfg(test)]
#[path = "ut_plugin_v3_layout.rs"]
mod layout_tests;
