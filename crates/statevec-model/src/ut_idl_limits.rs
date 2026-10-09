// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

use crate::{
    SchemaRegistry, Version,
    idl::{BoundedSchemaIdlError, SchemaIdlError, SchemaIdlLimits, SchemaIdlResource},
};
use test_case::test_case;

fn empty_registry() -> SchemaRegistry {
    SchemaRegistry::new(Version::new(1, 0), &[], &[], &[], &[])
}

fn with_extension(value: &str) -> Vec<u8> {
    let mut json = String::from_utf8(empty_registry().to_canonical_idl_bytes()).unwrap();
    assert_eq!(json.pop(), Some('}'));
    json.push_str(",\"extension\":");
    json.push_str(value);
    json.push('}');
    json.into_bytes()
}

fn assert_limit(error: BoundedSchemaIdlError, resource: SchemaIdlResource, limit: usize, observed: usize) {
    assert!(
        matches!(error, BoundedSchemaIdlError::LimitExceeded {
            resource: actual_resource, limit: actual_limit, observed: actual_observed,
        } if (actual_resource, actual_limit, actual_observed) == (resource, limit, observed)),
        "expected {resource:?} limit {limit} at {observed}, got {error:?}"
    );
}

#[test]
fn bounded_idl_default_byte_limit_is_inclusive() {
    let limits = SchemaIdlLimits::default();
    assert_eq!(limits.max_bytes, 16 * 1024 * 1024);
    let registry = empty_registry();
    let mut bytes = registry.to_canonical_idl_bytes();
    bytes.resize(limits.max_bytes, b' ');
    assert_eq!(SchemaRegistry::from_idl_bytes_with_limits(&bytes, limits).unwrap().identity(), registry.identity());

    bytes.push(b' ');
    assert_limit(
        SchemaRegistry::from_idl_bytes_with_limits(&bytes, limits).unwrap_err(),
        SchemaIdlResource::Bytes,
        limits.max_bytes,
        limits.max_bytes + 1,
    );
}

#[test]
fn bounded_idl_byte_budget_precedes_utf8_decode() {
    assert_limit(
        SchemaRegistry::from_idl_bytes_with_limits(&[0xff; 9], SchemaIdlLimits { max_bytes: 8, ..Default::default() })
            .unwrap_err(),
        SchemaIdlResource::Bytes,
        8,
        9,
    );
}

#[test]
fn bounded_idl_default_depth_limit_is_inclusive() {
    let limits = SchemaIdlLimits::default();
    assert_eq!(limits.max_depth, 32);
    // Root object plus 31 nested arrays; the innermost scalar adds no container depth.
    let at_limit = format!("{}0{}", "[".repeat(31), "]".repeat(31));
    let decoded = SchemaRegistry::from_idl_bytes_with_limits(&with_extension(&at_limit), limits).unwrap();
    assert_eq!(decoded.identity(), empty_registry().identity());

    let over_limit = format!("[{at_limit}]");
    assert_limit(
        SchemaRegistry::from_idl_bytes_with_limits(&with_extension(&over_limit), limits).unwrap_err(),
        SchemaIdlResource::Depth,
        32,
        33,
    );
}

fn json_nodes(value: &serde_json::Value) -> usize {
    1 + match value {
        serde_json::Value::Array(values) => values.iter().map(json_nodes).sum(),
        serde_json::Value::Object(values) => values.values().map(json_nodes).sum(),
        _ => 0,
    }
}

#[test]
fn bounded_idl_default_node_limit_is_inclusive() {
    let limits = SchemaIdlLimits::default();
    assert_eq!(limits.max_nodes, 262_144);
    let empty_array = with_extension("[]");
    let existing = json_nodes(&serde_json::from_slice(&empty_array).unwrap());
    let members = limits.max_nodes - existing;
    let at_limit = format!("[{}0]", "0,".repeat(members - 1));
    assert_eq!(
        SchemaRegistry::from_idl_bytes_with_limits(&with_extension(&at_limit), limits).unwrap().identity(),
        empty_registry().identity(),
    );

    let over_limit = format!("[{}0]", "0,".repeat(members));
    assert_limit(
        SchemaRegistry::from_idl_bytes_with_limits(&with_extension(&over_limit), limits).unwrap_err(),
        SchemaIdlResource::Nodes,
        limits.max_nodes,
        limits.max_nodes + 1,
    );
}

#[test_case(b"[[[]]]", SchemaIdlLimits { max_depth: 2, ..SchemaIdlLimits::default() }, SchemaIdlResource::Depth, 2, 3; "depth_before_document")]
#[test_case(b"[0,0,0]", SchemaIdlLimits { max_nodes: 3, ..SchemaIdlLimits::default() }, SchemaIdlResource::Nodes, 3, 4; "nodes_before_document")]
fn bounded_idl_prescan_precedes_document_decode(
    bytes: &[u8],
    limits: SchemaIdlLimits,
    resource: SchemaIdlResource,
    limit: usize,
    observed: usize,
) {
    // These are valid JSON but not IDL documents: allocating/deserializing the
    // document first would return its shape error instead of the resource cause.
    assert_limit(SchemaRegistry::from_idl_bytes_with_limits(bytes, limits).unwrap_err(), resource, limit, observed);
}

