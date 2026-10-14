//! Explicit whole-entry offset persistence. Directory-relative operations keep
//! the lock, selected file and directory sync bound to one opened directory.
use crate::{ConsumeOffset, ConsumeOffsetError, MAX_OFFSET_JSON_BYTES};
use sha2::{Digest, Sha256};
use std::{
    ffi::CString,
    fs::{self, File, TryLockError},
    io::{self, Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::ffi::OsStrExt,
    },
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};

const MAGIC: &[u8; 8] = b"SVPOFF01";
const HEADER: usize = 12;
const HASH: usize = 32;
const MAX_FILE: usize = HEADER + MAX_OFFSET_JSON_BYTES + HASH;
static TEMPORARY: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum OffsetStoreOperation {
    Initialize,
    OpenDirectory,
    OpenLock,
    Lock,
    OpenOffset,
    ReadOffset,
    CreateTemporary,
    WriteTemporary,
    SyncFile,
    Replace,
    SyncDirectory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum OffsetFileFailure {
    Header,
    Length,
    Checksum,
    TooLarge,
}

#[derive(Debug)]
#[non_exhaustive]
pub enum OffsetStoreError {
    InvalidPath,
    NotDirectory,
    NotRegularFile,
    WriterInUse,
    NeedsReopen,
    TemporaryNamesExhausted,
    Offset(ConsumeOffsetError),
    Format(OffsetFileFailure),
    Regression {
        stored: u64,
        proposed: u64,
    },
    TermConflict {
        index: u64,
        stored: u64,
        proposed: u64,
    },
    Io {
        operation: OffsetStoreOperation,
        source: io::Error,
    },
}
impl std::fmt::Display for OffsetStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "offset store: {self:?}")
    }
}
impl std::error::Error for OffsetStoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Offset(source) => Some(source),
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}
fn io_failure(operation: OffsetStoreOperation, source: io::Error) -> OffsetStoreError {
    OffsetStoreError::Io { operation, source }
}

enum State {
    Open,
    NeedsReopen,
}

/// An exclusive writer for one offset path. The stable `<filename>.lock`
/// sidecar is never unlinked by this helper, including on Drop. Cooperating
/// writers must use this helper and leave that lock inode/directory intact.
/// Closing the store releases the OS lock; no persistence occurs from Drop.
pub struct FileOffsetStore {
    directory: File,
    _writer: File,
    name: CString,
    cluster_id: String,
    projector_id: String,
    state: State,
}
impl FileOffsetStore {
    /// Explicitly create one directory under an existing parent, then sync
    /// both. Existing directories are preserved; ancestors are not created.
    pub fn initialize(directory: &Path) -> Result<(), OffsetStoreError> {
        fs::create_dir(directory)
            .or_else(|e| {
                if e.kind() == io::ErrorKind::AlreadyExists {
                    Ok(())
                } else {
                    Err(e)
                }
            })
            .map_err(|e| io_failure(OffsetStoreOperation::Initialize, e))?;
        let dir = File::open(directory)
            .map_err(|e| io_failure(OffsetStoreOperation::OpenDirectory, e))?;
        if !dir
            .metadata()
            .map_err(|e| io_failure(OffsetStoreOperation::OpenDirectory, e))?
            .is_dir()
        {
            return Err(OffsetStoreError::NotDirectory);
        }
        dir.sync_all()
            .map_err(|e| io_failure(OffsetStoreOperation::SyncDirectory, e))?;
        let parent = directory
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        File::open(parent)
            .and_then(|f| f.sync_all())
            .map_err(|e| io_failure(OffsetStoreOperation::SyncDirectory, e))
    }

