// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

#[allow(unused_imports)]
use super::*;
use statevec::schema_module;
#[allow(unused_imports)]
use statevec::{statevec_api, statevec_model};

mod bank {
    use super::*;
    use statevec::{command, event, record};

    #[schema_module(version = "1.0")]
    pub mod v1_0 {
        use super::*;

        #[record(kind = 1, record_len = 64, pk(fields = [account_id]))]
        pub struct Account {
            #[field(index = 1, immutable = true)]
            pub account_id: u64,
            #[field(index = 2)]
            pub total_credit: u64,
            #[field(index = 3)]
            pub total_debit: u64,
        }

        #[command(kind = 1)]
        pub struct Deposit {
            #[field(index = 1)]
            pub account_id: u64,
            #[field(index = 2)]
            pub amount: u64,
        }

        #[command(kind = 2)]
        pub struct Withdraw {
            #[field(index = 1)]
            pub account_id: u64,
            #[field(index = 2)]
            pub amount: u64,
        }

        #[command(kind = 3)]
        pub struct Transfer {
            #[field(index = 1)]
            pub from_account_id: u64,
            #[field(index = 2)]
            pub to_account_id: u64,
            #[field(index = 3)]
            pub amount: u64,
        }

        #[event(kind = 1)]
        pub struct BalanceChanged {
            #[field(index = 1)]
            pub account_id: u64,
            #[field(index = 2)]
            pub new_credit: u64,
            #[field(index = 3)]
            pub new_debit: u64,
            #[field(index = 4)]
            pub entry_type: u8,
        }
    }
}

pub use bank::v1_0::*;
