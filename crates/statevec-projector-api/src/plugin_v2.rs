//! Configured Projector contracts: implement configure -> prepare -> run.
//! Source evidence, durability declaration and owner acceptance are distinct.
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
#[path = "plugin_v2/export.rs"]
pub mod __export;

pub use plugin::*;
pub use crate::{ProjectorInitialization, ProjectorRecoveryMode, ProjectorRecoverySupport};

pub use transport::DeliveryLimits;
pub use io::{Delivery, FrameLoan, ProjectorControlPort, ProjectorIo, ProjectorIoParts, ProjectorServices, ProjectorSource, ProjectorStop};
pub use views::{FrameView, TransactionView, EventView, RecordChangeView};
pub use state::{StateLoan, StateRead, ProjectionStateInfo, ProjectionStateRecord};
pub use views::{EventView as ProjectedEventRef, RecordChangeView as ProjectedRecordChangeRef};
pub use control::{SourceCut, DurableOffsetReport, NotContinuousHistory, ControlDecision, RebuildCut, Offer, ReplyHandle, ReplyPoll};
pub use pending::{ControlProgress, PendingDurableOffset, PendingRebuildConfirmation};

pub use statevec_api::plugin_v3::{ConfigureFailure, HostFailure, ResourceFailure, SvStatus};
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
