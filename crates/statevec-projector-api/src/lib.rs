//! Consume complete business output and coordinate a projector's durable checkpoint.
//!
//! Applications implement their read models and stores against these interfaces.
//! The production runtime supplies the source, frames and checkpoint cut tokens.
//! Source attachment, replication, replay, backpressure and checkpoint selection
//! remain runtime responsibilities. This crate contains no replication runtime.
//! [`ConsumeOffset`] supplies a bounded four-field JSON codec. On Unix,
//! [`FileOffsetStore`] persists it explicitly; applications report durability
//! separately through [`plugin_v1::ProjectorControlPort::report_durable_offset`].
//! [`plugin_v1`] supplies the fixed wire
//! contract; native attachment and a loaded worker still require host support.
//!
//! ```
//! use statevec_projector_api::{CommittedFrame, CommittedTransaction};
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
//! use statevec_projector_api::{ProjectionPosition, ProjectorControl};
//! fn acknowledge_from_metadata<C: ProjectorControl>(control: &C, position: ProjectionPosition) {
//!     control.enqueue_projector_checkpoint(position);
//! }
//! ```
//!
//! Whole-frame consumption includes empty progress barriers. Receiving a frame
//! does not acknowledge a durable checkpoint. Save business effects and the
//! complete offset before reporting the source cut as checkpointed.

mod port;
mod types;
mod offset;
#[cfg(unix)]
mod offset_file;
pub mod plugin_v1;

#[cfg(unix)]
pub use offset_file::{FileOffsetStore, OffsetFileFailure, OffsetStoreError, OffsetStoreOperation};

pub use offset::{
    ConsumeOffset, ConsumeOffsetError, MAX_OFFSET_JSON_BYTES, MAX_PROJECTOR_ID_BYTES,
    OffsetJsonCategory, ProjectorId,
};

pub use port::{
    CheckpointCut, CheckpointPublication, CommittedFrame, CommittedTransaction, ProjectionReceiver,
    ProjectorControl,
};
pub use types::*;
pub use statevec_frame::Digest32;
