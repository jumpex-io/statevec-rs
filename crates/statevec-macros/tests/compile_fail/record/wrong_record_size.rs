// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

use statevec_macros::record;
#[allow(unused_imports)]
use statevec_model::FixedBytes;

// FixedBytes<123> occupies 16 + 2 + 123 = 127 bytes, which exceeds record_len = 64
#[record(kind = 3, record_len = 128)]
pub struct WrongSize {
    #[field(index = 1)]
    pub ccy: FixedBytes<123>,
}

fn main() {}
