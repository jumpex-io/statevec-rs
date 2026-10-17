//! Build a configured Projector: configure -> prepare -> run.
//!
//! Start with [`plugin_v2::ProjectorModule`], [`plugin_v2::ConfiguredProjector`]
//! and [`plugin_v2::PreparedProjector`]. The SDK runs the prepared plugin on its
//! worker with borrowed [`plugin_v2::ProjectorIo`] and joins scoped services.
//! Receiving a complete entry gives source evidence, not a durability receipt.
//! Persist effects and offset, declare that fact with [`plugin_v2::SourceCut::declare_durable`],
//! then wait for the separate owner decision through [`plugin_v2::PendingDurableOffset`].
//! Latest-state instead enumerates a frozen baseline and confirms its installation.
//! Source attachment, replication, replay and checkpoint selection stay native.
//! [`ConsumeOffset`] supplies a bounded four-field JSON codec. On Unix,
//! [`FileOffsetStore`] persists it explicitly; applications report durability
//! separately through [`plugin_v2::ProjectorControlPort::report_durable_offset`].
//! [`native`] contains the host's owned-frame/receiver/control traits; these are
//! not the borrowed plugin API. [`plugin_v2::host`] and [`plugin_v2::ffi`] are
//! integration/transport surfaces, not another plugin lifecycle.
//!
//! ```
//! use statevec_projector_api::native::{CommittedFrame, CommittedTransaction};
//! fn event_count<F: CommittedFrame>(frame: &F) -> usize {
//!     frame.transactions().iter().map(|tx| tx.output().events.len()).sum()
//! }
//! ```
//!
//! A trait implementation is not proof of commitment. The production source's
//! concrete frame and cut types have private constructors; its control endpoint
//! accepts only that source's concrete publication type. Public position and
//! restore metadata cannot manufacture those tokens. Local substitutes may
//! implement these traits for application tests without acquiring runtime authority.
//!
//! ```compile_fail,E0308
//! use statevec_projector_api::{ProjectionPosition, native::ProjectorControl};
//! fn acknowledge_from_metadata<C: ProjectorControl>(control: &C, position: ProjectionPosition) {
//!     control.enqueue_projector_checkpoint(position);
//! }
//! ```
//!
//! Whole-frame consumption includes empty progress barriers. Receiving a frame
//! does not acknowledge a durable checkpoint. Save business effects and the
//! complete offset before reporting the source cut as durable.

pub mod native;
mod types;
mod offset;
#[cfg(unix)]
mod offset_file;
pub mod plugin_v2;

#[cfg(unix)]
pub use offset_file::{FileOffsetStore, OffsetFileFailure, OffsetStoreError, OffsetStoreOperation};

pub use offset::{
    ConsumeOffset, ConsumeOffsetError, MAX_OFFSET_JSON_BYTES, MAX_PROJECTOR_ID_BYTES,
    OffsetJsonCategory, ProjectorId,
};

pub use types::*;
pub use statevec_frame::Digest32;
