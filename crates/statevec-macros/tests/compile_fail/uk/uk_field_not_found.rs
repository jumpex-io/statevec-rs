// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

use statevec_macros::record;

#[record(kind = 1, record_len = 64, uk(id = 0, fields = [missing_field]))]
pub struct Bad {
    #[field(index = 1)]
    pub a: u64,
}

fn main() {}
