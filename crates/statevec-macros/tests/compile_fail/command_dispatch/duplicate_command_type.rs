// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

use statevec_macros::command_dispatch;
use statevec_api::TxContext;

struct DemoRuntime;
struct DemoError;
struct AddAsset;

impl DemoRuntime {
    fn handle_add_asset<Tx>(&self, _ctx: &mut Tx, _cmd: &AddAsset) -> Result<(), DemoError>
    where
        Tx: TxContext,
        DemoError: From<Tx::Error>,
    {
        Ok(())
    }

    fn handle_add_asset_again<Tx>(&self, _ctx: &mut Tx, _cmd: &AddAsset) -> Result<(), DemoError>
    where
        Tx: TxContext,
        DemoError: From<Tx::Error>,
    {
        Ok(())
    }

    command_dispatch! {
        fn try_dispatch;
        runtime = DemoRuntime;
        error = DemoError;
        AddAsset => Self::handle_add_asset,
        AddAsset => Self::handle_add_asset_again
    }
}

fn main() {}
