//! Public SDK only: fresh private model -> real EOF -> reset -> baseline ACK
//! -> ordered deltas. No offset, cache or report is ever read for recovery.
use statevec_projector_api::{ProjectorInitialization, plugin_v2::ProjectorRecovery, plugin_v2::*};
use std::{io::Write, num::NonZeroUsize, path::PathBuf, time::Duration};
#[path = "model.rs"]
pub mod model;
use model::{Model, failure};

pub struct Latest;
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config { report_path: PathBuf }
pub struct Prepared { report: std::fs::File }
impl ProjectorModule for Latest {
    const RECOVERY_SUPPORT: ProjectorRecoverySupport = ProjectorRecoverySupport::LatestStateOnly;
    type Configured = Config;
    fn configure(&self, input: ProjectorConfig<'_>) -> Result<Config, ConfigureFailure> {
        struct Object;
        impl<'de> serde::de::Visitor<'de> for Object {
            type Value = Config;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str("latest configuration object") }
            fn visit_map<M: serde::de::MapAccess<'de>>(self, map: M) -> Result<Config, M::Error> {
                serde::Deserialize::deserialize(serde::de::value::MapAccessDeserializer::new(map))
            }
        }
        let mut decoder = serde_json::Deserializer::from_slice(input.bytes);
        let mut config: Config = serde::Deserializer::deserialize_map(&mut decoder, Object)
            .map_err(|_| ConfigureFailure::Invalid { code: 1 })?;
        decoder.end().map_err(|_| ConfigureFailure::Invalid { code: 1 })?;
        if config.report_path.as_os_str().is_empty() { return Err(ConfigureFailure::Invalid { code: 2 }); }
        if config.report_path.is_relative() { config.report_path = input.base_dir.join(config.report_path); }
        Ok(config)
    }
}
fn io_failure(source: std::io::Error, stage: ProjectorIoStage) -> ProjectorFailure {
    ProjectorFailure::Io { stage, kind: ProjectorIoKind::from_io(source.kind()), os_code: source.raw_os_error() }
}
impl ConfiguredProjector for Config {
    type Prepared = Prepared;
    fn prepare(&self, _: ProjectorPrepare<'_>) -> Result<Prepared, ProjectorFailure> {
        // Append output, never read it or truncate an old application cache.
        let report = std::fs::OpenOptions::new().append(true).create(true).open(&self.report_path)
            .map_err(|e| io_failure(e, ProjectorIoStage::StoreOpen))?;
        Ok(Prepared { report })
    }
}
fn write(report: &mut std::fs::File, row: serde_json::Value) -> Result<(), ProjectorFailure> {
    let mut bytes = serde_json::to_vec(&row).map_err(|_| failure(1208))?;
    bytes.push(b'\n');
    report.write_all(&bytes).and_then(|()| report.flush()).map_err(|e| io_failure(e, ProjectorIoStage::StoreWrite))
}
pub(crate) fn write_reset(report: &mut std::fs::File, model: &Model, target: u64) -> Result<(), ProjectorFailure> {
    // Isolate a prior process's partial last row without reading old output.
    // Readers resynchronize at reset; this is framing, not recovery/durability.
    report.write_all(b"\n").map_err(|e| io_failure(e, ProjectorIoStage::StoreWrite))?;
    write(report, model.reset(target))
}
// Queue pressure and reply loss retain this exact EOF-issued cut. No retry can
// fabricate a replacement or assert persistent history. Stop is not a receipt.
fn accept(control: &ProjectorControlPort<'_>, stop: &ProjectorStop<'_>, cut: &RebuildCut) -> Result<bool, ProjectorFailure> {
    let mut pending = PendingRebuildConfirmation::new(*cut);
    loop {
        if stop.is_stopping()? { return Ok(false); }
        match pending.progress(control)? {
            ControlProgress::Accepted => return Ok(true),
            ControlProgress::Pending => (),
            ControlProgress::Rejected(_) | ControlProgress::Closed if stop.is_stopping()? => return Ok(false),
            ControlProgress::Rejected(_) => return Err(failure(1204)),
            ControlProgress::Closed => return Err(failure(1205)),
        }
        std::thread::park_timeout(Duration::from_millis(10));
    }
}
impl PreparedProjector for Prepared {
    fn recovery(&self) -> ProjectorRecovery { ProjectorRecovery::LatestStateOnly }
    fn delivery_limits(&self) -> DeliveryLimits {
        DeliveryLimits { frame_capacity: NonZeroUsize::new(2).unwrap(), maximum_accounted_bytes: NonZeroUsize::new(1024 * 1024).unwrap() }
    }
    fn run(mut self, io: ProjectorIo<'_>) -> Result<(), ProjectorFailure> {
        let ProjectorInitialization::Rebuild { target } = io.initialization() else { return Err(failure(1206)); };
        let ProjectorIoParts { mut source, control, stop, .. } = io.split();
        let mut model = Model::default();
        let cut = loop {
            match source.next_timeout(Duration::from_millis(10))? {
                Delivery::State(mut state) => {
                    loop {
                        match state.next_record()? {
                            StateRead::Record(record) => model.insert(record.key.kind, record.key.sys_id, record.revision, record.bytes)?,
                            StateRead::Progress => (),
                            StateRead::End => break,
                            StateRead::Stop => return Ok(()),
                        }
                    }
                    break state.finish()?;
                }
                Delivery::Empty | Delivery::TimedOut => (),
                Delivery::Stop => return Ok(()),
                Delivery::Disconnected if stop.is_stopping()? => return Ok(()),
                _ => return Err(failure(1206)),
            }
        };
        // Complete local installation and subscriber reset precede suffix.
        // This output is NOT a durable checkpoint; no fsync/offset is needed.
        if stop.is_stopping()? { return Ok(()); }
        write_reset(&mut self.report, &model, target)?;
        if !accept(&control, &stop, &cut)? { return Ok(()); }
        write(&mut self.report, serde_json::json!({ "type": "ready", "raft_index": target.to_string() }))?;
        let mut index = target;
        loop {
            match source.next_timeout(Duration::from_millis(10))? {
                Delivery::Frame(frame) => {
                    let view = frame.view();
                    let next = view.position().raft_index;
                    if index.checked_add(1) != Some(next) || frame.source_cut().offset().is_some() { return Err(failure(1207)); }
                    let mut changes = Vec::new();
                    for i in 0..view.transaction_count() {
                        let tx = view.transaction(i)?;
                        for j in 0..tx.change_count() { changes.push(model.change(tx.change(j)?)?); }
                    }
                    write(&mut self.report, serde_json::json!({ "type": "delta", "raft_index": next.to_string(), "changes": changes }))?;
                    index = next;
                }
                Delivery::Empty | Delivery::TimedOut => (),
                Delivery::Stop => return Ok(()),
                Delivery::Disconnected if stop.is_stopping()? => return Ok(()),
                _ => return Err(failure(1206)),
            }
        }
    }
}
