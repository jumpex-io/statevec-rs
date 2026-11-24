//! SDK-exported additive qualification consumer. Reuses the history example's
//! atomic effects/offset store and latest example's private rebuild model.
//! The JSON lines are test observations, never recovery authority.
#[allow(dead_code, unused_imports)]
#[path = "../../../../crates/statevec/examples/core_bank_additive/schema.rs"]
mod bank;
#[path = "../../examples/projector_latest/model.rs"]
mod model;
#[path = "../../examples/projector_history/store.rs"]
mod store;
use statevec::GeneratedRecordAccess;
use statevec_projector_api::ConsumeOffset;
use statevec_projector_api::plugin::{StartRequest, *};
use std::io::Write;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub struct Bank<const TARGET: bool, const ADDITIVE: bool>;
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config<const TARGET: bool> {
    database_path: PathBuf,
    report_path: PathBuf,
    #[serde(default)]
    report_order: ReportOrder,
    /// While this file exists preparation waits and the consumer takes no
    /// frame, as a stalled store or downstream would; a running consumer
    /// still stops when asked.
    #[serde(default)]
    pause_path: Option<PathBuf>,
}
#[derive(Clone, Copy, Default, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ReportOrder {
    #[default]
    ReleaseBeforeReport,
    // Preserve the original bring-up schedule: hold through acceptance and
    // yield between polls. Stage observations diagnose it, not authorize it.
    HoldUntilAccepted,
}
impl ReportOrder {
    fn pause(self) {
        match self {
            Self::ReleaseBeforeReport => std::thread::park_timeout(Duration::from_millis(1)),
            Self::HoldUntilAccepted => std::thread::yield_now(),
        }
    }
}
pub struct Prepared<const TARGET: bool> {
    store: Option<store::HistoryStore>,
    restored: Option<ConsumeOffset>,
    report: std::fs::File,
    report_order: ReportOrder,
    pause_path: Option<PathBuf>,
}
impl<const T: bool, const A: bool> ProjectorModule for Bank<T, A> {
    type Configured = Config<T>;
    const SUPPORTED_HISTORY_MODES: SupportedHistoryModes = SupportedHistoryModes::Both;
    const SCHEMA_COMPATIBILITY: SchemaCompatibility =
        if A { SchemaCompatibility::Additive } else { SchemaCompatibility::Exact };
    fn decoder_idl() -> &'static [u8] {
        if T { bank::decoder_idl() } else { bank::source::decoder_idl() }
    }
    fn configure(&self, input: ProjectorConfig<'_>) -> Result<Config<T>, ConfigureError> {
        let mut config: Config<T> =
            serde_json::from_slice(input.bytes).map_err(|_| ConfigureError::Invalid { code: 1 })?;
        for path in [&mut config.database_path, &mut config.report_path].into_iter().chain(config.pause_path.as_mut()) {
            if path.as_os_str().is_empty() {
                return Err(ConfigureError::Invalid { code: 2 });
            }
            if path.is_relative() {
                *path = input.base_dir.join(&*path);
            }
        }
        Ok(config)
    }
}
fn io_error(source: std::io::Error) -> ProjectorError {
    ProjectorError::Io {
        stage: ProjectorIoStage::StoreWrite,
        kind: ProjectorIoKind::from_io(source.kind()),
        os_code: source.raw_os_error(),
    }
}
fn failure() -> ProjectorError {
    ProjectorError::Application { code: 960 }
}
fn write(report: &mut std::fs::File, row: serde_json::Value) -> Result<(), ProjectorError> {
    writeln!(report, "{row}").and_then(|()| report.flush()).map_err(io_error)
}
fn stage(report: &mut std::fs::File, index: u64, stage: &str) -> Result<(), ProjectorError> {
    write(report, serde_json::json!({"type":"progress", "index":index, "stage":stage}))
}
impl<const T: bool> ConfiguredProjector for Config<T> {
    type Prepared = Prepared<T>;
    fn prepare(&self, context: ProjectorPrepare<'_>) -> Result<Prepared<T>, ProjectorError> {
        while self.pause_path.as_ref().is_some_and(|path| path.exists()) {
            std::thread::park_timeout(Duration::from_millis(10));
        }
        let mut store = if context.history_mode == HistoryMode::LatestState {
            None
        } else {
            Some(
                store::HistoryStore::open(&self.database_path, None, context.cluster_id, context.projector_id)
                    .map_err(|e| e.failure(ProjectorIoStage::StoreOpen))?,
            )
        };
        let restored = store
            .as_mut()
            .map(|s| s.load())
            .transpose()
            .map_err(|e| e.failure(ProjectorIoStage::StoreRead))?
            .flatten();
        let report = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.report_path)
            .map_err(io_error)?;
        Ok(Prepared { store, restored, report, report_order: self.report_order, pause_path: self.pause_path.clone() })
    }
}
fn decoded<const T: bool>(kind: u16, bytes: &[u8]) -> serde_json::Value {
    match kind {
        1 => {
            let row = bank::source::Account::wrap(bytes);
            serde_json::json!({"account": row.account_id(), "credit": row.credit(), "debit": row.debit(),
                "tag": if T { Some(bank::Account::wrap(bytes).tag()) } else { None }})
        }
        2 if T => serde_json::json!({"receipt": bank::Receipt::wrap(bytes).tag()}),
        _ => serde_json::json!({"unknown_kind": kind}),
    }
}
fn observe_frame<const T: bool>(frame: &RaftEntryView<'_>) -> Result<serde_json::Value, ProjectorError> {
    let mut transactions = Vec::new();
    for i in 0..frame.tx_count() {
        // Both identities are read through SvTransactionInfo, not copied from
        // the configured decoder. U has no row changes but carries a T event.
        let tx = frame.tx(i)?;
        let mut events = Vec::new();
        for ordinal in 0..tx.event_count() {
            let event = tx.event(ordinal)?;
            events.push(serde_json::json!({"ordinal": ordinal, "seq": event.event_seq,
                "kind": event.event_kind, "bytes": event.payload,
                "unknown": event.event_kind != 1 && !(T && event.event_kind == 2)}));
        }
        let mut changes = Vec::new();
        for d in 0..tx.record_delta_count() {
            let (key, bytes) = match tx.record_delta(d)? {
                RecordDeltaView::Insert { key, after, .. } | RecordDeltaView::Update { key, after, .. } => (key, after),
                RecordDeltaView::Delete { .. } => return Err(failure()),
            };
            changes.push(serde_json::json!({"kind": key.kind, "id": key.sys_id,
                "bytes": bytes, "decoded": decoded::<T>(key.kind, bytes)}));
        }
        transactions.push(serde_json::json!({"seq": tx.result().tx_seq,
            "source": tx.source_schema_identity(), "target": tx.schema_identity(),
            "events": events, "changes": changes}));
    }
    Ok(serde_json::json!({"type":"frame", "index":frame.position().raft_index, "transactions":transactions}))
}
fn query(
    control: &ProjectorControlPort<'_>,
    key: EngineQueryKey<'_>,
    order: ReportOrder,
) -> Result<serde_json::Value, ProjectorError> {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut handle = loop {
        match control.offer_engine_query(&[key])? {
            QueryOffer::Queued(handle) => break handle,
            QueryOffer::OwnerQueueFull if Instant::now() < deadline => {
                order.pause();
            }
            _ => return Err(failure()),
        }
    };
    loop {
        match handle.poll()? {
            EngineQueryPoll::Ready(answer) => {
                let row = answer.record(0)?.map(|row| {
                    serde_json::json!({
                        "kind":row.kind,"id":row.sys_id,"revision":row.revision,"bytes":row.record,
                    })
                });
                return Ok(serde_json::json!({"index":answer.position().raft_index,
                    "schema":answer.schema_identity(),"row":row}));
            }
            EngineQueryPoll::Rejected(EngineQueryRejection::UnknownUniqueKey) => {
                return Ok(serde_json::json!({"rejected":"UnknownUniqueKey"}));
            }
            // Replay from an older local base may deliver the saved prefix
            // before the owner can serve reads. Observe the typed refusal and
            // keep consuming; waiting here could pin the replay frame budget.
            EngineQueryPoll::Rejected(EngineQueryRejection::RecoveryPending) => {
                return Ok(serde_json::json!({"rejected":"RecoveryPending"}));
            }
            EngineQueryPoll::Pending if Instant::now() < deadline => {
                order.pause();
            }
            _ => return Err(failure()),
        }
    }
}
fn observe_queries(
    control: &ProjectorControlPort<'_>,
    index: u64,
    report: &mut std::fs::File,
    order: ReportOrder,
) -> Result<serde_json::Value, ProjectorError> {
    let key = 1u64.to_be_bytes();
    stage(report, index, "query_old_key")?;
    let old = query(control, EngineQueryKey::UniqueKey { kind: 1, uk_id: 0, key: &key }, order)?;
    let by_id = match old["row"]["id"].as_u64() {
        Some(sys_id) => {
            stage(report, index, "query_sys_id")?;
            query(control, EngineQueryKey::SysId { kind: 1, sys_id }, order)?
        }
        None => serde_json::Value::Null,
    };
    stage(report, index, "query_new_key")?;
    let new = query(control, EngineQueryKey::UniqueKey { kind: 1, uk_id: 1, key: &key }, order)?;
    Ok(serde_json::json!({"type":"queries", "index":index, "old":old, "sys_id":by_id,"new":new}))
}
impl<const T: bool> PreparedProjector for Prepared<T> {
    fn start_request(&self) -> StartRequest {
        if self.store.is_none() {
            StartRequest::LatestState
        } else {
            StartRequest::Continuous { offset: self.restored.clone() }
        }
    }
    fn delivery_limits(&self) -> DeliveryLimits {
        DeliveryLimits::new(
            NonZeroUsize::new(2).unwrap(),
            NonZeroUsize::new(1024 * 1024).unwrap(),
        )
    }
    fn run(mut self, io: ProjectorIo<'_>) -> Result<(), ProjectorError> {
        let ProjectorIoParts { mut source, control, stop, .. } = io.split();
        let mut model = model::Model::default();
        write(
            &mut self.report,
            serde_json::json!({"type":"start","target_decoder":T,
            "restored":self.restored.as_ref().map(|p|p.raft_index)}),
        )?;
        loop {
            if stop.is_stopping()? {
                break;
            }
            if self.pause_path.as_ref().is_some_and(|path| path.exists()) {
                std::thread::park_timeout(Duration::from_millis(10));
                continue;
            }
            match source.next_timeout(Duration::from_millis(10))? {
                Delivery::RaftEntry(entry) => {
                    let view = entry.view();
                    let index = view.position().raft_index;
                    stage(&mut self.report, index, "received")?;
                    let observation = observe_frame::<T>(&view)?;
                    let mut pending = if let Some(store) = &mut self.store {
                        let token = entry.token();
                        let data = store::Entry::read(view, token.offset().ok_or_else(failure)?)?;
                        // A locked database clears: retry the held frame with a
                        // bounded backoff, still stopping when asked. Any other
                        // store error cannot clear and stops the learner.
                        let mut backoff = Duration::from_millis(1);
                        loop {
                            match store.commit_entry(&data) {
                                Ok(()) => break,
                                Err(store::Error::Database(rusqlite::Error::SqliteFailure(e, _)))
                                    if matches!(e.code, rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked) =>
                                {
                                    if stop.is_stopping()? {
                                        return Ok(());
                                    }
                                    stage(&mut self.report, index, "sink_busy")?;
                                    std::thread::park_timeout(backoff);
                                    backoff = (backoff * 2).min(Duration::from_millis(50));
                                }
                                Err(e) => return Err(e.failure(ProjectorIoStage::StoreWrite)),
                            }
                        }
                        // As in the history example, validate/idempotently read
                        // the replayed prefix without reporting an older P.
                        if index >= self.restored.as_ref().map_or(0, |p| p.raft_index) {
                            Some(PendingDurableOffset::new(token.declare_durable().map_err(|_| failure())?))
                        } else {
                            None
                        }
                    } else {
                        for t in 0..view.tx_count() {
                            let tx = view.tx(t)?;
                            for d in 0..tx.record_delta_count() {
                                model.change(tx.record_delta(d)?)?;
                            }
                        }
                        None
                    };
                    let mut held = Some(entry);
                    let mut observation = Some(observation);
                    if self.report_order == ReportOrder::ReleaseBeforeReport {
                        drop(held.take());
                        stage(&mut self.report, index, "released")?;
                        write(&mut self.report, observation.take().unwrap())?;
                    }
                    if let Some(pending) = &mut pending {
                        // This stage does not claim the report is queued:
                        // PendingDurableOffset retains the exact offer/receipt.
                        stage(&mut self.report, index, "durable_wait")?;
                        loop {
                            if stop.is_stopping()? {
                                return Ok(());
                            }
                            match pending.progress(&control)? {
                                ControlProgress::Accepted => break,
                                ControlProgress::Pending => self.report_order.pause(),
                                _ => return Err(failure()),
                            }
                        }
                        stage(&mut self.report, index, "durable_accepted")?;
                    }
                    if let Some(entry) = held.take() {
                        // Read the still-live loan after the actual Accepted
                        // reply, before returning its delivery credit.
                        if entry.view().position().raft_index != index {
                            return Err(failure());
                        }
                        drop(entry);
                        stage(&mut self.report, index, "released")?;
                        write(&mut self.report, observation.take().unwrap())?;
                    }
                    let queries = observe_queries(&control, index, &mut self.report, self.report_order)?;
                    write(&mut self.report, queries)?;
                    write(&mut self.report, serde_json::json!({"type":"ready","index":index}))?;
                }
                Delivery::Snapshot(mut loan) => {
                    let info = loan.info();
                    let mut records = Vec::new();
                    loop {
                        match loan.next_record()? {
                            SnapshotRead::Record(row) => {
                                records.push(serde_json::json!({"kind":row.key.kind,"id":row.key.sys_id,
                                    "bytes":row.bytes,"decoded":decoded::<T>(row.key.kind,row.bytes)}));
                                model.insert(row.key.kind, row.key.sys_id, row.revision, row.bytes)?;
                            }
                            SnapshotRead::Progress => (),
                            SnapshotRead::End => break,
                            SnapshotRead::Stop => return Ok(()),
                        }
                    }
                    let mut pending = PendingSnapshotConfirmation::new(loan.finish()?);
                    drop(loan);
                    loop {
                        if stop.is_stopping()? {
                            return Ok(());
                        }
                        match pending.progress(&control)? {
                            ControlProgress::Accepted => break,
                            ControlProgress::Pending => std::thread::park_timeout(Duration::from_millis(1)),
                            _ => return Err(failure()),
                        }
                    }
                    write(
                        &mut self.report,
                        serde_json::json!({"type":"baseline","index":info.position.raft_index,
                        "schema":info.schema_identity,"records":records,"count":info.record_count}),
                    )?;
                    let queries =
                        observe_queries(&control, info.position.raft_index, &mut self.report, self.report_order)?;
                    write(&mut self.report, queries)?;
                    write(&mut self.report, serde_json::json!({"type":"ready","index":info.position.raft_index}))?;
                }
                Delivery::Empty | Delivery::TimedOut => (),
                Delivery::Stop | Delivery::Disconnected => break,
            }
        }
        write(&mut self.report, serde_json::json!({"type":"stopped"}))
    }
}
