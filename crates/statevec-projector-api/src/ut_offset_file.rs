use super::*;
use std::{
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
use test_case::test_case;

fn offset(index: u64) -> ConsumeOffset {
    ConsumeOffset {
        cluster_id: "ab".repeat(32),
        projector_id: "trade-history".into(),
        raft_term: 9,
        raft_index: index,
    }
}
fn open(path: &Path) -> FileOffsetStore {
    FileOffsetStore::open(path, &offset(1).cluster_id, "trade-history").unwrap()
}

#[test]
fn initialize_is_explicit_and_open_preserves_positive_absence() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("offsets");
    let path = directory.join("history");
    assert!(matches!(
        FileOffsetStore::open(&path, &offset(1).cluster_id, "trade-history"),
        Err(OffsetStoreError::Io {
            operation: OffsetStoreOperation::OpenDirectory,
            ..
        })
    ));
    assert!(!directory.exists());
    FileOffsetStore::initialize(&directory).unwrap();
    FileOffsetStore::initialize(&directory).unwrap();
    let mut store = open(&path);
    assert_eq!(store.load().unwrap(), None);
    assert!(
        !path.exists(),
        "acquiring writer custody cannot create an offset"
    );
    assert!(matches!(
        FileOffsetStore::initialize(&path.with_extension("lock")),
        Err(OffsetStoreError::NotDirectory)
    ));
}

#[test]
fn persist_reopens_actual_bounded_envelope_and_duplicate_is_idempotent() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("history");
    let mut value = offset(u64::MAX);
    value.raft_term = u64::MAX;
    value.projector_id = "é".repeat(64);
    let mut store = FileOffsetStore::open(&path, &value.cluster_id, &value.projector_id).unwrap();
    store.persist_offset(&value).unwrap();
    let actual = fs::read(&path).unwrap();
    let json = value.encode_json().unwrap();
    assert_eq!(&actual[..8], b"SVPOFF01");
    assert_eq!(&actual[8..12], &(json.len() as u32).to_le_bytes());
    assert_eq!(&actual[12..12 + json.len()], &json);
    assert_eq!(
        &actual[12 + json.len()..],
        &Sha256::digest(&actual[..12 + json.len()])[..]
    );
    store.persist_offset(&value).unwrap();
    assert_eq!(fs::read(&path).unwrap(), actual);
    drop(store);
    let mut reopened =
        FileOffsetStore::open(&path, &value.cluster_id, &value.projector_id).unwrap();
    assert_eq!(reopened.load().unwrap(), Some(value));
}

#[test]
fn writer_lock_survives_offset_replacement_and_parent_aliases() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("history");
    let mut store = open(&path);
    store.persist_offset(&offset(1)).unwrap();
    store.persist_offset(&offset(2)).unwrap();
    let alias = root.path().join("alias");
    std::os::unix::fs::symlink(root.path(), &alias).unwrap();
    for other in [&path, &alias.join("history")] {
        assert!(matches!(
            FileOffsetStore::open(other, &offset(1).cluster_id, "trade-history"),
            Err(OffsetStoreError::WriterInUse)
        ));
    }
    drop(store);
    assert_eq!(open(&path).load().unwrap(), Some(offset(2)));
}

#[derive(Clone, Copy)]
enum Damage {
    Empty,
    Header,
    Length,
    Checksum,
    Oversized,
    Json,
}
#[test_case(Damage::Empty; "empty_is_corruption")]
#[test_case(Damage::Header; "unsupported_header")]
#[test_case(Damage::Length; "trailing_bytes")]
#[test_case(Damage::Checksum; "content_changed")]
#[test_case(Damage::Oversized; "length_bound")]
#[test_case(Damage::Json; "invalid_payload")]
fn corrupt_selection_refuses_without_replacing_its_bytes(damage: Damage) {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("history");
    let mut bytes = encode_file(&offset(1).encode_json().unwrap());
    let expected = match damage {
        Damage::Empty => {
            bytes.clear();
            OffsetFileFailure::Header
        }
        Damage::Header => {
            bytes[7] = b'2';
            OffsetFileFailure::Header
        }
        Damage::Length => {
            bytes.push(0);
            OffsetFileFailure::Length
        }
        Damage::Checksum => {
            bytes[12] ^= 1;
            OffsetFileFailure::Checksum
        }
        Damage::Oversized => {
            bytes.resize(MAX_FILE + 1, 0);
            OffsetFileFailure::TooLarge
        }
        Damage::Json => {
            bytes = encode_file(b"[]");
            OffsetFileFailure::Header
        }
    };
    fs::write(&path, &bytes).unwrap();
    let result = FileOffsetStore::open(&path, &offset(1).cluster_id, "trade-history");
    if matches!(damage, Damage::Json) {
        assert!(matches!(
            result,
            Err(OffsetStoreError::Offset(ConsumeOffsetError::Json { .. }))
        ));
    } else {
        assert!(matches!(result, Err(OffsetStoreError::Format(actual)) if actual == expected));
    }
    assert_eq!(fs::read(&path).unwrap(), bytes);
}

