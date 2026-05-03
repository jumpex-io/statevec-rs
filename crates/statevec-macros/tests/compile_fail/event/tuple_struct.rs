// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

use statevec_macros::event;

#[event(kind = 1)]
pub struct Bad(u64, u64);

fn main() {}
