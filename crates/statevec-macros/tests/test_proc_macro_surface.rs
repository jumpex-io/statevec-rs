// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

//! Auxiliary tests for proc-macro parser surface compatibility.

#[test]
fn syn_can_parse_schema_module_item_mod() {
    let src = r#"
        pub mod v1_0 {
            use super::*;

            #[record(kind = 1, record_len = 64, pk(fields = [asset_id]))]
            pub struct Asset {
                #[field(index = 1, immutable)]
                pub asset_id: u64,
            }

            #[command(kind = 1)]
            pub struct AddAsset {
                #[field(index = 1)]
                pub asset_id: u64,
            }
        }
    "#;

    syn::parse_str::<syn::ItemMod>(src).expect("item mod should parse");
}
