// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

use statevec_macros::record;
use statevec_model::{Decimal, GeneratedRecordAccess};

#[record(kind = 1, record_len = 64)]
pub struct DecimalRecord {
    #[field(index = 1)]
    pub amount: Decimal<6>,
}

fn main() {
    let mut buf = [0u8; DecimalRecordAccess::LEN];
    let mut builder = <DecimalRecord as GeneratedRecordAccess>::wrap_new(&mut buf);
    builder.set_amount(Decimal::<2>::from_mantissa(123));
}

// Keep `Decimal` non-unique in this crate so rustc always prints the full
// `statevec_model::Decimal` path. Path trimming otherwise depends on the
// target's dependency graph and made this expectation platform-specific.
#[allow(dead_code)]
mod path_shadow {
    pub struct Decimal;
}
