use statevec_macros::record;

#[record(kind = 1, record_len = 64, index(id = 0, name = "by_big", fields = [big]))]
pub struct UnsupportedIndexType {
    #[field(index = 1)]
    pub big: u128,
}

fn main() {}
