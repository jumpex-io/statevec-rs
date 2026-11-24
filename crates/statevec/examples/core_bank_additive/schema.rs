//! Additive Bank target. Old payload definitions keep their source versions;
//! the schema module below owns only the new/extended definitions.
use statevec::prelude::*;
#[path = "../core_bank/schema.rs"]
pub mod source;
pub use source::{Balance, Deposit, Transfer};

#[schema_module(version = "2.0")]
mod additions {
    use super::*;
    #[record(kind = 1, record_len = 128,
        unique_key(id = 0, fields = [account_id]),
        index(id = 0, name = "by_credit", fields = [credit]))]
    pub struct Account {
        #[field(index = 1, immutable = true)]
        pub account_id: u64,
        #[field(index = 2)]
        pub credit: u64,
        #[field(index = 3)]
        pub debit: u64,
        #[field(index = 4)]
        pub time_ns: u64,
        #[field(index = 5)]
        pub position: u64,
        #[field(index = 6)]
        pub tag: u64,
    }
    #[record(kind = 2, record_len = 64, unique_key(id = 0, fields = [tag]))]
    pub struct Receipt {
        #[field(index = 1, immutable = true)]
        pub tag: u64,
    }
    #[command(kind = 3)]
    pub struct TagAccount {
        #[field(index = 1)]
        pub account_id: u64,
        #[field(index = 2)]
        pub tag: u64,
    }
    #[event(kind = 2)]
    pub struct Activated {
        #[field(index = 1)]
        pub marker: u64,
    }
}
pub use additions::{Account, Activated, Receipt, TagAccount};

pub fn registry() -> SchemaRegistry {
    // Preserve the original unnamed key verbatim. The macro requires names
    // for a multi-key declaration, so append only the new host-generic key.
    static KEYS: std::sync::OnceLock<Vec<statevec::model::UniqueKeyDefinition>> = std::sync::OnceLock::new();
    let mut account = *Account::definition();
    account.unique_keys = KEYS.get_or_init(|| {
        let mut keys = Account::definition().unique_keys.to_vec();
        keys.push(statevec::model::UniqueKeyDefinition {
            id: 1, name: "by_account", fields: &["account_id"], encode: None,
        });
        keys
    });
    // A hand-built registry carries no validators: attach one per command.
    SchemaRegistry::new(
        Version::new(2, 0),
        &[account, *Receipt::definition()],
        &[*Deposit::definition(), *Transfer::definition(), *TagAccount::definition()],
        &[*Balance::definition(), *Activated::definition()],
        &[],
    )
    .with_command_validator::<Deposit>()
    .with_command_validator::<Transfer>()
    .with_command_validator::<TagAccount>()
}

#[allow(dead_code)]
pub fn decoder_idl() -> &'static [u8] {
    static IDL: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    IDL.get_or_init(|| registry().to_idl_json()).as_bytes()
}
