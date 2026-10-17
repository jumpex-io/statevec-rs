//! Durable public-SDK reference plugin. SQLite mode commits effects and the
//! four offset fields together; file mode uses idempotent DB effects then the
//! default offset file. All control work progresses during idle waits.
use statevec_projector_api::{ProjectorCheckpointStatus, plugin_v2::ProjectorRecovery, plugin_v2::*};
use std::{io::Write, num::NonZeroUsize, path::PathBuf, time::Duration};
#[path = "store.rs"]
pub(crate) mod store;

pub struct History;
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    database_path: PathBuf,
    #[serde(default)]
    offset_path: Option<PathBuf>,
    #[serde(default)]
    status_path: Option<PathBuf>,
}
pub struct Prepared {
    store: store::HistoryStore,
    restored: Option<statevec_projector_api::ConsumeOffset>,
    status: Option<std::fs::File>,
}
impl ProjectorModule for History {
    const RECOVERY_SUPPORT: ProjectorRecoverySupport = ProjectorRecoverySupport::ContinuousHistory;
    type Configured = Config;
    fn configure(&self, input: ProjectorConfig<'_>) -> Result<Config, ConfigureFailure> {
        struct Object;
        impl<'de> serde::de::Visitor<'de> for Object {
            type Value = Config;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("history configuration object")
            }
            fn visit_map<M: serde::de::MapAccess<'de>>(self, map: M) -> Result<Config, M::Error> {
                serde::Deserialize::deserialize(serde::de::value::MapAccessDeserializer::new(map))
            }
        }
        let mut decoder = serde_json::Deserializer::from_slice(input.bytes);
        let mut config: Config = serde::Deserializer::deserialize_map(&mut decoder, Object)
            .map_err(|_| ConfigureFailure::Invalid { code: 1 })?;
        decoder
            .end()
            .map_err(|_| ConfigureFailure::Invalid { code: 1 })?;
        for path in std::iter::once(&mut config.database_path)
            .chain(config.offset_path.iter_mut())
            .chain(config.status_path.iter_mut())
        {
            if path.as_os_str().is_empty() {
                return Err(ConfigureFailure::Invalid { code: 2 });
            }
            if path.is_relative() {
                *path = input.base_dir.join(&*path);
            }
        }
        Ok(config)
    }
}
impl ConfiguredProjector for Config {
    type Prepared = Prepared;
    fn prepare(&self, context: ProjectorPrepare<'_>) -> Result<Prepared, ProjectorFailure> {
        let mut store = store::HistoryStore::open(
            &self.database_path,
            self.offset_path.as_deref(),
            context.cluster_id,
            context.projector_id,
        )
        .map_err(|e| e.failure(ProjectorIoStage::StoreOpen))?;
        let restored = store
            .load()
            .map_err(|e| e.failure(ProjectorIoStage::StoreRead))?;
        let status = self
            .status_path
            .as_ref()
            .map(std::fs::File::create)
            .transpose()
            .map_err(|e| store::Error::Io(e).failure(ProjectorIoStage::StoreOpen))?;
        Ok(Prepared {
            store,
            restored,
            status,
        })
    }
}

fn progress_report<'io>(
    control: &ProjectorControlPort<'io>,
    report: &mut Option<PendingDurableOffset<'io>>,
    stop: &ProjectorStop<'io>,
) -> Result<(), ProjectorFailure> {
    let Some(pending) = report else { return Ok(()); };
    match pending.progress(control)? {
        ControlProgress::Pending => (),
        ControlProgress::Accepted => *report = None,
        ControlProgress::Rejected(statevec_projector_api::ProjectorCheckpointRejection::OwnerNotRunning)
            if stop.is_stopping()? => *report = None,
        ControlProgress::Rejected(_) => return Err(ProjectorFailure::Application { code: 1101 }),
        ControlProgress::Closed if stop.is_stopping()? => (),
        ControlProgress::Closed => return Err(ProjectorFailure::Application { code: 1102 }),
    }
    Ok(())
}
impl PreparedProjector for Prepared {
    fn recovery(&self) -> ProjectorRecovery {
        ProjectorRecovery::ContinuousHistory {
            offset: self.restored.clone(),
        }
    }
    fn delivery_limits(&self) -> DeliveryLimits {
        DeliveryLimits {
            frame_capacity: NonZeroUsize::new(2).unwrap(),
            maximum_accounted_bytes: NonZeroUsize::new(1024 * 1024).unwrap(),
        }
    }
    fn run(mut self, io: ProjectorIo<'_>) -> Result<(), ProjectorFailure> {
        let ProjectorIoParts {
            mut source,
            control,
            stop,
            ..
        } = io.split();
        let mut report = None;
        let mut status_reply: Option<ReplyHandle<'_, ProjectorCheckpointStatus>> = None;
        let mut observed = None;
        loop {
            if stop.is_stopping()? {
                return Ok(());
            }
            progress_report(&control, &mut report, &stop)?;
            // Poll status even without a frame, and during hot delivery.
            if let Some(reply) = &mut status_reply {
                match reply.poll()? {
                    ReplyPoll::Ready(value) => {
                        let model_index = self
                            .store
                            .model_index()
                            .map_err(|e| e.failure(ProjectorIoStage::StoreRead))?;
                        if let ProjectorCheckpointStatus::Active {
                            durable_raft_index, ..
                        } = value
                        {
                            if durable_raft_index >= model_index
                                && observed != Some(durable_raft_index)
                            {
                                if let Some(file) = &mut self.status {
                                    writeln!(file, "ready {durable_raft_index} {model_index}")
                                        .and_then(|()| file.flush())
                                        .map_err(|e| {
                                            store::Error::Io(e)
                                                .failure(ProjectorIoStage::StoreWrite)
                                        })?;
                                }
                                observed = Some(durable_raft_index);
                            }
                        }
                        status_reply = None;
                    }
                    ReplyPoll::Disconnected => status_reply = None,
                    ReplyPoll::Pending => (),
                }
            }
            if status_reply.is_none() {
                match control.offer_status()? {
                    Offer::Queued(reply) => status_reply = Some(reply),
                    Offer::OwnerQueueFull | Offer::ReplyCapacityExhausted => (),
                    Offer::Closed if stop.is_stopping()? => return Ok(()),
                    Offer::Closed => return Err(ProjectorFailure::Application { code: 1102 }),
                }
            }
            if report.is_some() {
                std::thread::park_timeout(Duration::from_millis(10));
                continue;
            }
            match source.next_timeout(Duration::from_millis(10))? {
                Delivery::Frame(loan) => {
                    let cut = loan.source_cut();
                    let entry = store::Entry::read(loan.view(), cut.offset().ok_or(ProjectorFailure::Application { code: 1103 })?)?;
                    self.store
                        .commit_entry(&entry)
                        .map_err(|e| e.failure(ProjectorIoStage::StoreWrite))?;
                    // No automatic ACK: this assertion follows confirmed save.
                    let restored = self.restored.as_ref().map_or(0, |p| p.raft_index);
                    if entry.offset.raft_index >= restored {
                        report = Some(PendingDurableOffset::new(cut.declare_durable().map_err(|_| ProjectorFailure::Application { code: 1103 })?));
                    }
                }
                Delivery::Empty | Delivery::TimedOut => (),
                Delivery::State(_) => return Err(ProjectorFailure::AbiContract(ProjectorAbiFailure::InvalidHandle)),
                Delivery::Stop => return Ok(()),
                Delivery::Disconnected => {
                    if stop.is_stopping()? {
                        return Ok(());
                    }
                    return Err(ProjectorFailure::Application { code: 1103 });
                }
            }
        }
    }
}
