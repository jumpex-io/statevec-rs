// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

use statevec_macros::record;

#[record(kind = 1, record_len = 64, unique_key(id = 0, fields = [_reserved]))]
pub struct Bad {
    #[field(index = 1, reserved)]
    pub _reserved: u64,
}

fn main() {}
