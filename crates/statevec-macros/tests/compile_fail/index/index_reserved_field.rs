use statevec_macros::record;

#[record(kind = 1, record_len = 64, index(id = 0, name = "by_reserved", fields = [_reserved]))]
pub struct ReservedIndexField {
    #[field(index = 1, reserved)]
    pub _reserved: u64,
}

fn main() {}
