// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

use statevec_macros::record;

#[record(kind = 61440, record_len = 64)]
pub struct Bad {
    #[field(index = 1)]
    pub value: u64,
}

fn main() {}
