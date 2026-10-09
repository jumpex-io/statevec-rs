// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

//! Typed V3 core failure transport under construction.
//!
//! This module currently supplies the business, static-schema and access error
//! portions of the ABI. It does not export a loadable plugin or an execution
//! adapter. Bootstrap, host-resource and instance-lifecycle causes are separate
//! pending parts of V3; callers must not interpret an unimplemented tag as a
//! business rejection or success. The legacy plugin ABI remains unchanged.

mod failure;
mod status;

pub use failure::{AccessFailure, SchemaFailure, StorageFailure};
pub use status::{StatusDecodeFailure, SvStatus};

#[cfg(test)]
#[path = "ut_plugin_v3_status.rs"]
mod tests;
