//! Source metadata for the client-only byte action tape.
pub fn source_snapshot() -> &'static (String, String) {
    static SOURCE: std::sync::OnceLock<(String, String)> = std::sync::OnceLock::new();
    SOURCE.get_or_init(|| {
        let git = |args: &[&str]| {
            let output = std::process::Command::new("git")
                .current_dir(env!("CARGO_MANIFEST_DIR"))
                .args(args)
                .output()
                .expect("recording checkout must expose source provenance");
            assert!(output.status.success(), "cannot record ingress DST source: {args:?}");
            String::from_utf8(output.stdout).expect("Git source metadata must be UTF-8")
        };
        (git(&["rev-parse", "HEAD"]).trim().to_owned(), git(&["diff", "--binary", "HEAD", "--"]))
    })
}
