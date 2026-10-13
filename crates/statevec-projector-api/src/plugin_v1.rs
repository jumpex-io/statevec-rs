//! Projector V1 transport. Raw tables do not grant source or publication authority.
//! Core V3 shares scalar representations only; its operation decoder is unchanged.
pub mod ffi;
mod failure;
mod status;
mod completion;
mod values;

pub use statevec_api::plugin_v3::{ConfigureFailure, HostFailure, ResourceFailure, SvStatus};
pub use failure::*;
pub use status::{decode_call, decode_failure};
pub use completion::{ProjectorCompletion, decode_observation};

#[cfg(test)]
#[path = "ut_plugin_v1.rs"]
mod tests;
