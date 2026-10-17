//! Configured Projector contracts: implement configure -> prepare -> run.
//! Source tokens, durability declarations and owner acceptance are distinct.
//! `v1` names the safe Rust API. `export_projector_plugin_v2!` still exports
//! Projector C ABI 2; `export_core_plugin_v3!` still exports Core C ABI 3.
//! These C ABI versions are independent of the Rust module name.
//! Core V3 shares scalar representations only; its operation decoder is unchanged.
pub mod ffi;
mod failure;
mod status;
mod completion;
mod values;
mod transport;
mod borrow;
mod io;
mod views;
mod state;
mod control;
mod pending;
mod plugin;
mod inputs;
mod observation;
mod wire;
#[doc(hidden)]
#[path = "v1/export.rs"]
pub mod __export;

pub use plugin::*;
pub use crate::{StartInstruction, HistoryMode, SupportedHistoryModes};

pub use transport::DeliveryLimits;
pub use io::{Delivery, RaftEntryDelivery, ProjectorControlPort, ProjectorIo, ProjectorIoParts, ProjectorServices, ProjectorSource, ProjectorStop};
pub use views::{RaftEntryView, TxView, EventView, RecordDeltaView};
pub use state::{SnapshotReader, SnapshotRead, SnapshotInfo, SnapshotRecordView};
pub use control::{RaftEntryToken, DurableOffsetReport, NotContinuousHistory, ControlDecision, SnapshotToken, Offer, ReplyHandle, ReplyPoll};
pub use pending::{ControlProgress, PendingDurableOffset, PendingSnapshotConfirmation};

pub use statevec_api::v1::{ConfigureError, HostError, ResourceError, SvStatus};
pub use failure::*;
pub(crate) use status::{decode_call, decode_failure};
#[cfg(test)]
use completion::decode_observation;
pub(crate) use transport::ControlReply;

/// Host/export integration. These decoders and unsafe callback adapters are
/// not needed by a plugin implementing the configured contracts above.
pub mod host {
    pub use super::io::with_host;
    pub use super::status::{decode_call, decode_failure};
    pub use super::completion::{ProjectorCompletion, decode_observation};
    pub use super::transport::ControlReply;
}

#[cfg(test)]
#[path = "ut_plugin_v2.rs"]
mod tests;
#[cfg(test)]
#[path = "ut_projector_io.rs"]
mod io_tests;
#[cfg(test)]
#[path = "ut_projector_export.rs"]
mod export_tests;
#[cfg(test)]
#[path = "ut_projector_poll.rs"]
mod poll_tests;
