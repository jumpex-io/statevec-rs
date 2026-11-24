//! SDK-only package authoring. The server distribution adds host-only commands.
use std::io::Write;

fn main() -> std::process::ExitCode {
    #[cfg(target_os = "linux")]
    let (code, output) = {
        use statevec_package::authoring;
        // One extra argument detects overflow instead of accepting a truncated command.
        let args: Vec<_> = std::env::args_os().skip(1).take(9).collect();
        match authoring::execute(&args) {
            Ok(value) => (0, value),
            Err(error) => authoring::failure(error),
        }
    };
    #[cfg(not(target_os = "linux"))]
    let (code, output) = (3, serde_json::json!({"category":"unsupported_platform"}));

    let mut stdout = std::io::stdout().lock();
    if serde_json::to_writer(&mut stdout, &statevec_package::PackageToolOutput::new(&output)).is_err()
        || writeln!(stdout).is_err()
    {
        return 3.into();
    }
    code.into()
}