#[test]
fn namespace_and_monotonicity_refusals_leave_selection_reusable() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("history");
    let mut store = open(&path);
    store.persist_offset(&offset(5)).unwrap();
    let before = fs::read(&path).unwrap();
    assert!(matches!(
        store.persist_offset(&offset(4)),
        Err(OffsetStoreError::Regression {
            stored: 5,
            proposed: 4
        })
    ));
    let mut conflict = offset(5);
    conflict.raft_term += 1;
    assert!(matches!(
        store.persist_offset(&conflict),
        Err(OffsetStoreError::TermConflict { index: 5, .. })
    ));
    for (cluster, projector, cause) in [
        (
            "cd".repeat(32),
            "trade-history",
            ConsumeOffsetError::ClusterMismatch,
        ),
        (
            offset(1).cluster_id,
            "another",
            ConsumeOffsetError::ProjectorMismatch,
        ),
    ] {
        assert!(matches!(
            FileOffsetStore::open(&path, &cluster, projector),
            Err(OffsetStoreError::WriterInUse)
        ));
        let candidate = ConsumeOffset {
            cluster_id: cluster,
            projector_id: projector.into(),
            ..offset(6)
        };
        assert!(
            matches!(store.persist_offset(&candidate), Err(OffsetStoreError::Offset(actual)) if actual == cause)
        );
    }
    assert_eq!(fs::read(&path).unwrap(), before);
    store.persist_offset(&offset(6)).unwrap();
    drop(store);
    assert!(matches!(
        FileOffsetStore::open(&path, &offset(1).cluster_id, "another"),
        Err(OffsetStoreError::Offset(
            ConsumeOffsetError::ProjectorMismatch
        ))
    ));
    assert_eq!(open(&path).load().unwrap(), Some(offset(6)));
}

#[test]
fn directory_descriptor_keeps_replacement_and_sync_on_the_opened_store() {
    let root = tempfile::tempdir().unwrap();
    let original = root.path().join("original");
    FileOffsetStore::initialize(&original).unwrap();
    let mut store = open(&original.join("history"));
    let moved = root.path().join("moved");
    fs::rename(&original, &moved).unwrap();
    fs::create_dir(&original).unwrap();
    store.persist_offset(&offset(3)).unwrap();
    assert!(!original.join("history").exists());
    drop(store);
    assert_eq!(
        open(&moved.join("history")).load().unwrap(),
        Some(offset(3))
    );
}

#[test]
fn fifo_and_symlink_selection_refuse_before_reading_or_writing() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("history");
    let name = CString::new(path.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    assert!(matches!(
        FileOffsetStore::open(&path, &offset(1).cluster_id, "trade-history"),
        Err(OffsetStoreError::NotRegularFile)
    ));
    fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink("missing", &path).unwrap();
    assert!(matches!(
        FileOffsetStore::open(&path, &offset(1).cluster_id, "trade-history"),
        Err(OffsetStoreError::Io {
            operation: OffsetStoreOperation::OpenOffset,
            ..
        })
    ));
    assert!(!root.path().join("missing").exists());
}