#[test_case(b"0", SchemaIdlLimits { max_bytes: 0, ..SchemaIdlLimits::default() }, SchemaIdlResource::Bytes; "no_bytes")]
#[test_case(b"{}", SchemaIdlLimits { max_depth: 0, ..SchemaIdlLimits::default() }, SchemaIdlResource::Depth; "no_containers")]
#[test_case(b"{}", SchemaIdlLimits { max_nodes: 0, ..SchemaIdlLimits::default() }, SchemaIdlResource::Nodes; "no_values")]
fn bounded_idl_zero_budget_is_not_unlimited(bytes: &[u8], limits: SchemaIdlLimits, resource: SchemaIdlResource) {
    assert_limit(SchemaRegistry::from_idl_bytes_with_limits(bytes, limits).unwrap_err(), resource, 0, 1);
}

#[test]
fn bounded_idl_depth_counts_objects_and_arrays_together() {
    let bytes = with_extension("{\"array\":[{}]}");
    let limits = SchemaIdlLimits { max_depth: 4, ..Default::default() };
    assert_eq!(
        SchemaRegistry::from_idl_bytes_with_limits(&bytes, limits).unwrap().identity(),
        empty_registry().identity(),
    );
    assert_limit(
        SchemaRegistry::from_idl_bytes_with_limits(&bytes, SchemaIdlLimits { max_depth: 3, ..limits }).unwrap_err(),
        SchemaIdlResource::Depth,
        3,
        4,
    );
}

#[test]
fn bounded_idl_counts_repeated_ignored_members() {
    // Deserializing into Value first would collapse the repeated key and lose
    // the input's actual resource count. Every encountered value is charged.
    let mut bytes = with_extension("{\"same\":0,\"same\":0}");
    let deduplicated = json_nodes(&serde_json::from_slice(&bytes).unwrap());
    let limits = SchemaIdlLimits { max_nodes: deduplicated, ..Default::default() };
    assert_limit(
        SchemaRegistry::from_idl_bytes_with_limits(&bytes, limits).unwrap_err(),
        SchemaIdlResource::Nodes,
        deduplicated,
        deduplicated + 1,
    );
    bytes = with_extension("{\"same\":0}");
    assert_eq!(
        SchemaRegistry::from_idl_bytes_with_limits(&bytes, limits).unwrap().identity(),
        empty_registry().identity(),
    );
}

#[test_case("[null,true,false,1,-1,1.5,\"[\\\"{}]\",{},[]]"; "all_value_kinds")]
#[test_case("{\"escaped\\u005bkey\": {\"inner\": 0}, \"other\": []}"; "object_values")]
fn bounded_idl_counts_values_not_brackets_in_strings(extension: &str) {
    let bytes = with_extension(extension);
    let nodes = json_nodes(&serde_json::from_slice(&bytes).unwrap());
    let limits = SchemaIdlLimits { max_nodes: nodes, ..Default::default() };
    assert_eq!(
        SchemaRegistry::from_idl_bytes_with_limits(&bytes, limits).unwrap().identity(),
        empty_registry().identity(),
    );
    assert_limit(
        SchemaRegistry::from_idl_bytes_with_limits(&bytes, SchemaIdlLimits { max_nodes: nodes - 1, ..limits })
            .unwrap_err(),
        SchemaIdlResource::Nodes,
        nodes - 1,
        nodes,
    );
}

#[test_case(b"{", serde_json::error::Category::Eof; "truncated_object")]
#[test_case(b"[0,]", serde_json::error::Category::Syntax; "trailing_comma")]
#[test_case(b"\"\\q\"", serde_json::error::Category::Syntax; "invalid_escape")]
#[test_case(b"{}{}", serde_json::error::Category::Syntax; "trailing_document")]
#[test_case(b"{\"a\":01}", serde_json::error::Category::Syntax; "invalid_number")]
fn bounded_idl_retains_json_cause(bytes: &[u8], category: serde_json::error::Category) {
    let error = SchemaRegistry::from_idl_bytes_with_limits(bytes, SchemaIdlLimits::default()).unwrap_err();
    let BoundedSchemaIdlError::Json(source) = error else {
        panic!("expected a structured pre-scan parser cause, got {error:?}");
    };
    assert_eq!(source.classify(), category);
    assert!(source.line() > 0);
}

#[test]
fn bounded_idl_retains_schema_causes() {
    let limits = SchemaIdlLimits::default();
    assert!(matches!(
        SchemaRegistry::from_idl_bytes_with_limits(&[0xff], limits),
        Err(BoundedSchemaIdlError::Schema(SchemaIdlError::InvalidUtf8))
    ));
    let registry = empty_registry();
    let mut doc: serde_json::Value = serde_json::from_slice(&registry.to_canonical_idl_bytes()).unwrap();
    let fingerprint = doc["fingerprints"]["records"].as_str().unwrap().to_owned();
    let replacement = if fingerprint.starts_with('0') { "1" } else { "0" };
    doc["fingerprints"]["records"] = serde_json::Value::String(format!("{replacement}{}", &fingerprint[1..]));
    let error = SchemaRegistry::from_idl_bytes_with_limits(doc.to_string().as_bytes(), limits).unwrap_err();
    assert!(matches!(
        error,
        BoundedSchemaIdlError::Schema(SchemaIdlError::FingerprintMismatch { declared, computed })
            if computed == registry.identity() && declared != computed
    ), "schema fingerprint cause must survive: {error:?}");
}
