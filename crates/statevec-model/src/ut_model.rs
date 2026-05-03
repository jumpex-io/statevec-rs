// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

use crate::read_var_bytes;

#[test]
fn read_var_bytes_rejects_overflowing_end_offset() {
    let err = read_var_bytes(&[], usize::MAX - 1).unwrap_err();
    assert_eq!(err.required, usize::MAX);
    assert_eq!(err.actual, 0);
}
