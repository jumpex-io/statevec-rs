use statevec_macros::record;

#[record(kind = 1, record_len = 64, index(name = "by_value", fields = [a]))]
pub struct MissingIndexId {
    #[field(index = 1)]
    pub a: u64,
}

fn main() {}
