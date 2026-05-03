// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

use super::{parse_payload_args, parse_record_args};
use quote::quote;

#[test]
fn parse_args_do_not_leak_state_after_error() {
    let err = parse_record_args(quote!(kind = 7, pk(unsupported = 1)))
        .err()
        .expect("expected parse error");
    assert!(!err.to_string().is_empty());

    let args = parse_record_args(quote!(record_len = 16)).expect("expected record args");
    assert_eq!(args.kind, 0);
    assert_eq!(args.record_len, 16);
    assert_eq!(args.version, 1);
    assert!(args.pk_fields.is_empty());

    let err = parse_payload_args(quote!(kind = 9, __schema_module_version =))
        .err()
        .expect("expected parse error");
    assert!(!err.to_string().is_empty());

    let err = parse_payload_args(quote!(__schema_module_version = 2))
        .err()
        .expect("expected parse error");
    assert!(
        err.to_string()
            .contains("missing required argument: kind = N")
    );
}
