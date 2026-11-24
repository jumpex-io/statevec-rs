//! Implement [`ProjectorModule`], [`ConfiguredProjector`] and [`PreparedProjector`],
//! then call [`export_projector_plugin!`](crate::export_projector_plugin) in the
//! final loadable library. The discovery table declares its binary ABI in
//! [`ffi::SvHeader::abi_major`].
#[doc(hidden)]
#[path = "plugin/export.rs"]
pub mod __export;
mod author;
mod borrow;
mod completion;
mod control;
mod failure;
pub mod ffi;
mod inputs;
mod io;
mod observation;
mod pending;
mod query;
mod state;
mod status;
mod transport;
mod values;
mod views;
mod wire;

pub use crate::{HistoryMode, StartInstruction, SupportedHistoryModes};
pub use author::*;

pub use control::{
    ControlDecision, DurableOffsetReport, NotContinuousHistory, Offer, RaftEntryToken, ReplyHandle, ReplyPoll,
    SnapshotToken,
};
pub use io::{
    Delivery, ProjectorControlPort, ProjectorIo, ProjectorIoParts, ProjectorServices, ProjectorSource, ProjectorStop,
    RaftEntryDelivery,
};
pub use pending::{ControlProgress, PendingDurableOffset, PendingSnapshotConfirmation};
pub use query::{
    EngineQueryHandle, EngineQueryKey, EngineQueryPoll, EngineQueryPosition, EngineQueryRejection, EngineQueryResult,
    EngineRecordView, QueryOffer,
};
pub use state::{SnapshotInfo, SnapshotRead, SnapshotReader, SnapshotRecordView};
pub use transport::DeliveryLimits;
pub use views::{EventView, RaftEntryView, RecordDeltaView, TxView};

#[cfg(test)]
use completion::decode_observation;
pub use failure::*;
pub use statevec_api::plugin::{ConfigureError, HostError, ResourceError, SvStatus};
pub(crate) use status::{decode_call, decode_failure};
pub(crate) use transport::ControlReply;

/// Host/export integration. These decoders and unsafe callback adapters are
/// not needed by a plugin implementing the configured contracts above.
#[doc(hidden)]
pub mod host {
    pub use super::completion::{ProjectorCompletion, decode_observation};
    pub use super::io::{with_host, with_query_host};
    pub use super::status::{decode_call, decode_failure};
    pub use super::transport::ControlReply;
}

#[cfg(test)]
#[path = "ut_projector_export.rs"]
mod export_tests;
#[cfg(test)]
#[path = "ut_projector_io.rs"]
mod io_tests;
#[cfg(test)]
#[path = "ut_projector_poll.rs"]
mod poll_tests;
#[cfg(test)]
#[path = "ut_plugin_v2.rs"]
mod tests;
