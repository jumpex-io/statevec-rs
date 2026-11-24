use statevec_macros::record;

#[record(
    kind = 1,
    record_len = 64,
    index(id = 0, name = "by_a", fields = [a]),
    index(id = 1, name = "by_b", fields = [b]),
    index(id = 2, name = "by_c", fields = [c])
)]
pub struct TooManyIndexes {
    #[field(index = 1)]
    pub a: u64,
    #[field(index = 2)]
    pub b: u64,
    #[field(index = 3)]
    pub c: u64,
}

fn main() {}
