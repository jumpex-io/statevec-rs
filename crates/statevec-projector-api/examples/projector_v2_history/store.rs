//! Application-owned schema: DB mode commits effects and the four offset fields
//! together. File mode commits idempotent DB effects before persisting the file.
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use statevec_projector_api::{
    ConsumeOffset, ConsumeOffsetError, FileOffsetStore, OffsetStoreError, plugin_v2::*,
};
use std::{
    fs::{File, TryLockError},
    os::unix::fs::OpenOptionsExt,
    path::Path,
};

#[derive(Debug)]
pub enum Error {
    Database(rusqlite::Error),
    File(OffsetStoreError),
    Metadata(ConsumeOffsetError),
    Io(std::io::Error),
    WriterInUse,
    Namespace,
    Format,
    MissingEffects,
    Conflict,
    ModelMismatch,
}
impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        Self::Database(e)
    }
}
impl From<OffsetStoreError> for Error {
    fn from(e: OffsetStoreError) -> Self {
        Self::File(e)
    }
}
impl From<ConsumeOffsetError> for Error {
    fn from(e: ConsumeOffsetError) -> Self {
        Self::Metadata(e)
    }
}
impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}
impl Error {
    pub fn failure(self, stage: ProjectorIoStage) -> ProjectorFailure {
        match self {
            Self::Io(e) | Self::File(OffsetStoreError::Io { source: e, .. }) => {
                ProjectorFailure::Io {
                    stage,
                    kind: ProjectorIoKind::from_io(e.kind()),
                    os_code: e.raw_os_error(),
                }
            }
            Self::Database(rusqlite::Error::SqliteFailure(e, _)) => ProjectorFailure::Application {
                code: u32::try_from(e.extended_code)
                    .ok()
                    .and_then(|n| 0x10000u32.checked_add(n))
                    .unwrap_or(1000),
            },
            Self::Database(_) => ProjectorFailure::Application { code: 1000 },
            Self::File(_) => ProjectorFailure::Application { code: 1001 },
            Self::Metadata(source) => ProjectorFailure::Application {
                code: match source {
                    ConsumeOffsetError::ClusterMismatch | ConsumeOffsetError::ProjectorMismatch => {
                        1004
                    }
                    _ => 1002,
                },
            },
            Self::WriterInUse => ProjectorFailure::Application { code: 1003 },
            Self::Namespace => ProjectorFailure::Application { code: 1004 },
            Self::Format => ProjectorFailure::Application { code: 1005 },
            Self::MissingEffects => ProjectorFailure::Application { code: 1006 },
            Self::Conflict => ProjectorFailure::Application { code: 1007 },
            Self::ModelMismatch => ProjectorFailure::Application { code: 1008 },
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct Entry {
    pub offset: ConsumeOffsetFields,
    pub events: Vec<Event>,
    pub changes: Vec<Change>,
}
#[derive(serde::Serialize, serde::Deserialize)]
pub struct ConsumeOffsetFields {
    pub cluster_id: String,
    pub projector_id: String,
    pub raft_term: u64,
    pub raft_index: u64,
}
#[derive(serde::Serialize, serde::Deserialize)]
pub struct Event {
    pub tx_seq: u64,
    pub tx_ordinal: u32,
    pub event_ordinal: u64,
    pub event_seq: u32,
    pub ref_tx_time_ns: u64,
    pub kind: u16,
    pub payload: Vec<u8>,
}
#[derive(serde::Serialize, serde::Deserialize)]
pub struct Change {
    pub kind: u16,
    pub sys_id: u64,
    pub revision: u64,
    pub before: Option<Vec<u8>>,
    pub after: Option<Vec<u8>>,
}
impl Entry {
    pub fn offset(&self) -> ConsumeOffset {
        let p = &self.offset;
        ConsumeOffset {
            cluster_id: p.cluster_id.clone(),
            projector_id: p.projector_id.clone(),
            raft_term: p.raft_term,
            raft_index: p.raft_index,
        }
    }
    pub fn read(frame: FrameView<'_>, offset: ConsumeOffset) -> Result<Self, ProjectorFailure> {
        let mut events = Vec::new();
        let mut changes = Vec::new();
        for ordinal in 0..frame.transaction_count() {
            let tx = frame.transaction(ordinal)?;
            for event_ordinal in 0..tx.event_count() {
                let event = tx.event(event_ordinal)?;
                events.push(Event {
                    tx_seq: tx.result().tx_seq,
                    tx_ordinal: tx.result().ordinal,
                    event_ordinal: event_ordinal as u64,
                    event_seq: event.event_seq,
                    ref_tx_time_ns: tx.ref_tx_time_ns(),
                    kind: event.event_kind,
                    payload: event.payload.to_vec(),
                });
            }
            for index in 0..tx.change_count() {
                let (key, revision, before, after) = match tx.change(index)? {
                    RecordChangeView::Insert {
                        key,
                        revision,
                        after,
                    } => (key, revision, None, Some(after.to_vec())),
                    RecordChangeView::Update {
                        key,
                        revision,
                        before,
                        after,
                    } => (key, revision, Some(before.to_vec()), Some(after.to_vec())),
                    RecordChangeView::Delete {
                        key,
                        revision,
                        before,
                    } => (key, revision, Some(before.to_vec()), None),
                };
                changes.push(Change {
                    kind: key.kind,
                    sys_id: key.sys_id,
                    revision,
                    before,
                    after,
                });
            }
        }
        Ok(Self {
            offset: ConsumeOffsetFields {
                cluster_id: offset.cluster_id,
                projector_id: offset.projector_id,
                raft_term: offset.raft_term,
                raft_index: offset.raft_index,
            },
            events,
            changes,
        })
    }
}

pub struct HistoryStore {
    pub database: Connection,
    _writer: File,
    cluster_id: String,
    projector_id: String,
    file: Option<FileOffsetStore>,
}
impl HistoryStore {
    pub fn open(
        path: &Path,
        offset_path: Option<&Path>,
        cluster_id: &str,
        projector_id: &str,
    ) -> Result<Self, Error> {
        ConsumeOffset {
            cluster_id: cluster_id.into(),
            projector_id: projector_id.into(),
            raft_term: 1,
            raft_index: 1,
        }
        .validate()?;
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        FileOffsetStore::initialize(parent)?;
        let mut lock = path.as_os_str().to_owned();
        lock.push(".writer.lock");
        let writer = File::options()
            .read(true)
            .write(true)
            .create(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(Path::new(&lock))?;
        if !writer.metadata()?.is_file() {
            return Err(Error::Format);
        }
        match writer.try_lock() {
            Ok(()) => (),
            Err(TryLockError::WouldBlock) => return Err(Error::WriterInUse),
            Err(TryLockError::Error(e)) => return Err(Error::Io(e)),
        }
        let file = if let Some(path) = offset_path {
            FileOffsetStore::initialize(
                path.parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .unwrap_or(Path::new(".")),
            )?;
            Some(FileOffsetStore::open(path, cluster_id, projector_id)?)
        } else {
            None
        };
        let mut database = Connection::open(path)?;
        let tables: i64 = database.query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='namespace'",
            [],
            |r| r.get(0),
        )?;
        if tables != 0 {
            let namespace: (String, String, u32) = database.query_row(
                "SELECT cluster_id,projector_id,version FROM namespace WHERE singleton=1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )?;
            if namespace.0 != cluster_id || namespace.1 != projector_id {
                return Err(Error::Namespace);
            }
            if namespace.2 != 1 {
                return Err(Error::Format);
            }
        } else {
            let other: i64 = database.query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table'",
                [],
                |r| r.get(0),
            )?;
            if other != 0 {
                return Err(Error::Format);
            }
        }
        database.pragma_update(None, "journal_mode", "WAL")?;
        database.pragma_update(None, "synchronous", "FULL")?;
        let tx = database.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute_batch("CREATE TABLE IF NOT EXISTS namespace(singleton INTEGER PRIMARY KEY CHECK(singleton=1),cluster_id TEXT NOT NULL,projector_id TEXT NOT NULL,version INTEGER NOT NULL);
            CREATE TABLE IF NOT EXISTS consumer(cluster_id TEXT NOT NULL,projector_id TEXT NOT NULL,raft_term BLOB NOT NULL,raft_index BLOB NOT NULL,PRIMARY KEY(cluster_id,projector_id));
            CREATE TABLE IF NOT EXISTS entries(raft_index BLOB PRIMARY KEY,raft_term BLOB NOT NULL,body BLOB NOT NULL);
            CREATE TABLE IF NOT EXISTS events(raft_index BLOB NOT NULL,tx_ordinal INTEGER NOT NULL,event_ordinal BLOB NOT NULL,tx_seq BLOB NOT NULL,event_seq INTEGER NOT NULL,ref_tx_time_ns BLOB NOT NULL,kind INTEGER NOT NULL,payload BLOB NOT NULL,PRIMARY KEY(raft_index,tx_ordinal,event_ordinal));
            CREATE TABLE IF NOT EXISTS records(kind INTEGER NOT NULL,sys_id BLOB NOT NULL,revision BLOB NOT NULL,value BLOB NOT NULL,PRIMARY KEY(kind,sys_id));")?;
        tx.execute(
            "INSERT OR IGNORE INTO namespace VALUES(1,?1,?2,1)",
            params![cluster_id, projector_id],
        )?;
        tx.commit()?;
        File::open(parent)?.sync_all()?;
        let mut store = Self {
            database,
            _writer: writer,
            cluster_id: cluster_id.into(),
            projector_id: projector_id.into(),
            file,
        };
        if let Some(offset) = store.load()? {
            store.require_effects(&offset)?;
        }
        Ok(store)
    }
    pub fn load(&mut self) -> Result<Option<ConsumeOffset>, Error> {
        if let Some(file) = &mut self.file {
            return Ok(file.load()?);
        }
        let raw: Option<(String, String, Vec<u8>, Vec<u8>)> = self
            .database
            .query_row(
                "SELECT cluster_id,projector_id,raft_term,raft_index FROM consumer",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        raw.map(|(cluster_id, projector_id, term, index)| {
            let offset = ConsumeOffset {
                cluster_id,
                projector_id,
                raft_term: coordinate(term)?,
                raft_index: coordinate(index)?,
            };
            offset.check_source(&self.cluster_id, &self.projector_id)?;
            Ok(offset)
        })
        .transpose()
    }
    pub fn model_index(&self) -> Result<u64, Error> {
        let raw: Option<Vec<u8>> = self
            .database
            .query_row(
                "SELECT raft_index FROM entries ORDER BY raft_index DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .optional()?;
        raw.map(coordinate).transpose().map(|p| p.unwrap_or(0))
    }
    pub fn commit_entry(&mut self, entry: &Entry) -> Result<(), Error> {
        let offset = entry.offset();
        offset.check_source(&self.cluster_id, &self.projector_id)?;
        let tx = self
            .database
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        Self::stage_effects(&tx, entry)?;
        if self.file.is_none() {
            Self::stage_offset(&tx, &offset)?;
        }
        tx.commit()?;
        // A separate file is deliberately AFTER the actual effects commit.
        if self.file.is_some() {
            self.persist_model_offset(&offset)?;
        }
        Ok(())
    }
    pub fn stage_effects(tx: &Transaction<'_>, entry: &Entry) -> Result<(), Error> {
        let index = entry.offset.raft_index.to_be_bytes();
        let term = entry.offset.raft_term.to_be_bytes();
        let body = serde_json::to_vec(entry).map_err(|_| Error::Format)?;
        let existing: Option<(Vec<u8>, Vec<u8>)> = tx
            .query_row(
                "SELECT raft_term,body FROM entries WHERE raft_index=?1",
                [&index[..]],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((old_term, old_body)) = existing {
            if old_term != term || old_body != body {
                return Err(Error::Conflict);
            }
            return Ok(()); // exact replay after commit but before offset/ACK
        }
        for event in &entry.events {
            tx.execute(
                "INSERT INTO events VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
                params![
                    &index[..],
                    event.tx_ordinal,
                    &event.event_ordinal.to_be_bytes()[..],
                    &event.tx_seq.to_be_bytes()[..],
                    event.event_seq,
                    &event.ref_tx_time_ns.to_be_bytes()[..],
                    event.kind,
                    &event.payload
                ],
            )?;
        }
        for change in &entry.changes {
            let key = change.sys_id.to_be_bytes();
            let current: Option<Vec<u8>> = tx
                .query_row(
                    "SELECT value FROM records WHERE kind=?1 AND sys_id=?2",
                    params![change.kind, &key[..]],
                    |r| r.get(0),
                )
                .optional()?;
            if current != change.before {
                return Err(Error::ModelMismatch);
            }
            match &change.after {
                Some(value) => {
                    tx.execute("INSERT INTO records VALUES(?1,?2,?3,?4) ON CONFLICT(kind,sys_id) DO UPDATE SET revision=excluded.revision,value=excluded.value", params![change.kind,&key[..],&change.revision.to_be_bytes()[..],value])?;
                }
                None => {
                    tx.execute(
                        "DELETE FROM records WHERE kind=?1 AND sys_id=?2",
                        params![change.kind, &key[..]],
                    )?;
                }
            }
        }
        tx.execute(
            "INSERT INTO entries VALUES(?1,?2,?3)",
            params![&index[..], &term[..], body],
        )?;
        Ok(())
    }
    pub fn stage_offset(tx: &Transaction<'_>, offset: &ConsumeOffset) -> Result<(), Error> {
        let term: Option<Vec<u8>> = tx
            .query_row(
                "SELECT raft_term FROM entries WHERE raft_index=?1",
                [&offset.raft_index.to_be_bytes()[..]],
                |r| r.get(0),
            )
            .optional()?;
        if term.as_deref() != Some(&offset.raft_term.to_be_bytes()[..]) {
            return Err(Error::MissingEffects);
        }
        let old: Option<(Vec<u8>, Vec<u8>)> = tx
            .query_row(
                "SELECT raft_term,raft_index FROM consumer WHERE cluster_id=?1 AND projector_id=?2",
                params![offset.cluster_id, offset.projector_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((old_term, old_index)) = old {
            let previous = coordinate(old_index)?;
            if offset.raft_index < previous {
                return Ok(());
            } // replay through durable P
            if previous == offset.raft_index && coordinate(old_term)? != offset.raft_term {
                return Err(Error::Conflict);
            }
        }
        tx.execute("INSERT INTO consumer VALUES(?1,?2,?3,?4) ON CONFLICT(cluster_id,projector_id) DO UPDATE SET raft_term=excluded.raft_term,raft_index=excluded.raft_index", params![offset.cluster_id,offset.projector_id,&offset.raft_term.to_be_bytes()[..],&offset.raft_index.to_be_bytes()[..]])?;
        Ok(())
    }
    fn require_effects(&self, offset: &ConsumeOffset) -> Result<(), Error> {
        let term: Option<Vec<u8>> = self
            .database
            .query_row(
                "SELECT raft_term FROM entries WHERE raft_index=?1",
                [&offset.raft_index.to_be_bytes()[..]],
                |r| r.get(0),
            )
            .optional()?;
        if term.as_deref() != Some(&offset.raft_term.to_be_bytes()[..]) {
            return Err(Error::MissingEffects);
        }
        Ok(())
    }
    pub fn persist_model_offset(&mut self, offset: &ConsumeOffset) -> Result<(), Error> {
        self.require_effects(offset)?;
        if let Some(file) = &mut self.file {
            if file
                .load()?
                .is_none_or(|p| p.raft_index <= offset.raft_index)
            {
                file.persist_offset(offset)?;
            }
        }
        Ok(())
    }
}
fn coordinate(bytes: Vec<u8>) -> Result<u64, Error> {
    Ok(u64::from_be_bytes(
        bytes.try_into().map_err(|_| Error::Format)?,
    ))
}
