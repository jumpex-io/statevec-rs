mod support;
use serde_json::{Value, json};
use statevec_package::{PluginTarget, hex};
use std::ffi::OsString;
use std::path::Path;
use std::process::Command;
use test_case::test_case;

#[test]
fn closed_stdout_returns_io_exit_for_the_sdk_report() {
    let (reader, writer) = std::io::pipe().unwrap();
    drop(reader);
    let result = Command::new(env!("CARGO_BIN_EXE_sv-package")).stdout(writer).output().unwrap();
    assert_eq!(result.status.code(), Some(3), "{result:?}");
    assert!(result.stderr.is_empty(), "{result:?}");
}

fn declaration(path: &Path) {
    let declared = support::declaration();
    let schema = declared.schema_identity;
    std::fs::write(
        path,
        serde_json::to_vec(&json!({
            "format": statevec_package::DECLARATION_FORMAT,
            "version": statevec_frame::release::PROTOCOL_NUMBER,
            "schema_identity":{"version":hex(&schema.schema_version.to_bytes()),
                "types":hex(&schema.types_schema_fingerprint),"records":hex(&schema.record_schema_fingerprint),
                "commands":hex(&schema.command_schema_fingerprint),"events":hex(&schema.event_schema_fingerprint)},
            "execution_revision":declared.execution_revision,"has_tx_invariants":declared.has_tx_invariants,
        }))
        .unwrap(),
    )
    .unwrap();
}

fn run(args: &[OsString], exit: i32) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_sv-package"))
        .args(args)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(exit), "{args:?}: {output:?}");
    assert!(output.stdout.len() <= 32768);
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(!text.contains("private-config"), "configuration cannot appear in stdout");
    serde_json::from_str(&text).unwrap()
}

#[test_case(&[PluginTarget::X86_64LinuxGnu], "616a15d73d3967c45a94283c6aae96917156524bd91f0f507538854bd3eadafe"; "single_platform")]
#[test_case(&[PluginTarget::X86_64LinuxGnu, PluginTarget::Aarch64LinuxGnu], "08ba236658b8de6e04d9429d5b44a1de30d04e80da9c016a7c18d41d81cd6c25"; "dual_platform")]
#[cfg_attr(not(target_os = "linux"), ignore = "the SDK material binary supports Linux only")]
fn sdk_binary_packs_canonical_bytes_and_never_replaces_output(targets: &[PluginTarget], digest: &str) {
    let directory = tempfile::tempdir().unwrap();
    let declared = directory.path().join("declaration.json");
    let config = directory.path().join("config");
    let package = directory.path().join("core.svtar");
    declaration(&declared);
    std::fs::write(&config, b"private-config").unwrap();
    let mut args = vec![OsString::from("pack"), declared.into(), config.into(), package.clone().into()];
    for target in targets {
        let artifact = directory.path().join(format!("{}.so", target.triple()));
        std::fs::write(&artifact, support::elf_bytes(*target)).unwrap();
        args.extend([OsString::from(target.triple()), artifact.into()]);
    }

    let packed = run(&args, 0);
    assert_eq!(packed["package_digest"], digest);
    assert_eq!(packed["execution"], "not_checked");
    let original = std::fs::read(&package).unwrap();
    assert_eq!(run(&args, 3)["category"], "io");
    assert_eq!(std::fs::read(&package).unwrap(), original);

    let link = directory.path().join("output-link");
    std::os::unix::fs::symlink(&package, &link).unwrap();
    args[3] = link.into();
    assert_eq!(run(&args, 3)["category"], "io");
    assert_eq!(std::fs::read(&package).unwrap(), original);
}

#[test_case(Vec::new(); "missing_command")]
#[test_case(vec!["pack", "d", "c", "o", "bad-target", "artifact"]; "bad_target")]
#[test_case(vec!["pack", "d", "c", "o", "x86_64-unknown-linux-gnu", "a", "aarch64-unknown-linux-gnu", "b", "extra"]; "trailing_argument")]
#[test_case(vec!["verify", "package"]; "execution_is_not_an_sdk_command")]
#[cfg_attr(not(target_os = "linux"), ignore = "the SDK material binary supports Linux only")]
fn usage_errors_are_not_material_refusals(args: Vec<&str>) {
    let args = args.into_iter().map(OsString::from).collect::<Vec<_>>();
    assert_eq!(run(&args, 2)["category"], "invalid_input");
}

#[test]
#[cfg_attr(not(target_os = "linux"), ignore = "the SDK material binary supports Linux only")]
fn sdk_command_refuses_symlink_input_without_touching_output() {
    let directory = tempfile::tempdir().unwrap();
    let declared = directory.path().join("declaration.json");
    declaration(&declared);
    let link = directory.path().join("input-link");
    std::os::unix::fs::symlink(declared, &link).unwrap();
    let output = directory.path().join("never-created.svtar");
    let result = run(
        &[
            "pack".into(),
            link.into(),
            "config".into(),
            output.clone().into(),
            "x86_64-unknown-linux-gnu".into(),
            "artifact".into(),
        ],
        4,
    );
    assert_eq!(result["detail"]["reason"], "not_regular");
    assert!(!output.exists());
}
