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
    let payload = format!(
        "cluster_id={}\nprojector_id={}\nraft_term=18446744073709551615\nraft_index=18446744073709551615\n",
        value.cluster_id, value.projector_id,
    );
    assert_eq!(&actual[..8], b"SVPOFF02");
    assert_eq!(&actual[8..12], &(payload.len() as u32).to_le_bytes());
    assert_eq!(&actual[12..12 + payload.len()], payload.as_bytes());
    assert_eq!(
        &actual[12 + payload.len()..],
        &Sha256::digest(&actual[..12 + payload.len()])[..]
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
}
#[test_case(Damage::Empty; "empty_is_corruption")]
#[test_case(Damage::Header; "unsupported_header")]
#[test_case(Damage::Length; "trailing_bytes")]
#[test_case(Damage::Checksum; "content_changed")]
#[test_case(Damage::Oversized; "length_bound")]
fn corrupt_selection_refuses_without_replacing_its_bytes(damage: Damage) {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("history");
    let mut bytes = encode_file(&offset(1).encode_properties().unwrap());
    let expected = match damage {
        Damage::Empty => {
            bytes.clear();
            OffsetFileError::Header
        }
        Damage::Header => {
            bytes[7] = b'9';
            OffsetFileError::Header
        }
        Damage::Length => {
            bytes.push(0);
            OffsetFileError::Length
        }
        Damage::Checksum => {
            bytes[12] ^= 1;
            OffsetFileError::Checksum
        }
        Damage::Oversized => {
            bytes.resize(MAX_FILE + 1, 0);
            OffsetFileError::TooLarge
        }
    };
    fs::write(&path, &bytes).unwrap();
    let result = FileOffsetStore::open(&path, &offset(1).cluster_id, "trade-history");
    assert!(matches!(result, Err(OffsetStoreError::Format(actual)) if actual == expected));
    assert_eq!(fs::read(&path).unwrap(), bytes);
}

#[test]
fn valid_checksum_does_not_admit_malformed_properties() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("history");
    let payload = format!(
        "cluster_id={}\nprojector_id=trade-history\nraft_term=9\nraft_index=1\nraft_index=2\n",
        offset(1).cluster_id,
    );
    let bytes = encode_file(payload.as_bytes());
    fs::write(&path, &bytes).unwrap();

    assert!(matches!(
        FileOffsetStore::open(&path, &offset(1).cluster_id, "trade-history"),
        Err(OffsetStoreError::Offset(ConsumeOffsetError::DuplicateProperty {
            field: crate::OffsetField::RaftIndex,
        }))
    ));
    assert_eq!(fs::read(&path).unwrap(), bytes);
}

