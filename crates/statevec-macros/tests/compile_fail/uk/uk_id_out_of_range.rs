use statevec_macros::record;

#[record(kind = 1, record_len = 64, uk(id = 3, fields = [a]))]
pub struct UkIdOutOfRange {
    #[field(index = 1, immutable)]
    pub a: u64,
}

fn main() {}
