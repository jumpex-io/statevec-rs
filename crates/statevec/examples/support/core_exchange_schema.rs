use statevec::prelude::*;

#[schema_module(version = "1.0")]
pub mod exchange {
    use super::*;
    // RC1 R14: a non-power-of-two slot (56 data bytes) beside the 64-byte
    // Order, so loaded-Core qualification covers both lengths.
    #[record(kind = 1, record_len = 80, unique_key(id = 0, fields = [trader]))]
    pub struct Position {
        #[field(index = 1, immutable = true)] pub trader: u64,
        #[field(index = 2)] pub available: u64,
        #[field(index = 3)] pub reserved: u64,
    }
    #[record(kind = 2, record_len = 64, unique_key(id = 0, fields = [order_id]), index(id = 0, name = "by_trader", fields = [trader]))]
    pub struct Order {
        #[field(index = 1, immutable = true)] pub order_id: u64,
        #[field(index = 2, immutable = true)] pub trader: u64,
        #[field(index = 3, immutable = true)] pub price: u64,
        #[field(index = 4)] pub remaining: u64,
    }
    #[command(kind = 1)]
    pub struct Credit {
        #[field(index = 1)] pub trader: u64,
        #[field(index = 2)] pub amount: u64,
    }
    #[command(kind = 2)]
    pub struct Place {
        #[field(index = 1)] pub order_id: u64,
        #[field(index = 2)] pub trader: u64,
        #[field(index = 3)] pub price: u64,
        #[field(index = 4)] pub quantity: u64,
    }
    #[command(kind = 3)]
    pub struct Cancel {
        #[field(index = 1)] pub order_id: u64,
    }
    #[command(kind = 4)]
    pub struct Fill {
        #[field(index = 1)] pub order_id: u64,
        #[field(index = 2)] pub quantity: u64,
    }
    #[command(kind = 5)]
    pub struct Audit {
        #[field(index = 1)] pub trader: u64,
        #[field(index = 2)] pub cap: u64,
        #[field(index = 3)] pub panic_in_visitor: u64,
    }
    #[event(kind = 2, inline_response = true)]
    pub struct Audited {
        #[field(index = 1)] pub visited: u64,
        #[field(index = 2)] pub reserved: u64,
        #[field(index = 3)] pub count: u64,
        #[field(index = 4)] pub at_least: u64,
    }
    #[event(kind = 1, inline_response = true)]
    pub struct Changed {
        #[field(index = 1)] pub trader: u64,
        #[field(index = 2)] pub available: u64,
        #[field(index = 3)] pub reserved: u64,
        #[field(index = 4)] pub order_id: u64,
        #[field(index = 5)] pub remaining: u64,
        #[field(index = 6)] pub time_ns: u64,
        #[field(index = 7)] pub position: u64,
    }
}
pub use exchange::*;

#[allow(dead_code)] // Shared by Core and decoder fixtures; only the latter exports IDL here.
pub fn decoder_idl() -> &'static [u8] {
    static IDL: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    IDL.get_or_init(|| registry().to_idl_json()).as_bytes()
}
