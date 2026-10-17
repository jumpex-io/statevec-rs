use statevec_macros::record;

#[record(kind = 1, record_len = 64, unique_key(fields = [a]))]
pub struct MissingUkId {
    #[field(index = 1, immutable)]
    pub a: u64,
}

fn main() {}
