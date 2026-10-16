use super::author::store;
use std::path::PathBuf;
use std::{
    path::Path,
    process::{Child, Command},
    time::{Duration, Instant},
};
use store::{Change, ConsumeOffsetFields, Entry, Event, HistoryStore};
use test_case::test_case;

fn entry(index: u64, before: Option<&[u8]>, after: &[u8]) -> Entry {
    Entry {
        offset: ConsumeOffsetFields {
            cluster_id: "ab".repeat(32),
            projector_id: "history".into(),
            raft_term: 9,
            raft_index: index,
        },
        events: vec![Event {
            tx_seq: index,
            tx_ordinal: 0,
            event_ordinal: 0,
            event_seq: 1,
            ref_tx_time_ns: 100,
            kind: 1,
            payload: after.to_vec(),
        }],
        changes: vec![Change {
            kind: 1,
            sys_id: 7,
            revision: index,
            before: before.map(Vec::from),
            after: Some(after.to_vec()),
        }],
    }
}
fn open(path: &Path, file: bool) -> HistoryStore {
    HistoryStore::open(
        path,
        file.then(|| path.with_extension("offset")).as_deref(),
        &"ab".repeat(32),
        "history",
    )
    .unwrap()
}
fn events(store: &HistoryStore) -> Vec<Vec<u8>> {
    store
        .database
        .prepare("SELECT payload FROM events ORDER BY raft_index,tx_ordinal,event_ordinal")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}
fn record(store: &HistoryStore) -> Vec<u8> {
    store
        .database
        .query_row("SELECT value FROM records WHERE kind=1", [], |r| r.get(0))
        .unwrap()
}

#[test_case(false; "atomic_db_offset")]
#[test_case(true; "default_file_after_effects")]
fn restored_consumer_replays_idempotently_and_applies_the_next_update(file: bool) {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("history.sqlite");
    let first = entry(1, None, b"a");
    let second = entry(2, Some(b"a"), b"b");
    let mut store = open(&path, file);
    assert!(store.load().unwrap().is_none());
    store.commit_entry(&first).unwrap();
    store.commit_entry(&second).unwrap();
    drop(store);
    let mut restored = open(&path, file);
    assert_eq!(restored.load().unwrap(), Some(second.offset()));
    restored.commit_entry(&first).unwrap();
    assert_eq!(
        restored.load().unwrap(), Some(second.offset()),
        "replaying below P cannot temporarily regress the saved offset"
    );
    assert_eq!(record(&restored), b"b");
    restored.commit_entry(&second).unwrap();
    assert_eq!(events(&restored), [b"a".to_vec(), b"b".to_vec()]);
    restored.commit_entry(&entry(3, Some(b"b"), b"c")).unwrap();
    assert_eq!(record(&restored), b"c");
    assert_eq!(
        events(&restored),
        [b"a".to_vec(), b"b".to_vec(), b"c".to_vec()]
    );
    assert_eq!(restored.load().unwrap().unwrap().raft_index, 3);
}

#[test]
fn reopening_an_older_business_backup_refuses_the_newer_file_offset() {
    use statevec_projector_api::plugin_v2::{ProjectorFailure, ProjectorIoStage};
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("history.sqlite");
    let backup = root.path().join("stopped-backup.sqlite");
    let first = entry(1, None, b"a");
    let second = entry(2, Some(b"a"), b"b");
    let mut store = open(&path, true);
    store.commit_entry(&first).unwrap();
    drop(store); // Close SQLite's WAL before copying a stopped business backup.
    std::fs::copy(&path, &backup).unwrap();
    let mut store = open(&path, true);
    store.commit_entry(&second).unwrap();
    drop(store);
    let offset_bytes = std::fs::read(path.with_extension("offset")).unwrap();

    std::fs::copy(&backup, &path).unwrap();
    let error = match HistoryStore::open(
        &path,
        Some(&path.with_extension("offset")),
        &"ab".repeat(32),
        "history",
    ) {
        Err(error) => error,
        Ok(_) => panic!("offset P must not authorize recovery from a business backup below P"),
    };
    assert!(matches!(error, store::Error::MissingEffects));
    assert!(matches!(
        error.failure(ProjectorIoStage::StoreOpen),
        ProjectorFailure::Application { code: 1006 }
    ));
    assert_eq!(
        std::fs::read(path.with_extension("offset")).unwrap(), offset_bytes
    );
    let database = rusqlite::Connection::open_with_flags(
        &path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    ).unwrap();
    let model: Vec<u8> = database
        .query_row("SELECT value FROM records WHERE kind=1", [], |r| r.get(0))
        .unwrap();
    assert_eq!(model, b"a", "refusal must not repair or reset the business backup");
}

