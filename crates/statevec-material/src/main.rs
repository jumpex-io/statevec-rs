//! SDK-only package authoring. The server distribution adds host-only commands.
fn main() -> std::process::ExitCode {
    #[cfg(target_os = "linux")]
    {
        use statevec_material::authoring;
        // One extra argument detects overflow instead of accepting a truncated command.
        let args: Vec<_> = std::env::args_os().skip(1).take(9).collect();
        let (code, output) = match authoring::execute(&args) {
            Ok(value) => (0, value),
            Err(error) => authoring::failure(error),
        };
        println!("{output}");
        std::process::ExitCode::from(code)
    }
    #[cfg(not(target_os = "linux"))]
    {
        println!("{{\"version\":1,\"category\":\"unsupported_platform\"}}");
        std::process::ExitCode::from(3)
    }
}
