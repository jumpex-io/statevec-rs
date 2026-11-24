//! Linux qualification only: the generated worker reads its learner's own
//! engine records through the host query table after each frame, and after a
//! latest-state baseline. It writes observations, NOT a durable store: start
//! continuous history on a fresh learner at genesis.
#[allow(dead_code, unused_imports)]
#[path = "../../../../crates/statevec/examples/core_bank/schema.rs"]
mod bank_decoder;
#[cfg(target_os = "linux")]
#[path = "query_trial.rs"]
mod trial;
use statevec_projector_api::plugin::{StartRequest, *};
use std::collections::BTreeMap;
use std::io::Write;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::time::{Duration, Instant};

struct QueryProbe;
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Config { report_path: PathBuf, trial_after_tx: Option<u64> }
struct Prepared { report: std::fs::File, latest: bool, trial_after_tx: Option<u64> }
impl ProjectorModule for QueryProbe {
    fn decoder_idl() -> &'static [u8] { bank_decoder::decoder_idl() }
    const SUPPORTED_HISTORY_MODES: SupportedHistoryModes = SupportedHistoryModes::Both;
    type Configured = Config;
    fn configure(&self, input: ProjectorConfig<'_>) -> Result<Config, ConfigureError> {
        let mut config: Config = serde_json::from_slice(input.bytes).map_err(|_| ConfigureError::Invalid { code: 1 })?;
        if config.report_path.as_os_str().is_empty() { return Err(ConfigureError::Invalid { code: 2 }); }
        if config.report_path.is_relative() { config.report_path = input.base_dir.join(config.report_path); }
        Ok(config)
    }
}
fn failure(error: std::io::Error) -> ProjectorError {
    ProjectorError::Io { stage: ProjectorIoStage::StoreWrite, kind: ProjectorIoKind::from_io(error.kind()), os_code: error.raw_os_error() }
}
impl ConfiguredProjector for Config {
    type Prepared = Prepared;
    fn prepare(&self, input: ProjectorPrepare<'_>) -> Result<Prepared, ProjectorError> {
        let latest = input.history_mode == HistoryMode::LatestState;
        Ok(Prepared { report: std::fs::File::create(&self.report_path).map_err(failure)?, latest, trial_after_tx: self.trial_after_tx })
    }
}
/// The last write of each record in one frame: (kind, sys_id) -> (revision, bytes).
type Written = BTreeMap<(u16, u64), (u64, Vec<u8>)>;
impl PreparedProjector for Prepared {
    fn start_request(&self) -> StartRequest {
        if self.latest { StartRequest::LatestState } else { StartRequest::Continuous { offset: None } }
    }
    fn delivery_limits(&self) -> DeliveryLimits {
        DeliveryLimits::new(NonZeroUsize::new(2).unwrap(), NonZeroUsize::new(1024 * 1024).unwrap())
    }
    fn run(mut self, io: ProjectorIo<'_>) -> Result<(), ProjectorError> {
        if let Some(after) = self.trial_after_tx {
            #[cfg(target_os = "linux")]
            return trial::run(io, self.report, after);
            #[cfg(not(target_os = "linux"))]
            return Err(ProjectorError::Application { code: 953 });
        }
        let ProjectorIoParts { mut source, control, services, .. } = io.split();
        services.spawn(|stop| {
            while !stop.is_stopping()? { std::thread::park_timeout(Duration::from_millis(1)); }
            Ok(())
        })?;
        writeln!(self.report, "supported {}", u8::from(control.engine_query_supported())).map_err(failure)?;
        let mut released = 0u64;
        'run: loop {
            match source.next_timeout(Duration::from_secs(60))? {
                Delivery::RaftEntry(entry) => {
                    let (index, written) = {
                        let view = entry.view();
                        let mut written = Written::new();
                        for t in 0..view.tx_count() {
                            let tx = view.tx(t)?;
                            for d in 0..tx.record_delta_count() {
                                match tx.record_delta(d)? {
                                    RecordDeltaView::Insert { key, revision, after } | RecordDeltaView::Update { key, revision, after, .. } => {
                                        written.insert((key.kind, key.sys_id), (revision, after.to_vec()));
                                    }
                                    RecordDeltaView::Delete { .. } => (),
                                }
                            }
                        }
                        (view.position().raft_index, written)
                    };
                    // The frame is released before the query: the plugin
                    // consumes nothing more until it is answered.
                    drop(entry);
                    writeln!(self.report, "frame {index} {}", written.len()).map_err(failure)?;
                    if !control.engine_query_supported() { continue; }
                    for ((kind, sys_id), (revision, after)) in written {
                        let line = query_one(&control, index, kind, sys_id, revision, &after)?;
                        released += 1;
                        writeln!(self.report, "{line}").map_err(failure)?;
                    }
                }
                Delivery::Stop | Delivery::Disconnected => break,
                Delivery::Empty | Delivery::TimedOut => (),
                Delivery::Snapshot(mut loan) => {
                    // Accept the baseline, then read each of its records back.
                    let mut records = Vec::new();
                    loop {
                        match loan.next_record()? {
                            SnapshotRead::Record(r) => records.push((r.key.kind, r.key.sys_id, r.revision, r.bytes.to_vec())),
                            SnapshotRead::Progress => (),
                            SnapshotRead::End => break,
                            SnapshotRead::Stop => break 'run,
                        }
                    }
                    let index = loan.info().position.raft_index;
                    let mut pending = PendingSnapshotConfirmation::new(loan.finish()?);
                    drop(loan);
                    let deadline = Instant::now() + Duration::from_secs(10);
                    loop {
                        match pending.progress(&control)? {
                            ControlProgress::Accepted => break,
                            ControlProgress::Pending if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(1)),
                            _ => return Err(ProjectorError::Application { code: 952 }),
                        }
                    }
                    writeln!(self.report, "baseline {index} {}", records.len()).map_err(failure)?;
                    for (kind, sys_id, revision, after) in records {
                        let line = query_one(&control, index, kind, sys_id, revision, &after)?;
                        released += 1;
                        writeln!(self.report, "{line}").map_err(failure)?;
                    }
                }
            }
        }
        writeln!(self.report, "stopped released={released}").map_err(failure)
    }
}
/// `query <frame> <answered at> <sys_id> <found> <frame revision> <answer revision> <same bytes>`.
/// The handle is dropped, so its answer released, before this returns.
fn query_one(control: &ProjectorControlPort<'_>, index: u64, kind: u16, sys_id: u64, revision: u64, after: &[u8]) -> Result<String, ProjectorError> {
    let deadline = Instant::now() + Duration::from_secs(10);
    let keys = [EngineQueryKey::SysId { kind, sys_id }];
    let mut handle = loop {
        match control.offer_engine_query(&keys)? {
            QueryOffer::Queued(handle) => break handle,
            QueryOffer::OwnerQueueFull if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(1)),
            _ => return Err(ProjectorError::Application { code: 950 }),
        }
    };
    loop {
        match handle.poll()? {
            EngineQueryPoll::Pending if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(1)),
            EngineQueryPoll::Ready(result) => {
                let at = result.position().raft_index;
                return Ok(match result.record(0)? {
                    Some(record) => format!("query {index} {at} {sys_id} 1 {revision} {} {}", record.revision, u8::from(record.record == after)),
                    None => format!("query {index} {at} {sys_id} 0 {revision} 0 0"),
                });
            }
            EngineQueryPoll::Rejected(cause) => return Ok(format!("rejected {index} {cause:?}")),
            _ => return Err(ProjectorError::Application { code: 951 }),
        }
    }
}
statevec_projector_api::export_projector_plugin!(QueryProbe);
