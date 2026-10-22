use crate::*;
use statevec_model::Digest32;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::Write;
use std::os::unix::fs::FileExt;
use crate::package_io as port;

/// Write a new package, never load an artifact. The caller owns partial output on failure.
/// Input handles remain pinned; mutation between hash and copy is detected.
pub fn pack(
    output: &mut impl Write,
    declaration: CorePackageDeclaration,
    configuration: &[u8],
    artifacts: &[(PluginTarget, File)],
) -> Result<CoreMaterialId, CoreMaterialError> {
    if !(1..=2).contains(&artifacts.len()) {
        return Err(CoreMaterialError::Manifest { field: ManifestField::Artifacts });
    }
    if configuration.len() > MAX_CORE_CONFIG_BYTES {
        return Err(CoreMaterialError::Manifest { field: ManifestField::Configuration });
    }
    let mut manifest = CorePackageManifest {
        declaration,
        configuration_digest: Digest32::new(Sha256::digest(configuration).into()),
        configuration_length: configuration.len() as u64,
        artifacts: artifacts
            .iter()
            .map(|(target, _)| CoreArtifact { target: *target, length: 1, digest: PluginArtifactDigest([0; 32]) })
            .collect(),
    };
    manifest::validate(&manifest)?;
    for ((target, file), entry) in artifacts.iter().zip(&mut manifest.artifacts) {
        let length = port::regular_length(file)?;
        if length == 0 || length > MAX_PLUGIN_ARTIFACT_BYTES {
            return Err(CoreMaterialError::Manifest { field: ManifestField::Artifacts });
        }
        elf::inspect(file, length, *target).map_err(CoreMaterialError::Artifact)?;
        entry.length = length;
        entry.digest = artifact_io::digest(file, length).map_err(CoreMaterialError::Artifact)?;
    }
    let bytes = encode_manifest(&manifest)?;
    let mut writer = PackageWriter { output, hash: Sha256::new(), length: 0 };
    writer.member("manifest.json", &bytes)?;
    writer.member("configuration.bin", configuration)?;
    let mut buffer = [0; TRANSFER_BYTES];
    for ((_, file), entry) in artifacts.iter().zip(&manifest.artifacts) {
        writer.write(&tar::header(target_name(entry.target), entry.length)?)?;
        let mut position = 0;
        let mut hash = Sha256::new();
        while position < entry.length {
            let count = (entry.length - position).min(buffer.len() as u64) as usize;
            file.read_exact_at(&mut buffer[..count], position)
                .map_err(|e| port::error(MaterialIoStep::Read, e))?;
            hash.update(&buffer[..count]);
            writer.write(&buffer[..count])?;
            position += count as u64;
        }
        let expected = Digest32::new(entry.digest.0);
        let actual = Digest32::new(hash.finalize().into());
        if expected != actual { return Err(CoreMaterialError::Digest { expected, actual }); }
        writer.write(&[0; tar::BLOCK][..tar::padding(entry.length)])?;
    }
    writer.write(&[0; 2 * tar::BLOCK])?;
    Ok(CoreMaterialId {
        core_version: manifest.declaration.core_version(),
        package_digest: Digest32::new(writer.hash.finalize().into()),
    })
}

struct PackageWriter<'a, W> {
    output: &'a mut W,
    hash: Sha256,
    length: u64,
}
impl<W: Write> PackageWriter<'_, W> {
    fn write(&mut self, bytes: &[u8]) -> Result<(), CoreMaterialError> {
        let next = self.length + bytes.len() as u64;
        if next > MAX_PACKAGE_BYTES {
            return Err(CoreMaterialError::TooLarge { observed: next, limit: MAX_PACKAGE_BYTES });
        }
        self.output
            .write_all(bytes)
            .map_err(|e| port::error(MaterialIoStep::Write, e))?;
        self.hash.update(bytes);
        self.length = next;
        Ok(())
    }
    fn member(&mut self, name: &str, bytes: &[u8]) -> Result<(), CoreMaterialError> {
        self.write(&tar::header(name, bytes.len() as u64)?)?;
        self.write(bytes)?;
        self.write(&[0; tar::BLOCK][..tar::padding(bytes.len() as u64)])
    }
}
