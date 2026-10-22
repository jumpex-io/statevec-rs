use super::*;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireManifest {
    format: u32,
    core_abi_major: u32,
    core_version: String,
    schema_identity: WireSchema,
    execution_revision: String,
    has_tx_invariants: bool,
    configuration_digest: String,
    configuration_length: String,
    artifacts: Vec<WireArtifact>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireSchema {
    version: String,
    types: String,
    records: String,
    commands: String,
    events: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireArtifact {
    target: String,
    digest: String,
    length: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireDeclaration {
    schema_identity: WireSchema,
    execution_revision: String,
    has_tx_invariants: bool,
}

pub fn decode_declaration(bytes: &[u8]) -> Result<CorePackageDeclaration, CoreMaterialError> {
    if bytes.len() > MAX_MANIFEST_BYTES {
        return Err(CoreMaterialError::TooLarge { observed: bytes.len() as u64, limit: MAX_MANIFEST_BYTES as u64 });
    }
    let wire: WireDeclaration =
        serde_json::from_slice(bytes).map_err(|source| CoreMaterialError::ManifestJson { source })?;
    if wire.execution_revision.is_empty()
        || wire.execution_revision.contains('\0')
        || wire.execution_revision.len() > MAX_CORE_REVISION_BYTES
    {
        return Err(invalid(ManifestField::Revision));
    }
    Ok(CorePackageDeclaration {
        schema_identity: SchemaIdentity {
            schema_version: statevec_model::Version::from_bytes(unhex(&wire.schema_identity.version)?),
            types_schema_fingerprint: unhex(&wire.schema_identity.types)?,
            record_schema_fingerprint: unhex(&wire.schema_identity.records)?,
            command_schema_fingerprint: unhex(&wire.schema_identity.commands)?,
            event_schema_fingerprint: unhex(&wire.schema_identity.events)?,
        },
        execution_revision: wire.execution_revision,
        has_tx_invariants: wire.has_tx_invariants,
    })
}

fn invalid(field: ManifestField) -> CoreMaterialError {
    CoreMaterialError::Manifest { field }
}

fn decimal(value: &str) -> Result<u64, CoreMaterialError> {
    if value.is_empty()
        || value.len() > 20
        || (value.len() > 1 && value.starts_with('0'))
        || !value.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(invalid(ManifestField::Decimal));
    }
    value.parse().map_err(|_| invalid(ManifestField::Decimal))
}

pub(super) fn validate(manifest: &CorePackageManifest) -> Result<(), CoreMaterialError> {
    if manifest.declaration.execution_revision.is_empty()
        || manifest.declaration.execution_revision.contains('\0')
        || manifest.declaration.execution_revision.len() > MAX_CORE_REVISION_BYTES
    {
        return Err(invalid(ManifestField::Revision));
    }
    if manifest.configuration_length > MAX_CORE_CONFIG_BYTES as u64 {
        return Err(invalid(ManifestField::Configuration));
    }
    let valid_targets = match manifest.artifacts.as_slice() {
        [one] => one.length > 0,
        [first, second] => {
            first.target == PluginTarget::X86_64LinuxGnu && second.target == PluginTarget::Aarch64LinuxGnu
        }
        _ => false,
    };
    if !valid_targets
        || manifest
            .artifacts
            .iter()
            .any(|a| a.length == 0 || a.length > MAX_PLUGIN_ARTIFACT_BYTES)
    {
        return Err(invalid(ManifestField::Artifacts));
    }
    Ok(())
}

pub fn decode_manifest(bytes: &[u8]) -> Result<CorePackageManifest, CoreMaterialError> {
    if bytes.len() > MAX_MANIFEST_BYTES {
        return Err(CoreMaterialError::TooLarge { observed: bytes.len() as u64, limit: MAX_MANIFEST_BYTES as u64 });
    }
    let wire: WireManifest =
        serde_json::from_slice(bytes).map_err(|source| CoreMaterialError::ManifestJson { source })?;
    if wire.format != 1 {
        return Err(invalid(ManifestField::Format));
    }
    if wire.core_abi_major != statevec_api::v1::ffi::CORE_ABI_MAJOR {
        return Err(invalid(ManifestField::Abi));
    }
    let manifest = CorePackageManifest {
        declaration: CorePackageDeclaration {
            schema_identity: SchemaIdentity {
                schema_version: statevec_model::Version::from_bytes(unhex(&wire.schema_identity.version)?),
                types_schema_fingerprint: unhex(&wire.schema_identity.types)?,
                record_schema_fingerprint: unhex(&wire.schema_identity.records)?,
                command_schema_fingerprint: unhex(&wire.schema_identity.commands)?,
                event_schema_fingerprint: unhex(&wire.schema_identity.events)?,
            },
            execution_revision: wire.execution_revision,
            has_tx_invariants: wire.has_tx_invariants,
        },
        configuration_digest: Digest32::new(unhex(&wire.configuration_digest)?),
        configuration_length: decimal(&wire.configuration_length)?,
        artifacts: wire
            .artifacts
            .into_iter()
            .map(|a| {
                Ok(CoreArtifact {
                    target: parse_target(&a.target)?,
                    digest: PluginArtifactDigest(unhex(&a.digest)?),
                    length: decimal(&a.length)?,
                })
            })
            .collect::<Result<_, CoreMaterialError>>()?,
    };
    validate(&manifest)?;
    if manifest.declaration.core_version().digest().bytes() != unhex::<32>(&wire.core_version)? {
        return Err(invalid(ManifestField::CoreVersion));
    }
    Ok(manifest)
}

pub fn encode_manifest(manifest: &CorePackageManifest) -> Result<Vec<u8>, CoreMaterialError> {
    validate(manifest)?;
    let schema = manifest.declaration.schema_identity;
    let wire = WireManifest {
        format: 1,
        core_abi_major: statevec_api::v1::ffi::CORE_ABI_MAJOR,
        core_version: hex(&manifest.declaration.core_version().digest().bytes()),
        schema_identity: WireSchema {
            version: hex(&schema.schema_version.to_bytes()),
            types: hex(&schema.types_schema_fingerprint),
            records: hex(&schema.record_schema_fingerprint),
            commands: hex(&schema.command_schema_fingerprint),
            events: hex(&schema.event_schema_fingerprint),
        },
        execution_revision: manifest.declaration.execution_revision.clone(),
        has_tx_invariants: manifest.declaration.has_tx_invariants,
        configuration_digest: hex(&manifest.configuration_digest.bytes()),
        configuration_length: manifest.configuration_length.to_string(),
        artifacts: manifest
            .artifacts
            .iter()
            .map(|a| WireArtifact {
                target: a.target.triple().into(),
                digest: hex(&a.digest.0),
                length: a.length.to_string(),
            })
            .collect(),
    };
    let bytes = serde_json::to_vec(&wire).map_err(|source| CoreMaterialError::ManifestJson { source })?;
    if bytes.len() > MAX_MANIFEST_BYTES {
        return Err(CoreMaterialError::TooLarge { observed: bytes.len() as u64, limit: MAX_MANIFEST_BYTES as u64 });
    }
    Ok(bytes)
}
