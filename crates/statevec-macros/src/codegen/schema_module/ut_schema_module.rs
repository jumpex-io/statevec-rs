// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

use super::attr_has_version_arg;
use syn::parse_quote;

#[test]
fn attr_has_version_arg_ignores_version_in_value_position() {
    let attr: syn::Attribute = parse_quote!(#[record(kind = version, record_len = 64)]);
    assert!(!attr_has_version_arg(&attr).unwrap());
}

#[test]
fn attr_has_version_arg_detects_version_keys() {
    let attr: syn::Attribute = parse_quote!(#[record(version = "1.0", record_len = 64)]);
    assert!(attr_has_version_arg(&attr).unwrap());

    let attr: syn::Attribute = parse_quote!(#[record(__schema_module_version = 12, record_len = 64)]);
    assert!(attr_has_version_arg(&attr).unwrap());
}