#[test]
fn failed_model_update_rolls_back_effects_and_offset_together() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("history.sqlite");
    let mut store = open(&path, false);
    store.commit_entry(&entry(1, None, b"a")).unwrap();
    let wrong = entry(2, Some(b"wrong"), b"b");
    assert!(matches!(
        store.commit_entry(&wrong),
        Err(store::Error::ModelMismatch)
    ));
    assert_eq!(events(&store), [b"a".to_vec()]);
    assert_eq!(record(&store), b"a");
    assert_eq!(store.load().unwrap().unwrap().raft_index, 1);
    store.commit_entry(&entry(2, Some(b"a"), b"b")).unwrap();
}

#[test_case(false; "db_transaction_requires_effects")]
#[test_case(true; "file_save_requires_effects")]
fn consumer_cannot_persist_a_position_before_its_business_effects(file: bool) {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("history.sqlite");
    let mut store = open(&path, file);
    let offset = entry(1, None, b"a").offset();
    if file {
        assert!(matches!(
            store.persist_model_offset(&offset),
            Err(store::Error::MissingEffects)
        ));
    } else {
        let tx = store.database.transaction().unwrap();
        assert!(matches!(
            HistoryStore::stage_offset(&tx, &offset),
            Err(store::Error::MissingEffects)
        ));
    }
    assert_eq!(store.load().unwrap(), None);
    assert!(events(&store).is_empty());
}

#[test]
fn database_preserves_full_u64_positions_and_refuses_other_namespaces() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("history.sqlite");
    let mut value = entry(u64::MAX, None, b"max");
    value.offset.raft_term = u64::MAX;
    let mut store = open(&path, false);
    store.commit_entry(&value).unwrap();
    drop(store);
    let mut reopened = open(&path, false);
    assert_eq!(reopened.load().unwrap(), Some(value.offset()));
    assert_eq!(reopened.model_index().unwrap(), u64::MAX);
    drop(reopened);
    assert!(matches!(
        HistoryStore::open(&path, None, &"ab".repeat(32), "other"),
        Err(store::Error::Namespace)
    ));
    assert_eq!(open(&path, false).load().unwrap(), Some(value.offset()));
}

#[derive(Clone, Copy)]
enum Cut {
    BeforeCommit,
    AfterCommit,
    BeforeFileSave,
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

#[test_case(Cut::BeforeCommit; "kill_with_uncommitted_effects_and_offset")]
#[test_case(Cut::AfterCommit; "kill_after_atomic_commit_before_host_report")]
#[test_case(Cut::BeforeFileSave; "kill_after_effects_before_file_offset")]
fn process_crash_recovers_the_actual_database_and_next_business_result(cut: Cut) {
    let file = matches!(cut, Cut::BeforeFileSave);
    if let Some(path) = std::env::var_os("SV_HISTORY_CRASH_CHILD") {
        let path = PathBuf::from(path);
        let mut store = open(&path, file);
        store.commit_entry(&entry(1, None, b"a")).unwrap();
        let second = entry(2, Some(b"a"), b"b");
        let tx = store
            .database
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .unwrap();
        HistoryStore::stage_effects(&tx, &second).unwrap();
        if !file {
            HistoryStore::stage_offset(&tx, &second.offset()).unwrap();
        }
        if !matches!(cut, Cut::BeforeCommit) {
            tx.commit().unwrap();
        }
        std::fs::write(path.with_extension("ready"), b"at actual transaction cut").unwrap();
        loop {
            std::thread::park();
        }
    }
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("history.sqlite");
    let child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            std::thread::current().name().unwrap(),
            "--nocapture",
        ])
        .env("SV_HISTORY_CRASH_CHILD", &path)
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let mut child = Process(Some(child));
    let deadline = Instant::now() + Duration::from_secs(5);
    while !path.with_extension("ready").exists() {
        assert!(
            Instant::now() < deadline,
            "child must reach the real DB cut"
        );
        assert!(child.0.as_mut().unwrap().try_wait().unwrap().is_none());
        std::thread::sleep(Duration::from_millis(1));
    }
    child.0.as_mut().unwrap().kill().unwrap();
    assert!(!child.0.take().unwrap().wait().unwrap().success());
    let mut restored = open(&path, file);
    let expected = if matches!(cut, Cut::AfterCommit) {
        2
    } else {
        1
    };
    assert_eq!(restored.load().unwrap().unwrap().raft_index, expected);
    let effects = if matches!(cut, Cut::BeforeCommit) {
        vec![b"a".to_vec()]
    } else {
        vec![b"a".to_vec(), b"b".to_vec()]
    };
    assert_eq!(events(&restored), effects);
    restored.commit_entry(&entry(2, Some(b"a"), b"b")).unwrap();
    restored.commit_entry(&entry(3, Some(b"b"), b"c")).unwrap();
    assert_eq!(
        events(&restored),
        [b"a".to_vec(), b"b".to_vec(), b"c".to_vec()]
    );
    assert_eq!(record(&restored), b"c");
}
