// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

use statevec_macros::command;

#[command(kind = 61440)]
pub struct Bad {
    #[field(index = 1)]
    pub value: u64,
}

fn main() {}
