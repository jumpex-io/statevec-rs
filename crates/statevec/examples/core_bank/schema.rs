use statevec::prelude::*;

#[schema_module(version = "1.0")]
pub mod bank {
    use super::*;
    #[record(kind = 1, record_len = 128, unique_key(id = 0, fields = [account_id]))]
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
    }
    #[command(kind = 1)]
    pub struct Deposit {
        #[field(index = 1)]
        pub account_id: u64,
        #[field(index = 2)]
        pub amount: u64,
    }
    #[command(kind = 2)]
    pub struct Transfer {
        #[field(index = 1)]
        pub from: u64,
        #[field(index = 2)]
        pub to: u64,
        #[field(index = 3)]
        pub amount: u64,
    }
    #[event(kind = 1, inline_response = true)]
    pub struct Balance {
        #[field(index = 1)]
        pub account_id: u64,
        #[field(index = 2)]
        pub credit: u64,
        #[field(index = 3)]
        pub debit: u64,
        #[field(index = 4)]
        pub time_ns: u64,
        #[field(index = 5)]
        pub position: u64,
    }
}
pub use bank::*;

/// This example Projector decodes the same Bank schema as the Core example.
#[allow(dead_code)] // Shared by Core and decoder fixtures; only the latter exports IDL here.
pub fn decoder_idl() -> &'static [u8] {
    static IDL: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    IDL.get_or_init(|| registry().to_idl_json()).as_bytes()
}