struct Process(Option<Child>);
impl Drop for Process {
    fn drop(&mut self) {
        if let Some(child) = &mut self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
fn run_child(path: &Path, mode: &str) -> Process {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            std::thread::current().name().unwrap(),
            "--nocapture",
        ])
        .env("SV_OFFSET_FILE_CHILD", path)
        .env("SV_OFFSET_FILE_MODE", mode)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    Process(Some(command.spawn().unwrap()))
}
fn finish(mut process: Process) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while process.0.as_mut().unwrap().try_wait().unwrap().is_none() {
        assert!(
            Instant::now() < deadline,
            "offset-store child must terminate"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    let result = process.0.take().unwrap().wait_with_output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(
        String::from_utf8_lossy(&result.stdout).contains("1 passed"),
        "child selection must execute"
    );
}

#[test]
fn killed_writer_releases_lock_and_recovery_reads_only_disk() {
    if let Some(path) = std::env::var_os("SV_OFFSET_FILE_CHILD") {
        let path = PathBuf::from(path);
        let mut store = open(&path);
        store.persist_offset(&offset(7)).unwrap();
        fs::write(path.with_extension("ready"), b"saved").unwrap();
        loop {
            std::thread::park();
        }
    }
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("history");
    let mut child = run_child(&path, "hold");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !path.with_extension("ready").exists() {
        assert!(Instant::now() < deadline, "child must persist before kill");
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(matches!(
        FileOffsetStore::open(&path, &offset(1).cluster_id, "trade-history"),
        Err(OffsetStoreError::WriterInUse)
    ));
    child.0.as_mut().unwrap().kill().unwrap();
    assert!(!child.0.take().unwrap().wait().unwrap().success());
    assert_eq!(open(&path).load().unwrap(), Some(offset(7)));
}

#[cfg(target_os = "linux")]
#[test_case("replace"; "rename_failure_preserves_old_selection")]
#[test_case("directory"; "directory_sync_failure_can_select_new_offset")]
fn physical_save_failure_requires_reopen_without_claiming_rollback(mode: &str) {
    if let Some(path) = std::env::var_os("SV_OFFSET_FILE_CHILD") {
        let path = PathBuf::from(path);
        let mut store = open(&path);
        store.persist_offset(&offset(1)).unwrap();
        let operation = if mode == "replace" {
            OffsetStoreOperation::Replace
        } else {
            OffsetStoreOperation::SyncDirectory
        };
        refuse_syscall(mode, store.directory.as_raw_fd());
        assert!(
            matches!(store.persist_offset(&offset(2)), Err(OffsetStoreError::Io { operation: actual, source }) if actual == operation && source.raw_os_error() == Some(libc::EIO))
        );
        assert!(matches!(store.load(), Err(OffsetStoreError::NeedsReopen)));
        assert!(matches!(
            store.persist_offset(&offset(3)),
            Err(OffsetStoreError::NeedsReopen)
        ));
        drop(store);
        assert_eq!(
            open(&path).load().unwrap(),
            Some(offset(if mode == "replace" { 1 } else { 2 }))
        );
        return;
    }
    let root = tempfile::tempdir().unwrap();
    finish(run_child(&root.path().join("history"), mode));
}

#[cfg(target_os = "linux")]
fn refuse_syscall(mode: &str, directory: i32) {
    let i = |code: u32, jt, jf, k| libc::sock_filter {
        code: code as u16,
        jt,
        jf,
        k,
    };
    let arch = if cfg!(target_arch = "x86_64") {
        0xc000003e
    } else {
        0xc00000b7
    };
    let mut instructions = vec![
        i(libc::BPF_LD | libc::BPF_W | libc::BPF_ABS, 0, 0, 4),
        i(libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K, 1, 0, arch),
        i(
            libc::BPF_RET | libc::BPF_K,
            0,
            0,
            libc::SECCOMP_RET_KILL_PROCESS,
        ),
        i(libc::BPF_LD | libc::BPF_W | libc::BPF_ABS, 0, 0, 0),
    ];
    if mode == "replace" {
        instructions.push(i(
            libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K,
            0,
            1,
            libc::SYS_renameat as u32,
        ));
    } else {
        instructions.extend([
            i(
                libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K,
                0,
                3,
                libc::SYS_fsync as u32,
            ),
            i(libc::BPF_LD | libc::BPF_W | libc::BPF_ABS, 0, 0, 16),
            i(
                libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K,
                0,
                1,
                directory as u32,
            ),
        ]);
    }
    instructions.extend([
        i(
            libc::BPF_RET | libc::BPF_K,
            0,
            0,
            libc::SECCOMP_RET_ERRNO | libc::EIO as u32,
        ),
        i(libc::BPF_RET | libc::BPF_K, 0, 0, libc::SECCOMP_RET_ALLOW),
    ]);
    let program = libc::sock_fprog {
        len: instructions.len() as u16,
        filter: instructions.as_mut_ptr(),
    };
    // SAFETY: this test subprocess alone installs the kernel-copied filter.
    assert_eq!(
        unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) },
        0
    );
    assert_eq!(
        unsafe { libc::prctl(libc::PR_SET_SECCOMP, libc::SECCOMP_MODE_FILTER, &program) },
        0
    );
}
