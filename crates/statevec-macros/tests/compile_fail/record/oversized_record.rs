use statevec_macros::record;

#[record(kind = 1, record_len = 65536)]
pub struct AboveRuntimeLimit {
    #[field(index = 1)]
    pub value: u64,
}

#[record(kind = 2, record_len = 8589934592)]
pub struct TruncatedMetadata {
    #[field(index = 1)]
    pub value: u64,
}

fn main() {}
