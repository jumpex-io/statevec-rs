use statevec_macros::record;

#[record(kind = 1, record_len = 128, index(id = 0, name = "by_decimal", fields = [prefix, value]))]
pub struct DecimalIndexTooLong {
    #[field(index = 1)]
    pub prefix: statevec_model::FixedBytes<49>,
    #[field(index = 2)]
    pub value: statevec_model::Decimal<8>,
}

fn main() {}
