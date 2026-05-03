// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

use statevec_macros::command;

#[command(kind = 1)]
struct BadCommand {
    #[field(index = 1)]
    #[field(index = 2)]
    user_id: u64,
}

fn main() {}
