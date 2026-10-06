// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

//! Whole-batch ingress wire protocol and generic StateVec envelope adapters.
//!
//! This crate depends on `statevec-frame`, but not on engine execution,
//! raft-rs or domain/demo command types. Retired singleton wire formats have no
//! decoder or compatibility fallback.

use std::error::Error;
use std::fmt;

use serde::{Deserialize, Serialize};
use statevec_frame::{EngineInput, InputRef};

pub mod stream_batch;

/// Result type for ingress adapter operations.
pub type IngressResult<T> = Result<T, IngressError>;

/// Error returned by ingress adapters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IngressError {
    message: String,
}

impl IngressError {
    /// Creates an ingress error from a message.
    pub fn new(message: impl Into<String>) -> Self {
        Self { message: message.into() }
    }
}

impl fmt::Display for IngressError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl Error for IngressError {}

/// Ingress-decoded command envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IngressEnvelope<C> {
    /// Optional source evidence.
    pub input_ref: Option<InputRef>,
    /// Decoded command.
    pub command: C,
}

impl<C> From<IngressEnvelope<C>> for EngineInput<C> {
    fn from(envelope: IngressEnvelope<C>) -> Self {
        Self { input_ref: envelope.input_ref, command: envelope.command }
    }
}

/// Adapter from transport request into an ingress envelope.
pub trait IngressAdapter {
    /// Transport request type.
    type Request;
    /// Command type.
    type Command;

    /// Decodes a request into an ingress envelope.
    fn decode_request(&self, request: Self::Request) -> IngressResult<IngressEnvelope<Self::Command>>;
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use super::*;

    #[test]
    fn ingress_envelope_converts_to_engine_input_with_input_ref_identity() {
        let envelope = IngressEnvelope {
            input_ref: Some(InputRef {
                client_id: 42,
                stream_id: 0,
                client_seq: 7,
                request_id: Some("request-7".to_string()),
            }),
            command: vec![1, 2, 3],
        };

        let input: EngineInput<Vec<u8>> = envelope.into();

        assert_eq!(input.command, vec![1, 2, 3]);
        let input_ref = input.input_ref.expect("input reference should be preserved");
        assert_eq!(input_ref.client_id, 42);
        assert_eq!(input_ref.stream_id, 0);
        assert_eq!(input_ref.client_seq, 7);
        assert_eq!(input_ref.request_id.as_deref(), Some("request-7"));
    }

    #[test]
    fn ingress_error_display_is_typed_and_has_no_source() {
        let err = IngressError::new("bad request");
        assert_eq!(err.to_string(), "bad request");
        assert!(err.source().is_none());
    }
}
