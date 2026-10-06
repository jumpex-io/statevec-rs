use statevec_macros::command;

#[command(kind = 1, inline_response = true)]
struct BadCommandInlineFlag {
    #[field(index = 1)]
    amount: u64,
}

fn main() {}
