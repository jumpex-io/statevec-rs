// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

use statevec_macros::record;

#[record(kind = 0, record_len = 64)]
pub struct Bad {
    #[field(index = 1)]
    pub a: u64,

    #[field(index = 2)]
    pub b: u64,
}

fn main() {}
