use statevec_macros::record;

#[record(
    kind = 1,
    record_len = 64,
    uk(id = 0, name = "by_a", fields = [a]),
    uk(id = 2, name = "by_b", fields = [b])
)]
pub struct UkIdGap {
    #[field(index = 1, immutable)]
    pub a: u64,
    #[field(index = 2, immutable)]
    pub b: u64,
}

fn main() {}
