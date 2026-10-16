//! Current-state reference consumer. The report is a subscriber-output stream,
//! never a recovery store: every start emits a new reset followed by deltas.
#[path = "projector_v2_latest/author.rs"]
mod author;
use author::Latest;
statevec_projector_api::export_projector_plugin_v2!(Latest);
