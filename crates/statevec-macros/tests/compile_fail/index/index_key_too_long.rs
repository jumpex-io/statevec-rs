use statevec_macros::record;

#[record(kind = 1, record_len = 128, index(id = 0, name = "by_large", fields = [a, b]))]
pub struct IndexKeyTooLong {
    #[field(index = 1)]
    pub a: statevec_model::FixedBytes<40>,
    #[field(index = 2)]
    pub b: statevec_model::FixedBytes<40>,
}

fn main() {}