    /// Open an existing directory and acquire nonblocking writer custody.
    /// Does not create directories or replace/create the selected offset.
    /// Positive absence is returned by load; invalid existing files refuse.
    /// Before exposing an existing offset for restore, validate and sync the
    /// selected file, then its directory. Visibility after an uncertain save
    /// is not enough; either sync failure refuses construction.
    pub fn open(
        path: &Path,
        cluster_id: &str,
        projector_id: &str,
    ) -> Result<Self, OffsetStoreError> {
        ConsumeOffset {
            cluster_id: cluster_id.into(),
            projector_id: projector_id.into(),
            raft_term: 1,
            raft_index: 1,
        }
        .validate()
        .map_err(OffsetStoreError::Offset)?;
        let name = path.file_name().ok_or(OffsetStoreError::InvalidPath)?;
        let name = CString::new(name.as_bytes()).map_err(|_| OffsetStoreError::InvalidPath)?;
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let directory =
            File::open(parent).map_err(|e| io_failure(OffsetStoreOperation::OpenDirectory, e))?;
        if !directory
            .metadata()
            .map_err(|e| io_failure(OffsetStoreOperation::OpenDirectory, e))?
            .is_dir()
        {
            return Err(OffsetStoreError::NotDirectory);
        }
        let mut lock_name = name.as_bytes().to_vec();
        lock_name.extend_from_slice(b".lock");
        let lock_name = CString::new(lock_name).map_err(|_| OffsetStoreError::InvalidPath)?;
        let writer = open_at(&directory, &lock_name, libc::O_RDWR | libc::O_CREAT)
            .map_err(|e| io_failure(OffsetStoreOperation::OpenLock, e))?;
        if !writer
            .metadata()
            .map_err(|e| io_failure(OffsetStoreOperation::OpenLock, e))?
            .is_file()
        {
            return Err(OffsetStoreError::NotRegularFile);
        }
        match writer.try_lock() {
            Ok(()) => (),
            Err(TryLockError::WouldBlock) => return Err(OffsetStoreError::WriterInUse),
            Err(TryLockError::Error(source)) => {
                return Err(io_failure(OffsetStoreOperation::Lock, source));
            }
        }
        let store = Self {
            directory,
            _writer: writer,
            name,
            cluster_id: cluster_id.into(),
            projector_id: projector_id.into(),
            state: State::Open,
        };
        if let Some((file, _)) = store.read_selected()? {
            file.sync_all()
                .map_err(|e| io_failure(OffsetStoreOperation::SyncFile, e))?;
            store.directory
                .sync_all()
                .map_err(|e| io_failure(OffsetStoreOperation::SyncDirectory, e))?;
        }
        Ok(store)
    }

    /// Read the actual selected file each time, never a cached cursor.
    pub fn load(&mut self) -> Result<Option<ConsumeOffset>, OffsetStoreError> {
        self.check_open()?;
        Ok(self.read_selected()?.map(|(_, offset)| offset))
    }

    fn read_selected(&self) -> Result<Option<(File, ConsumeOffset)>, OffsetStoreError> {
        let file = match open_at(&self.directory, &self.name, libc::O_RDONLY) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(io_failure(OffsetStoreOperation::OpenOffset, e)),
        };
        let offset = read_offset(&file)?;
        offset
            .check_source(&self.cluster_id, &self.projector_id)
            .map_err(OffsetStoreError::Offset)?;
        Ok(Some((file, offset)))
    }

    /// Persist after business effects are durable. This does not notify the
    /// host. Success includes file and directory sync; every write-path error
    /// makes this object unusable until reopened to inspect the real selection.
    pub fn persist_offset(&mut self, offset: &ConsumeOffset) -> Result<(), OffsetStoreError> {
        self.check_open()?;
        offset
            .check_source(&self.cluster_id, &self.projector_id)
            .map_err(OffsetStoreError::Offset)?;
        let payload = offset.encode_json().map_err(OffsetStoreError::Offset)?;
        let previous = self.load()?;
        if let Some(old) = &previous {
            if offset.raft_index < old.raft_index {
                return Err(OffsetStoreError::Regression {
                    stored: old.raft_index,
                    proposed: offset.raft_index,
                });
            }
            if offset.raft_index == old.raft_index && offset.raft_term != old.raft_term {
                return Err(OffsetStoreError::TermConflict {
                    index: old.raft_index,
                    stored: old.raft_term,
                    proposed: offset.raft_term,
                });
            }
        }
        // Arm before any fallible write. Unwind also cannot falsely leave an
        // object reusable after uncertain selection/durability.
        self.state = State::NeedsReopen;
        if previous.as_ref() == Some(offset) {
            // Exact duplicate saves refresh the receipt without replacing the
            // selected file. Open also establishes it before exposing restore.
            let file = open_at(&self.directory, &self.name, libc::O_RDONLY)
                .map_err(|e| io_failure(OffsetStoreOperation::OpenOffset, e))?;
            file.sync_all()
                .map_err(|e| io_failure(OffsetStoreOperation::SyncFile, e))?;
        } else {
            let mut temporary = Temporary::create(&self.directory)?;
            let bytes = encode_file(&payload);
            temporary
                .file
                .write_all(&bytes)
                .map_err(|e| io_failure(OffsetStoreOperation::WriteTemporary, e))?;
            temporary
                .file
                .sync_all()
                .map_err(|e| io_failure(OffsetStoreOperation::SyncFile, e))?;
            // SAFETY: both names are validated C strings relative to the same
            // live directory descriptor; rename consumes only this temp name.
            if unsafe {
                libc::renameat(
                    self.directory.as_raw_fd(),
                    temporary.name.as_ptr(),
                    self.directory.as_raw_fd(),
                    self.name.as_ptr(),
                )
            } != 0
            {
                return Err(io_failure(
                    OffsetStoreOperation::Replace,
                    io::Error::last_os_error(),
                ));
            }
            temporary.selected = true;
        }
        self.directory
            .sync_all()
            .map_err(|e| io_failure(OffsetStoreOperation::SyncDirectory, e))?;
        self.state = State::Open;
        Ok(())
    }
    fn check_open(&self) -> Result<(), OffsetStoreError> {
        match self.state {
            State::Open => Ok(()),
            State::NeedsReopen => Err(OffsetStoreError::NeedsReopen),
        }
    }
}

