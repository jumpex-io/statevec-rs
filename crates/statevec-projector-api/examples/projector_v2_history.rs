//! Durable public-SDK reference consumer; the qualification interposer reuses
//! this same plugin rather than implementing another business/store protocol.
#![cfg(unix)]
#[path = "projector_v2_history/plugin.rs"]
mod plugin;
use plugin::History;
statevec_projector_api::export_projector_plugin_v2!(History);

#[cfg(test)]
#[path = "projector_v2_history/tests.rs"]
mod tests;
