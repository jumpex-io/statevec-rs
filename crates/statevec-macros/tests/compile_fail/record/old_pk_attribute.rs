use statevec_macros::record;

#[record(kind = 1, record_len = 64, pk(fields = [id]))]
pub struct Bad {
    #[field(index = 1, immutable = true)]
    pub id: u64,
}

fn main() {}
