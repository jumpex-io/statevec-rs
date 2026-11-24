// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

use statevec_macros::record;
use statevec_model::GeneratedRecordAccess;

#[record(kind = 1, record_len = 64)]
pub struct DecimalRecord {
    #[field(index = 1)]
    pub amount: statevec_model::Decimal<6>,
}

fn main() {
    let mut buf = [0u8; DecimalRecordAccess::LEN];
    let mut builder = <DecimalRecord as GeneratedRecordAccess>::wrap_new(&mut buf);
    let _: Scale<2> = setter_scale(|value| {
        builder.set_amount(value);
    });
}

// Infer the generated setter's exact scale. A local marker keeps rustc's
// version-dependent shortening of external Decimal paths out of the diagnostic.
struct Scale<const N: u8>;

fn setter_scale<const N: u8>(_: impl FnOnce(statevec_model::Decimal<N>)) -> Scale<N> {
    Scale
}
