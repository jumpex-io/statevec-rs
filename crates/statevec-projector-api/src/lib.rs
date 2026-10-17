//! Build a configured Projector: configure -> prepare -> run.
//!
//! Start with [`v1::ProjectorModule`], [`v1::ConfiguredProjector`]
//! and [`v1::PreparedProjector`]. The SDK runs the prepared plugin on its
//! worker with borrowed [`v1::ProjectorIo`] and joins scoped services.
//! Receiving a complete entry gives a source token, not proof of durability.
//! Persist effects and offset, declare that fact with [`v1::RaftEntryToken::declare_durable`],
//! then wait for the separate owner decision through [`v1::PendingDurableOffset`].
//! Latest-state instead enumerates a frozen baseline and confirms its installation.
//! Source attachment, replication, replay and checkpoint selection stay native.
//! [`ConsumeOffset`] supplies a bounded four-field UTF-8 properties codec. On Unix,
//! [`FileOffsetStore`] persists it explicitly; applications report durability
//! separately through [`v1::ProjectorControlPort::report_durable_offset`].
//! [`native`] contains the host's owned-entry/receiver/control traits; these are
//! not the borrowed plugin API. [`v1::host`] and [`v1::ffi`] are
//! integration/transport surfaces, not another plugin lifecycle.
//!
//! ```
//! use statevec_projector_api::native::{AppliedRaftEntry, AppliedTx};
//! fn event_count<F: AppliedRaftEntry>(entry: &F) -> usize {
//!     entry.txs().iter().map(|tx| tx.output().events.len()).sum()
//! }
//! ```
//!
//! A trait implementation is not proof of commitment. The production source's
//! concrete entry and token types have private constructors; its control endpoint
//! accepts only that source's concrete report type. Public position and
//! restore metadata cannot manufacture those tokens. Local substitutes may
//! implement these traits for application tests without acquiring runtime authority.
//!
//! ```compile_fail,E0308
//! use statevec_projector_api::{SourcePosition, native::ProjectorControl};
//! fn acknowledge_from_metadata<C: ProjectorControl>(control: &C, position: SourcePosition) {
//!     control.enqueue_durable_offset(position);
//! }
//! ```
//!
//! Whole-entry consumption includes empty progress barriers. Receiving an entry
//! does not acknowledge durable processing. Save business effects and the
//! complete offset before declaring its token durable.

pub mod native;
mod types;
mod offset;
#[cfg(unix)]
mod offset_file;
pub mod v1;

#[cfg(unix)]
pub use offset_file::{FileOffsetStore, OffsetFileError, OffsetStoreError, OffsetStoreOperation};

pub use offset::{
    ConsumeOffset, ConsumeOffsetError, MAX_OFFSET_PROPERTIES_BYTES, MAX_PROJECTOR_ID_BYTES,
    OffsetField, ProjectorId,
};

pub use types::*;
pub use statevec_frame::Digest32;
