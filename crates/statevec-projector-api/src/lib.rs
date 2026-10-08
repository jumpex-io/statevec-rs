//! Consume complete business output and coordinate a projector's durable checkpoint.
//!
//! Applications implement their read models and stores against these interfaces.
//! The production runtime supplies the source, frames and checkpoint cut tokens.
//! Source attachment, replication, replay, backpressure and checkpoint selection
//! remain runtime responsibilities. This crate contains no runtime or file codec.
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
//! does not acknowledge a durable checkpoint. Only after syncing checkpoint data,
//! its selected manifest and their directories may the store report publication.

mod port;
mod types;

pub use port::{
    CheckpointCut, CheckpointPublication, CommittedFrame, CommittedTransaction, ProjectionReceiver,
    ProjectorControl,
};
pub use types::*;
pub use statevec_frame::{Digest32, outcome::EntryDigest};
