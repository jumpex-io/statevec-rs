//! Positioned reads of one already opened artifact. No sealing or loading occurs here.
use crate::{PluginArtifactDigest, PluginMaterialFailure, PluginMaterialIoStep};
use std::fs::File;
use std::io;
use std::os::unix::fs::FileExt;
use sha2::{Digest, Sha256};
pub const COPY_BYTES: usize = crate::TRANSFER_BYTES;

pub fn read_exact(
    file: &File,
    bytes: &mut [u8],
    offset: u64,
    step: PluginMaterialIoStep,
) -> Result<(), PluginMaterialFailure> {
    file.read_exact_at(bytes, offset).map_err(|source| failure(step, source))
}

pub fn digest(file: &File, byte_len: u64) -> Result<PluginArtifactDigest, PluginMaterialFailure> {
    let mut hash = Sha256::new();
    let mut buffer = [0; COPY_BYTES];
    let mut offset = 0;
    while offset < byte_len {
        let count = (byte_len - offset).min(COPY_BYTES as u64) as usize;
        read_exact(file, &mut buffer[..count], offset, PluginMaterialIoStep::HashSealed)?;
        hash.update(&buffer[..count]);
        offset += count as u64;
    }
    Ok(PluginArtifactDigest(hash.finalize().into()))
}


fn failure(step: PluginMaterialIoStep, source: io::Error) -> PluginMaterialFailure {
    PluginMaterialFailure::Io { step, source }
}
