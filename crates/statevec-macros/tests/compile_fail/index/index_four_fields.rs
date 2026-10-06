use statevec_macros::record;

#[record(kind = 1, record_len = 64, index(id = 0, name = "by_four", fields = [a, b, c, d]))]
pub struct FourIndexFields {
    #[field(index = 1)]
    pub a: u64,
    #[field(index = 2)]
    pub b: u64,
    #[field(index = 3)]
    pub c: u64,
    #[field(index = 4)]
    pub d: u64,
}

fn main() {}
