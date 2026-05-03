// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

use statevec_macros::record;
use statevec_model::FixedBytes;

#[record(kind = 1, record_len = 64, pk(fields = [a,b]))]
pub struct Bad {
    #[field(index = 1, immutable = true)]
    pub a: u64,
    #[field(index = 2, immutable = true)]
    pub b: FixedBytes<64>,
}

fn main() {}
