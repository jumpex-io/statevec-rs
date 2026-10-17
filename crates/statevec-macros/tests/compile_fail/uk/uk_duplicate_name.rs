use statevec_macros::record;

#[record(
    kind = 1,
    record_len = 64,
    unique_key(id = 0, name = "by_value", fields = [a]),
    unique_key(id = 1, name = "by_value", fields = [b])
)]
pub struct DuplicateUkName {
    #[field(index = 1, immutable)]
    pub a: u64,
    #[field(index = 2, immutable)]
    pub b: u64,
}

fn main() {}
