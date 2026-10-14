//! Projector V1 transport. Raw tables do not grant source or publication authority.
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
mod author;
mod inputs;
mod observation;
#[doc(hidden)]
#[path = "plugin_v1/export.rs"]
pub mod __export;

pub use author::*;
pub use crate::{ProjectorInitialization, ProjectorRecoveryMode, ProjectorRecoverySupport};

pub use transport::{ControlReply, DeliveryLimits};
pub use io::{Delivery, FrameLoan, ProjectorControlPort, ProjectorIo, ProjectorIoParts, ProjectorServices, ProjectorSource, ProjectorStop, with_host};
pub use views::{FrameView, TransactionView, EventView, RecordChangeView};
pub use state::{StateLoan, StateRead, ProjectionStateInfo, ProjectionStateRecord};
pub use views::{EventView as ProjectedEventRef, RecordChangeView as ProjectedRecordChangeRef};
pub use control::{CheckpointCut, OffsetCheckpoint, RebuildCut, Offer, ReplyHandle, ReplyPoll};

pub use statevec_api::plugin_v3::{ConfigureFailure, HostFailure, ResourceFailure, SvStatus};
pub use failure::*;
pub use status::{decode_call, decode_failure};
pub use completion::{ProjectorCompletion, decode_observation};

#[cfg(test)]
#[path = "ut_plugin_v1.rs"]
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
