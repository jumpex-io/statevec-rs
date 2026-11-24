use statevec_macros::record;

#[record(
    kind = 1,
    record_len = 64,
    unique_key(id = 0, name = "by_a", fields = [a]),
    unique_key(id = 1, name = "by_b", fields = [b]),
    unique_key(id = 2, name = "by_c", fields = [c]),
    unique_key(id = 3, name = "by_d", fields = [d])
)]
pub struct TooManyUk {
    #[field(index = 1, immutable)]
    pub a: u64,
    #[field(index = 2, immutable)]
    pub b: u64,
    #[field(index = 3, immutable)]
    pub c: u64,
    #[field(index = 4, immutable)]
    pub d: u64,
}

fn main() {}
