use statevec_macros::record;

#[record(
    kind = 1,
    record_len = 64,
    index(id = 1, name = "by_a", fields = [a])
)]
pub struct IndexIdGap {
    #[field(index = 1)]
    pub a: u64,
}

fn main() {}
