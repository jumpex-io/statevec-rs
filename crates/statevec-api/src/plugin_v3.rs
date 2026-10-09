// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

//! Typed V3 core failure transport under construction.
//!
//! This module supplies typed Core causes and operation-specific status decoding.
//! It does not yet export a loadable plugin or an execution adapter. In particular,
//! a status is not proof that speculation was discarded or that an instance is
//! reusable. The legacy plugin ABI remains unchanged.

mod failure;
mod status;
mod core_failure;
mod core_status;

pub use failure::{AccessFailure, SchemaFailure, StorageFailure};
pub use status::{StatusDecodeFailure, SvStatus};
pub use core_failure::{AbiFailure, ConfigureFailure, CoreFailure, CoreOperation, CreateFailure, ExecutionFailure, HostFailure, ResourceFailure, TxFailure};

#[cfg(test)]
#[path = "ut_plugin_v3_status.rs"]
mod tests;

#[cfg(test)]
#[path = "ut_plugin_v3_core_status.rs"]
mod core_tests;
