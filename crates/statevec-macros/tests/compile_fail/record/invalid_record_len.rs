// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

use statevec_macros::record;

#[record(kind = 1, record_len = 100)]
pub struct Bad {
    #[field(index = 1)]
    pub a: u64,
}

fn main() {}
