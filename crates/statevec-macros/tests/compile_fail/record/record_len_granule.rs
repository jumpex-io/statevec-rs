// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

use statevec_macros::record;

// A multiple of 8 but not of 16.
#[record(kind = 1, record_len = 72)]
pub struct NotAGranuleMultiple {
    #[field(index = 1)]
    pub a: u64,
}

// Below the 32-byte minimum; smaller than the 24-byte header.
#[record(kind = 2, record_len = 16)]
pub struct BelowMinimum {
    #[field(index = 1)]
    pub a: u64,
}

fn main() {}
