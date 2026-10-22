use super::{CoreMaterialError, MAX_PACKAGE_BYTES, MaterialIoStep};
use std::fs::{File, OpenOptions};
use std::io::{self, Read};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

pub fn error(step: MaterialIoStep, source: io::Error) -> CoreMaterialError {
    CoreMaterialError::Io { step, source }
}

pub fn open_package(path: &Path) -> Result<File, CoreMaterialError> {
    // Pin the inode without opening a device/FIFO for I/O. Reject symlinks too.
    let pinned = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_PATH | libc::O_NOFOLLOW)
        .open(path)
        .map_err(|e| error(MaterialIoStep::Open, e))?;
    regular_length(&pinned)?;
    File::open(format!("/proc/self/fd/{}", pinned.as_raw_fd())).map_err(|e| error(MaterialIoStep::ReopenPinned, e))
}

pub fn regular_length(file: &File) -> Result<u64, CoreMaterialError> {
    let meta = file.metadata().map_err(|e| error(MaterialIoStep::Metadata, e))?;
    if !meta.is_file() {
        return Err(CoreMaterialError::NotRegular);
    }
    if meta.len() > MAX_PACKAGE_BYTES {
        return Err(CoreMaterialError::TooLarge { observed: meta.len(), limit: MAX_PACKAGE_BYTES });
    }
    Ok(meta.len())
}

pub fn read_exact(reader: &mut impl Read, bytes: &mut [u8]) -> Result<(), CoreMaterialError> {
    reader.read_exact(bytes).map_err(|source| match source.kind() {
        io::ErrorKind::UnexpectedEof => CoreMaterialError::Truncated,
        _ => error(MaterialIoStep::Read, source),
    })
}
