// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

use statevec_macros::EnumU8;

#[derive(EnumU8)]
#[repr(u8)]
pub enum Bad {
    A = 0,
    B,
}

fn main() {}
