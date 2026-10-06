use statevec_macros::record;

#[record(
    kind = 1,
    record_len = 64,
    index(id = 0, name = "by_a", fields = [a]),
    index(id = 0, name = "by_b", fields = [b])
)]
pub struct DuplicateIndexId {
    #[field(index = 1)]
    pub a: u64,
    #[field(index = 2)]
    pub b: u64,
}

fn main() {}
