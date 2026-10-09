// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

use crate::{SchemaRegistry, idl::SchemaIdlError};
use serde::de::{DeserializeSeed, Error, IgnoredAny, MapAccess, SeqAccess, Visitor};
use std::fmt;

/// Resource limits checked before allocating the schema document or promoting names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SchemaIdlLimits {
    /// Maximum input length in bytes, including whitespace.
    pub max_bytes: usize,
    /// Maximum number of nested JSON objects/arrays. The root container has depth one.
    pub max_depth: usize,
    /// Maximum JSON values, including containers and values in unknown fields.
    /// Object keys are not additional values; each corresponding value is counted.
    pub max_nodes: usize,
}

impl Default for SchemaIdlLimits {
    fn default() -> Self {
        Self { max_bytes: 16 * 1024 * 1024, max_depth: 32, max_nodes: 262_144 }
    }
}

/// The resource whose schema-import budget was exceeded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaIdlResource {
    Bytes,
    Depth,
    Nodes,
}

/// Typed pre-scan failure or the unchanged schema decoder's cause.
#[derive(Debug)]
pub enum BoundedSchemaIdlError {
    LimitExceeded { resource: SchemaIdlResource, limit: usize, observed: usize },
    /// Malformed JSON, retaining the parser's structured category and position.
    Json(serde_json::Error),
    /// Invalid UTF-8, document shape, schema definitions, or declared identity.
    Schema(SchemaIdlError),
}

impl fmt::Display for BoundedSchemaIdlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LimitExceeded { resource, limit, observed } => {
                write!(f, "schema IDL {resource:?} budget exceeded: {observed} > {limit}")
            }
            Self::Json(error) => write!(f, "invalid schema IDL JSON: {error}"),
            Self::Schema(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for BoundedSchemaIdlError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::LimitExceeded { .. } => None,
            Self::Json(error) => Some(error),
            Self::Schema(error) => Some(error),
        }
    }
}

impl SchemaRegistry {
    /// Imports IDL after a bounded structural scan of the same immutable bytes.
    ///
    /// The scan counts all JSON values (including ignored metadata), checks
    /// container depth, and builds no document tree. The existing schema decoder
    /// then reconstructs and verifies the identity. Its promoted names still
    /// live for the process lifetime: this is a bootstrap operation, not a
    /// request-path or repeated failed-open API. Serde's own recursion limit
    /// remains in force even when the caller supplies a larger depth budget.
    ///
    /// ```
    /// use statevec_model::{SchemaRegistry, Version, idl::SchemaIdlLimits};
    ///
    /// let schema = SchemaRegistry::new(Version::new(1, 0), &[], &[], &[], &[]);
    /// let bytes = schema.to_canonical_idl_bytes();
    /// let imported = SchemaRegistry::from_idl_bytes_with_limits(&bytes, SchemaIdlLimits::default()).unwrap();
    /// assert_eq!(imported.identity(), schema.identity());
    /// ```
    pub fn from_idl_bytes_with_limits(bytes: &[u8], limits: SchemaIdlLimits) -> Result<Self, BoundedSchemaIdlError> {
        if bytes.len() > limits.max_bytes {
            return Err(BoundedSchemaIdlError::LimitExceeded {
                resource: SchemaIdlResource::Bytes,
                limit: limits.max_bytes,
                observed: bytes.len(),
            });
        }
        let json = std::str::from_utf8(bytes)
            .map_err(|_| BoundedSchemaIdlError::Schema(SchemaIdlError::InvalidUtf8))?;
        let mut scan = Scan { limits, nodes: 0, limit_error: None };
        let mut decoder = serde_json::Deserializer::from_str(json);
        let scanned = ScanValue { scan: &mut scan, depth: 0 }
            .deserialize(&mut decoder)
            .and_then(|()| decoder.end());
        if let Err(error) = scanned {
            return Err(scan.limit_error.unwrap_or(BoundedSchemaIdlError::Json(error)));
        }
        Self::from_idl_json(json).map_err(BoundedSchemaIdlError::Schema)
    }
}

// Call-local scanner bookkeeping only. Serde may use bounded string scratch,
// but no Vec/Map of values or schema document is constructed by this visitor.
struct Scan {
    limits: SchemaIdlLimits,
    nodes: usize,
    // Carry our typed cause out of Serde without inspecting its custom error text.
    limit_error: Option<BoundedSchemaIdlError>,
}

impl Scan {
    fn check<E: Error>(&mut self, resource: SchemaIdlResource, limit: usize, observed: usize) -> Result<(), E> {
        if observed > limit {
            self.limit_error = Some(BoundedSchemaIdlError::LimitExceeded { resource, limit, observed });
            return Err(E::custom("schema IDL resource limit exceeded"));
        }
        Ok(())
    }
}

struct ScanValue<'a> {
    scan: &'a mut Scan,
    // Enclosing containers, not a value's position or number of siblings.
    depth: usize,
}

impl<'de> DeserializeSeed<'de> for ScanValue<'_> {
    type Value = ();

    fn deserialize<D: serde::Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        // Each value consumes at least one input byte. The count cannot overflow
        // usize for an existing byte slice, even with a caller-supplied larger cap.
        self.scan.nodes += 1;
        self.scan.check(SchemaIdlResource::Nodes, self.scan.limits.max_nodes, self.scan.nodes)?;
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for ScanValue<'_> {
    type Value = ();

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON value within the schema IDL limits")
    }

    fn visit_bool<E: Error>(self, _: bool) -> Result<(), E> {
        Ok(())
    }

    fn visit_i64<E: Error>(self, _: i64) -> Result<(), E> {
        Ok(())
    }

    fn visit_u64<E: Error>(self, _: u64) -> Result<(), E> {
        Ok(())
    }

    fn visit_f64<E: Error>(self, _: f64) -> Result<(), E> {
        Ok(())
    }

    fn visit_str<E: Error>(self, _: &str) -> Result<(), E> {
        Ok(())
    }

    fn visit_unit<E: Error>(self) -> Result<(), E> {
        Ok(())
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
        let depth = self.depth + 1;
        self.scan.check(SchemaIdlResource::Depth, self.scan.limits.max_depth, depth)?;
        while seq.next_element_seed(ScanValue { scan: self.scan, depth })?.is_some() {}
        Ok(())
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        let depth = self.depth + 1;
        self.scan.check(SchemaIdlResource::Depth, self.scan.limits.max_depth, depth)?;
        while map.next_key::<IgnoredAny>()?.is_some() {
            map.next_value_seed(ScanValue { scan: self.scan, depth })?;
        }
        Ok(())
    }
}
