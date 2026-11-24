use statevec_macros::record;

#[record(kind = 1, record_len = 64, index(id = 0, fields = [a]))]
pub struct MissingIndexName {
    #[field(index = 1)]
    pub a: u64,
}

fn main() {}
