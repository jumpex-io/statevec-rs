//! A configured, stop-aware observer using the generated worker lifecycle.
//! It writes observations, NOT a durable effects/offset store: it never offers
//! checkpoints and recovery truthfully returns None. Use on a fresh learner
//! whose base is genesis; durable consumption belongs to the offset-store API.
use statevec_projector_api::{v1::StartRequest, v1::*};
use std::{io::Write, num::NonZeroUsize, path::PathBuf, time::Duration};

struct Observer;
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Config { report_path: PathBuf }
struct Prepared { report: std::fs::File }
impl ProjectorModule for Observer {
    const SUPPORTED_HISTORY_MODES: SupportedHistoryModes = SupportedHistoryModes::Continuous;
    type Configured = Config;
    fn configure(&self, input: ProjectorConfig<'_>) -> Result<Config, ConfigureError> {
        let mut config: Config = serde_json::from_slice(input.bytes).map_err(|_| ConfigureError::Invalid { code: 1 })?;
        if config.report_path.as_os_str().is_empty() { return Err(ConfigureError::Invalid { code: 2 }); }
        if config.report_path.is_relative() { config.report_path = input.base_dir.join(config.report_path); }
        Ok(config)
    }
}
fn failure(error: std::io::Error, stage: ProjectorIoStage) -> ProjectorError {
    ProjectorError::Io { stage, kind: ProjectorIoKind::from_io(error.kind()), os_code: error.raw_os_error() }
}
impl ConfiguredProjector for Config {
    type Prepared = Prepared;
    fn prepare(&self, _: ProjectorPrepare<'_>) -> Result<Prepared, ProjectorError> {
        let report = std::fs::File::create(&self.report_path).map_err(|e| failure(e, ProjectorIoStage::StoreOpen))?;
        Ok(Prepared { report })
    }
}
impl PreparedProjector for Prepared {
    fn start_request(&self) -> StartRequest { StartRequest::Continuous { offset: None } }
    fn delivery_limits(&self) -> DeliveryLimits {
        DeliveryLimits { frame_capacity: NonZeroUsize::new(2).unwrap(), maximum_accounted_bytes: NonZeroUsize::new(1024 * 1024).unwrap() }
    }
    fn run(mut self, io: ProjectorIo<'_>) -> Result<(), ProjectorError> {
        let ProjectorIoParts { mut source, services, .. } = io.split();
        services.spawn(|stop| {
            while !stop.is_stopping()? { std::thread::park_timeout(Duration::from_millis(1)); }
            Ok(())
        })?;
        writeln!(self.report, "running").map_err(|e| failure(e, ProjectorIoStage::StoreWrite))?;
        loop {
            match source.next_timeout(Duration::from_secs(60))? {
                Delivery::RaftEntry(entry) => {
                    let view = entry.view();
                    let position = view.position();
                    writeln!(self.report, "frame {} {} {}", position.raft_term, position.raft_index, view.tx_count())
                        .map_err(|e| failure(e, ProjectorIoStage::StoreWrite))?;
                }
                Delivery::Stop | Delivery::Disconnected => break,
                Delivery::Empty | Delivery::TimedOut => (),
                Delivery::Snapshot(_) => return Err(ProjectorError::AbiContract(ProjectorAbiError::InvalidHandle)),
            }
        }
        writeln!(self.report, "stopped").map_err(|e| failure(e, ProjectorIoStage::StoreWrite))
    }
}
statevec_projector_api::export_projector_plugin_v2!(Observer);
