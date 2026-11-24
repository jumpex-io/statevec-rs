use statevec_macros::event;

struct VarBytes;

#[event(kind = 1, inline_response = "yes")]
struct BadInlineFlag {
    #[field(index = 1)]
    payload: VarBytes,
}

fn main() {}
