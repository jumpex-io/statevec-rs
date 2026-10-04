use statevec_macros::record;

#[record(kind = 1, record_len = 64, uk(id = 0, fields = [a]), uk(id = 1, fields = [b]))]
pub struct MultipleAnonymousUk {
    #[field(index = 1, immutable)]
    pub a: u64,
    #[field(index = 2, immutable)]
    pub b: u64,
}

fn main() {}