#[test]
fn version_one_json_selection_is_refused_without_rewrite_or_fresh_fallback() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("history");
    let json = format!(
        r#"{{"cluster_id":"{}","projector_id":"trade-history","raft_term":9,"raft_index":1}}"#,
        offset(1).cluster_id,
    );
    // Construct the actual old format independently of the new encoder.
    let mut bytes = b"SVPOFF01".to_vec();
    bytes.extend_from_slice(&(json.len() as u32).to_le_bytes());
    bytes.extend_from_slice(json.as_bytes());
    bytes.extend_from_slice(&Sha256::digest(&bytes));
    fs::write(&path, &bytes).unwrap();

    for _ in 0..2 {
        assert!(matches!(
            FileOffsetStore::open(&path, &offset(1).cluster_id, "trade-history"),
            Err(OffsetStoreError::Format(OffsetFileError::Header))
        ));
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
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
fn run_child(path: &Path) -> Process {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            std::thread::current().name().unwrap(),
            "--nocapture",
        ])
        .env("SV_OFFSET_FILE_CHILD", path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    Process(Some(command.spawn().unwrap()))
}
fn wait_output(mut process: Process) -> std::process::Output {
    let deadline = Instant::now() + Duration::from_secs(5);
    while process.0.as_mut().unwrap().try_wait().unwrap().is_none() {
        assert!(
            Instant::now() < deadline,
            "offset-store child must terminate"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    process.0.take().unwrap().wait_with_output().unwrap()
}
fn finish(process: Process) {
    let result = wait_output(process);
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
    let mut child = run_child(&path);
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
#[derive(Clone, Copy)]
enum IoFault {
    WriteTemporary,
    SyncFile,
    Replace,
    SyncDirectory,
    TerminateBeforeReplace,
}

#[cfg(target_os = "linux")]
#[test_case(IoFault::WriteTemporary; "temporary_write_failure_preserves_old_selection")]
#[test_case(IoFault::SyncFile; "temporary_sync_failure_preserves_old_selection")]
#[test_case(IoFault::Replace; "rename_failure_preserves_old_selection")]
#[test_case(IoFault::SyncDirectory; "directory_sync_failure_can_select_new_offset")]
fn physical_save_failure_requires_reopen_without_claiming_rollback(fault: IoFault) {
    let selected = if matches!(fault, IoFault::SyncDirectory) {
        2
    } else {
        1
    };
    if let Some(path) = std::env::var_os("SV_OFFSET_FILE_CHILD") {
        let path = PathBuf::from(path);
        let mut store = open(&path);
        store.persist_offset(&offset(1)).unwrap();
        let operation = match fault {
            IoFault::WriteTemporary => OffsetStoreOperation::WriteTemporary,
            IoFault::SyncFile => OffsetStoreOperation::SyncFile,
            IoFault::Replace => OffsetStoreOperation::Replace,
            IoFault::SyncDirectory => OffsetStoreOperation::SyncDirectory,
            IoFault::TerminateBeforeReplace => unreachable!("separate crash case"),
        };
        refuse_syscall(fault, store.directory.as_raw_fd());
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
            read_offset(&File::open(&path).unwrap()).unwrap(),
            offset(selected)
        );
        assert!(
            temporary_paths(path.parent().unwrap()).is_empty(),
            "failed save must unlink its own temporary"
        );
        if matches!(fault, IoFault::SyncFile | IoFault::SyncDirectory) {
            assert!(
                matches!(
                    FileOffsetStore::open(&path, &offset(1).cluster_id, "trade-history"),
                    Err(OffsetStoreError::Io { operation: actual, .. }) if actual == operation
                ),
                "a visible selection cannot authorize restore while sync still fails"
            );
        } else {
            assert_eq!(open(&path).load().unwrap(), Some(offset(1)));
        }
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("history");
    finish(run_child(&path));
    let mut reopened = open(&path);
    assert_eq!(reopened.load().unwrap(), Some(offset(selected)));
    reopened.persist_offset(&offset(3)).unwrap();
    assert_eq!(reopened.load().unwrap(), Some(offset(3)));
}

#[cfg(target_os = "linux")]
#[test_case(IoFault::SyncFile; "selected_file_sync_failure")]
#[test_case(IoFault::SyncDirectory; "selected_directory_sync_failure")]
fn reopen_requires_durability_before_exposing_restore(fault: IoFault) {
    if let Some(path) = std::env::var_os("SV_OFFSET_FILE_CHILD") {
        let path = PathBuf::from(path);
        let mut store = open(&path);
        store.persist_offset(&offset(2)).unwrap();
        let bytes = fs::read(&path).unwrap();
        let directory = store.directory.as_raw_fd();
        drop(store);

        // This isolated child opens no other files between close and reopen:
        // the directory reacquires the lowest free fd used by the first open.
        // Denying every fsync in the file case also checks file-before-dir order.
        refuse_syscall(fault, directory);
        let expected = if matches!(fault, IoFault::SyncFile) {
            OffsetStoreOperation::SyncFile
        } else {
            OffsetStoreOperation::SyncDirectory
        };
        match FileOffsetStore::open(&path, &offset(1).cluster_id, "trade-history") {
            Err(OffsetStoreError::Io { operation, source }) => {
                assert_eq!(operation, expected);
                assert_eq!(source.raw_os_error(), Some(libc::EIO));
            }
            Err(error) => panic!("restore must preserve the sync failure: {error:?}"),
            Ok(_) => panic!("reopen exposed restore without confirming durability"),
        }
        assert_eq!(fs::read(&path).unwrap(), bytes);
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("history");
    finish(run_child(&path));
    // A new process without the physical fault can establish the receipt and
    // continue. Failed construction did not keep the writer lock or erase P.
    let mut store = open(&path);
    assert_eq!(store.load().unwrap(), Some(offset(2)));
    store.persist_offset(&offset(3)).unwrap();
    assert_eq!(store.load().unwrap(), Some(offset(3)));
}

#[cfg(target_os = "linux")]
fn temporary_paths(directory: &Path) -> Vec<PathBuf> {
    fs::read_dir(directory)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name().unwrap().to_string_lossy().starts_with(".sv-offset-")
        })
        .collect()
}

#[cfg(target_os = "linux")]
#[test]
fn termination_before_rename_never_selects_or_deletes_an_orphan_temporary() {
    use std::os::unix::process::ExitStatusExt;
    if let Some(path) = std::env::var_os("SV_OFFSET_FILE_CHILD") {
        let path = PathBuf::from(path);
        let mut store = open(&path);
        store.persist_offset(&offset(1)).unwrap();
        // Kill only this child at the real rename syscall, after temp write and
        // fsync. No Rust Drop runs, as with SIGKILL before selection.
        let limit = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        assert_eq!(unsafe { libc::setrlimit(libc::RLIMIT_CORE, &limit) }, 0);
        refuse_syscall(
            IoFault::TerminateBeforeReplace,
            store.directory.as_raw_fd(),
        );
        let _ = store.persist_offset(&offset(2));
        panic!("the real replace syscall must terminate this child");
    }
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("history");
    let output = wait_output(run_child(&path));
    assert_eq!(output.status.signal(), Some(libc::SIGSYS));
    let temporary = temporary_paths(root.path());
    assert_eq!(
        temporary.len(), 1,
        "the process reached temporary creation, not selection"
    );
    assert_eq!(
        read_offset(&File::open(&temporary[0]).unwrap()).unwrap(),
        offset(2)
    );
    let bytes = fs::read(&temporary[0]).unwrap();

    let mut reopened = open(&path);
    assert_eq!(reopened.load().unwrap(), Some(offset(1)));
    reopened.persist_offset(&offset(3)).unwrap();
    assert_eq!(reopened.load().unwrap(), Some(offset(3)));
    assert_eq!(temporary_paths(root.path()), temporary);
    assert_eq!(
        fs::read(&temporary[0]).unwrap(), bytes,
        "no directory-wide orphan cleanup"
    );
}

#[cfg(target_os = "linux")]
fn refuse_syscall(fault: IoFault, directory: i32) {
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
    if matches!(fault, IoFault::WriteTemporary) {
        instructions.extend([
            i(
                libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K,
                0,
                4,
                libc::SYS_write as u32,
            ),
            i(libc::BPF_LD | libc::BPF_W | libc::BPF_ABS, 0, 0, 16),
            i(libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K, 2, 0, 1),
            i(libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K, 1, 0, 2),
        ]); // Preserve stdout/stderr so a missing write still has diagnostics.
    } else if matches!(fault, IoFault::SyncFile) {
        instructions.push(i(
            libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K,
            0,
            1,
            libc::SYS_fsync as u32,
        ));
    } else if matches!(fault, IoFault::Replace | IoFault::TerminateBeforeReplace) {
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
            if matches!(fault, IoFault::TerminateBeforeReplace) {
                libc::SECCOMP_RET_KILL_PROCESS
            } else {
                libc::SECCOMP_RET_ERRNO | libc::EIO as u32
            },
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
