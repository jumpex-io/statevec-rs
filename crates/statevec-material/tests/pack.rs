#![cfg(target_os = "linux")]
mod support;
use statevec_material::*;
use statevec_model::CoreVersion;
use std::io::{self, Write};
use std::os::unix::fs::FileExt;
use test_case::test_case;

fn artifact(target: PluginTarget) -> std::fs::File {
    let mut file = tempfile::tempfile().unwrap();
    file.write_all(&support::elf_bytes(target)).unwrap();
    file
}

// Independent Python hashlib/json/TarInfo(USTAR_FORMAT) answers, ABI 5.
#[test_case(&[PluginTarget::X86_64LinuxGnu], 4608, "3d62766e4104b9279c2319627d1160e6a3713cd7479da917b949a558ca0e02b2"; "x86")]
#[test_case(&[PluginTarget::Aarch64LinuxGnu], 4608, "f5c2e026acaa752589a6547d7477ac4291d1251585e2cbe67adbdef0038a8e5c"; "arm")]
#[test_case(&[PluginTarget::X86_64LinuxGnu, PluginTarget::Aarch64LinuxGnu], 5632, "0c31698671a90f971a29550472332c8c10f23e213859b492f32dadb5eb3fdbce"; "both")]
fn archive_bytes_match_independent_known_answers(targets: &[PluginTarget], length: usize, digest: &str) {
    let artifacts: Vec<_> = targets.iter().map(|target| (*target, artifact(*target))).collect();
    let mut bytes = Vec::new();
    let id = pack(&mut bytes, support::declaration(), b"private-config", &artifacts).unwrap();

    assert_eq!(bytes.len(), length);
    assert_eq!(hex(&id.package_digest.bytes()), digest);
    assert_eq!(hex(&id.core_version.digest().bytes()), "8a021e94600ab1e74b0a5927279e10d5d41f3d6b937d75312299702292215625");
    let mut again = Vec::new();
    assert_eq!(pack(&mut again, support::declaration(), b"private-config", &artifacts).unwrap(), id);
    assert_eq!(bytes, again, "the held input descriptors may be reused without cursor-dependent output");
}

#[test_case(true, "f25f71d1baf90556437674b1bb6059f6ed7e7ed2e60d73581d861692cfb4127d"; "rules_on")]
#[test_case(false, "fc1bf8e527a3ed0dec1698b00eef12cea3766251b7a4400fb80b291c8e4312ba"; "rules_off")]
fn sdk_derives_the_frozen_engine_semantic_version(rules: bool, expected: &str) {
    let schema = statevec_model::SchemaIdentity {
        schema_version: statevec_model::Version::new(2, 3),
        types_schema_fingerprint: [0x11; 16], record_schema_fingerprint: [0x22; 16],
        command_schema_fingerprint: [0x33; 16], event_schema_fingerprint: [0x44; 16],
    };
    assert_eq!(hex(&CoreVersion::derive(schema, "rules;limit=17", rules).digest().bytes()), expected);
}

#[test_case(""; "empty")]
#[test_case("visible\0hidden"; "nul")]
fn invalid_revision_refuses_before_archive_output(revision: &str) {
    let mut declaration = support::declaration();
    declaration.execution_revision = revision.into();
    let mut output = Vec::new();
    let result = pack(&mut output, declaration, b"", &[(PluginTarget::X86_64LinuxGnu, artifact(PluginTarget::X86_64LinuxGnu))]);
    assert!(matches!(result, Err(CoreMaterialError::Manifest { field: ManifestField::Revision })), "{result:?}");
    assert!(output.is_empty());
}

#[test_case(&[PluginTarget::X86_64LinuxGnu, PluginTarget::X86_64LinuxGnu]; "duplicate_target")]
#[test_case(&[PluginTarget::Aarch64LinuxGnu, PluginTarget::X86_64LinuxGnu]; "reversed_targets")]
#[test_case(&[]; "missing_artifact")]
fn noncanonical_target_list_refuses_before_output(targets: &[PluginTarget]) {
    let artifacts: Vec<_> = targets.iter().map(|target| (*target, artifact(*target))).collect();
    let mut output = Vec::new();
    let result = pack(&mut output, support::declaration(), b"", &artifacts);
    assert!(matches!(result, Err(CoreMaterialError::Manifest { field: ManifestField::Artifacts })), "{result:?}");
    assert!(output.is_empty());
}

#[test]
fn configuration_limit_precedes_writing() {
    let mut output = Vec::new();
    let artifacts = [(PluginTarget::X86_64LinuxGnu, artifact(PluginTarget::X86_64LinuxGnu))];
    let result = pack(&mut output, support::declaration(), &vec![0; MAX_CORE_CONFIG_BYTES + 1], &artifacts);
    assert!(matches!(result, Err(CoreMaterialError::Manifest { field: ManifestField::Configuration })), "{result:?}");
    assert!(output.is_empty());
}

#[test]
fn artifact_target_must_match_elf_before_writing() {
    let mut output = Vec::new();
    let result = pack(&mut output, support::declaration(), b"", &[(PluginTarget::Aarch64LinuxGnu, artifact(PluginTarget::X86_64LinuxGnu))]);
    assert!(matches!(result, Err(CoreMaterialError::Artifact(PluginMaterialFailure::Elf(PluginElfFailure::MachineMismatch { expected: 183, observed: 62 })))), "{result:?}");
    assert!(output.is_empty());
}

#[test]
fn source_change_between_hash_and_copy_is_not_a_successful_package() {
    struct ChangingOutput { file: std::fs::File, bytes: Vec<u8> }
    impl Write for ChangingOutput {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            // pack hashes before its first output; mutate the very same inode.
            self.file.write_all_at(b"changed!", 0)?;
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> { Ok(()) }
    }
    let artifact = artifact(PluginTarget::X86_64LinuxGnu);
    let mut output = ChangingOutput { file: artifact.try_clone().unwrap(), bytes: Vec::new() };
    let result = pack(&mut output, support::declaration(), b"", &[(PluginTarget::X86_64LinuxGnu, artifact)]);
    assert!(matches!(result, Err(CoreMaterialError::Digest { expected, actual }) if expected != actual));
    assert!(!output.bytes.is_empty(), "caller retains partial output, not a valid archive receipt");
}

#[test]
fn failed_write_preserves_the_physical_cause() {
    struct RefuseWrite;
    impl Write for RefuseWrite {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> { Err(io::ErrorKind::StorageFull.into()) }
        fn flush(&mut self) -> io::Result<()> { Ok(()) }
    }
    let result = pack(&mut RefuseWrite, support::declaration(), b"", &[(PluginTarget::X86_64LinuxGnu, artifact(PluginTarget::X86_64LinuxGnu))]);
    assert!(matches!(result, Err(CoreMaterialError::Io { step: MaterialIoStep::Write, source }) if source.kind() == io::ErrorKind::StorageFull));
}