fn open_at(directory: &File, name: &CString, flags: i32) -> Result<File, io::Error> {
    // SAFETY: borrowed valid directory descriptor, NUL-terminated name and
    // explicit mode for optional creation. The returned fd has one File owner.
    let fd = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            flags | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
            0o600,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}
fn read_offset(file: &File) -> Result<ConsumeOffset, OffsetStoreError> {
    if !file
        .metadata()
        .map_err(|e| io_failure(OffsetStoreOperation::ReadOffset, e))?
        .is_file()
    {
        return Err(OffsetStoreError::NotRegularFile);
    }
    let mut bytes = Vec::with_capacity(MAX_FILE + 1);
    file.take((MAX_FILE + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| io_failure(OffsetStoreOperation::ReadOffset, e))?;
    if bytes.len() > MAX_FILE {
        return Err(OffsetStoreError::Format(OffsetFileFailure::TooLarge));
    }
    if bytes.len() < HEADER + HASH || &bytes[..8] != MAGIC {
        return Err(OffsetStoreError::Format(OffsetFileFailure::Header));
    }
    let length = u32::from_le_bytes(bytes[8..12].try_into().expect("checked header")) as usize;
    if length > MAX_OFFSET_JSON_BYTES || bytes.len() != HEADER + length + HASH {
        return Err(OffsetStoreError::Format(OffsetFileFailure::Length));
    }
    let digest = Sha256::digest(&bytes[..HEADER + length]);
    if digest.as_slice() != &bytes[HEADER + length..] {
        return Err(OffsetStoreError::Format(OffsetFileFailure::Checksum));
    }
    ConsumeOffset::decode_json(&bytes[HEADER..HEADER + length]).map_err(OffsetStoreError::Offset)
}
fn encode_file(payload: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(HEADER + payload.len() + HASH);
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    bytes.extend_from_slice(payload);
    let digest = Sha256::digest(&bytes);
    bytes.extend_from_slice(&digest);
    bytes
}
struct Temporary<'a> {
    directory: &'a File,
    file: File,
    name: CString,
    selected: bool,
}
impl<'a> Temporary<'a> {
    fn create(directory: &'a File) -> Result<Self, OffsetStoreError> {
        for _ in 0..32 {
            let next = TEMPORARY
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
                .map_err(|_| OffsetStoreError::TemporaryNamesExhausted)?;
            let name = CString::new(format!(".sv-offset-{}-{next}", std::process::id()))
                .expect("numeric temp name");
            match open_at(
                directory,
                &name,
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
            ) {
                Ok(file) => {
                    return Ok(Self {
                        directory,
                        file,
                        name,
                        selected: false,
                    });
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => (),
                Err(e) => return Err(io_failure(OffsetStoreOperation::CreateTemporary, e)),
            }
        }
        Err(OffsetStoreError::TemporaryNamesExhausted)
    }
}
impl Drop for Temporary<'_> {
    fn drop(&mut self) {
        if !self.selected {
            // SAFETY: clean up only our exclusively created temporary name.
            unsafe {
                libc::unlinkat(self.directory.as_raw_fd(), self.name.as_ptr(), 0);
            }
        }
    }
}

#[cfg(test)]
#[path = "ut_offset_file.rs"]
mod tests;
