//! Durable public-SDK reference consumer; the qualification interposer reuses
//! this same author rather than implementing another business/store protocol.
#![cfg(unix)]
#[path = "projector_v1_history/author.rs"]
mod author;
use author::History;
statevec_projector_api::export_projector_plugin_v1!(History);

#[cfg(test)]
#[path = "projector_v1_history/tests.rs"]
mod tests;
