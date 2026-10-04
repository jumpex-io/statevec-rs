use statevec_macros::record;

#[record(kind = 1, record_len = 64, index(id = 0, name = "by_missing", fields = [missing]))]
pub struct MissingIndexField {
    #[field(index = 1)]
    pub a: u64,
}

fn main() {}
