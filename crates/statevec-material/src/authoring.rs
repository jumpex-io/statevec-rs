//! Shared `pack` command. No runtime, store, loader, or node dependency.
use crate::*;
use serde_json::{Value, json};
use std::ffi::OsString;
use std::fs::OpenOptions;
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

pub const USAGE: &str = "statevec-material pack DECLARATION CONFIG OUTPUT TARGET ARTIFACT [TARGET ARTIFACT]";

#[derive(Debug)]
pub enum Error {
    Input,
    Material(CoreMaterialError),
}
impl From<CoreMaterialError> for Error {
    fn from(value: CoreMaterialError) -> Self { Self::Material(value) }
}

fn small_file(path: &Path, limit: usize) -> Result<Vec<u8>, CoreMaterialError> {
    let mut bytes = Vec::new();
    package_io::open_package(path)?.take(limit as u64 + 1).read_to_end(&mut bytes)
        .map_err(|source| CoreMaterialError::Io { step: MaterialIoStep::Read, source })?;
    if bytes.len() > limit {
        return Err(CoreMaterialError::TooLarge { observed: bytes.len() as u64, limit: limit as u64 });
    }
    Ok(bytes)
}

/// Create-only output. On I/O/refusal the caller owns any partial output; never
/// retry with overwrite, publish into a store, or load a plugin implicitly.
pub fn execute(args: &[OsString]) -> Result<Value, Error> {
    if args.len() == 1 && matches!(args[0].to_str(), Some("--help" | "-h")) {
        return Ok(json!({"version":1,"category":"help","usage":USAGE}));
    }
    if !matches!(args.len(), 6 | 8) || args[0] != "pack" { return Err(Error::Input); }
    let targets = args[4..].chunks_exact(2)
        .map(|pair| parse_target(pair[0].to_str().ok_or(Error::Input)?).map_err(|_| Error::Input))
        .collect::<Result<Vec<_>, _>>()?;
    let declaration = decode_declaration(&small_file(Path::new(&args[1]), MAX_MANIFEST_BYTES)?)?;
    let configuration = small_file(Path::new(&args[2]), MAX_CORE_CONFIG_BYTES)?;
    let artifacts = args[4..].chunks_exact(2).zip(targets)
        .map(|(pair, target)| Ok((target, package_io::open_package(Path::new(&pair[1]))?)))
        .collect::<Result<Vec<_>, Error>>()?;
    let mut file = OpenOptions::new().write(true).create_new(true).custom_flags(libc::O_NOFOLLOW)
        .open(Path::new(&args[3]))
        .map_err(|source| CoreMaterialError::Io { step: MaterialIoStep::Open, source })?;
    let id = pack(&mut file, declaration, &configuration, &artifacts)?;
    file.sync_all().map_err(|source| CoreMaterialError::Io { step: MaterialIoStep::SyncFile, source })?;
    Ok(json!({"version":1,"category":"packed","core_version":hex(&id.core_version.digest().bytes()),
        "package_digest":hex(&id.package_digest.bytes()),"execution":"not_checked"}))
}

/// Typed material classification shared with the server's extended tool.
pub fn material_failure(error: CoreMaterialError) -> (u8, &'static str, Value) {
    match error {
        CoreMaterialError::Io { step, source } => (3, "io", json!({
            "stage":format!("{step:?}"),"os_code":source.raw_os_error(),"io_kind":format!("{:?}",source.kind())})),
        CoreMaterialError::Artifact(PluginMaterialFailure::Io { step, source }) => (3, "io", json!({
            "stage":format!("{step:?}"),"os_code":source.raw_os_error(),"io_kind":format!("{:?}",source.kind())})),
        error => {
            let code = if matches!(error, CoreMaterialError::NamespaceLimit) { 3 } else { 4 };
            let detail = match error {
                CoreMaterialError::ManifestJson { source } => json!({"reason":"manifest_json",
                    "line":source.line().to_string(),"column":source.column().to_string()}),
                CoreMaterialError::Manifest { field } => json!({"reason":"manifest","field":format!("{field:?}")}),
                CoreMaterialError::NotRegular => json!({"reason":"not_regular"}),
                CoreMaterialError::TooLarge { observed, limit } => json!({"reason":"too_large",
                    "observed":observed.to_string(),"limit":limit.to_string()}),
                CoreMaterialError::Truncated => json!({"reason":"truncated"}),
                CoreMaterialError::Header { offset } => json!({"reason":"header","offset":offset.to_string()}),
                CoreMaterialError::Padding { offset } => json!({"reason":"padding","offset":offset.to_string()}),
                CoreMaterialError::TrailingBytes => json!({"reason":"trailing_bytes"}),
                CoreMaterialError::Digest { .. } => json!({"reason":"digest"}),
                CoreMaterialError::Artifact(_) => json!({"reason":"artifact"}),
                CoreMaterialError::MissingTarget { .. } => json!({"reason":"missing_target"}),
                CoreMaterialError::MissingPackage { .. } => json!({"reason":"missing_package"}),
                CoreMaterialError::MissingSelection { .. } => json!({"reason":"missing_selection"}),
                CoreMaterialError::InvalidSelection => json!({"reason":"invalid_selection"}),
                CoreMaterialError::SelectionChanged => json!({"reason":"selection_changed"}),
                CoreMaterialError::NamespaceLimit => json!({"reason":"namespace_limit"}),
                CoreMaterialError::PageLimit => json!({"reason":"page_limit"}),
                CoreMaterialError::Io { .. } => unreachable!("handled above"),
            };
            (code, "material_refused", detail)
        }
    }
}

pub fn failure(error: Error) -> (u8, Value) {
    let (code, category, detail) = match error {
        Error::Input => (2, "invalid_input", json!({"usage":USAGE})),
        Error::Material(error) => material_failure(error),
    };
    (code, json!({"version":1,"category":category,"detail":detail}))
}
