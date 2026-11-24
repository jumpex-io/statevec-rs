use sdk_source_consumer::schema;
use statevec_projector_api::plugin::{
    ConfigureError, ConfiguredProjector, ControlDecision, Delivery, DeliveryLimits, DurableOffsetReport, HistoryMode,
    Offer, PreparedProjector, ProjectorConfig, ProjectorControlPort, ProjectorError, ProjectorIo, ProjectorModule,
    ProjectorPrepare, ReplyPoll, SchemaCompatibility, SourceIdentity, StartRequest, SupportedHistoryModes,
};
use statevec_projector_api::{AppliedTxProgress, ReadLimits, TxChainHash, TxSeq};
use std::{num::NonZeroUsize, path::Path, sync::OnceLock, time::Duration};

// A Projector can name the delivered frontier and the host query limits from its own dependency.
const _: fn(AppliedTxProgress) -> (TxSeq, TxChainHash) = |progress| (progress.applied_tx_seq, progress.tx_chain_hash);
const _: ReadLimits = statevec_projector_api::plugin::ffi::ENGINE_QUERY_LIMITS;

struct Observer;

impl ProjectorModule for Observer {
    type Configured = Observer;
    const SUPPORTED_HISTORY_MODES: SupportedHistoryModes = SupportedHistoryModes::Continuous;
    const SCHEMA_COMPATIBILITY: SchemaCompatibility = SchemaCompatibility::Additive;

    fn decoder_idl() -> &'static [u8] {
        static IDL: OnceLock<String> = OnceLock::new();
        IDL.get_or_init(|| schema::registry().to_idl_json()).as_bytes()
    }

    fn configure(&self, _: ProjectorConfig<'_>) -> Result<Observer, ConfigureError> {
        Ok(Observer)
    }
}

impl ConfiguredProjector for Observer {
    type Prepared = Observer;

    fn prepare(&self, _: ProjectorPrepare<'_>) -> Result<Observer, ProjectorError> {
        Ok(Observer)
    }
}

impl PreparedProjector for Observer {
    fn start_request(&self) -> StartRequest {
        StartRequest::Continuous { offset: None }
    }

    fn delivery_limits(&self) -> DeliveryLimits {
        DeliveryLimits::new(NonZeroUsize::MIN, NonZeroUsize::new(65536).unwrap())
    }

    fn run(self, io: ProjectorIo<'_>) -> Result<(), ProjectorError> {
        let mut source = io.split().source;
        loop {
            match source.next_timeout(Duration::from_secs(1))? {
                Delivery::RaftEntry(entry) => println!("observed entry {}", entry.view().position().raft_index),
                Delivery::Stop | Delivery::Disconnected => return Ok(()),
                Delivery::Empty | Delivery::TimedOut => {}
                Delivery::Snapshot(_) => return Err(ProjectorError::Application { code: 1 }),
            }
        }
    }
}

/// Type-check the concrete replies an author can poll after queueing each request.
pub fn poll_control_examples(
    control: &ProjectorControlPort<'_>,
    durable: &DurableOffsetReport,
) -> Result<(), ProjectorError> {
    if let Offer::Queued(mut handle) = control.offer_status()? {
        let _: ReplyPoll<statevec_projector_api::ProjectorStatus> = handle.poll()?;
    }
    if let Offer::Queued(mut handle) = control.report_durable_offset(durable)? {
        let _: ReplyPoll<ControlDecision> = handle.poll()?;
    }
    Ok(())
}

statevec_projector_api::export_projector_plugin!(Observer);

/// Exercise the author's configure and prepare callbacks without starting a node.
pub fn offline_start_request(base_dir: &Path, identity: SourceIdentity) -> StartRequest {
    let configured = Observer.configure(ProjectorConfig::new(b"{}", base_dir)).expect("configure observer");
    let input = ProjectorPrepare::new(
        identity, 4, "trial", "observer", Observer::decoder_idl(), None, HistoryMode::Continuous,
    );
    configured.prepare(input).expect("prepare observer").start_request()
}

#[test]
fn offline_observer_starts_at_the_beginning_of_history() {
    use statevec_projector_api::{Digest32, ProjectionManifest};

    let identity = SourceIdentity {
        cluster_identity: Digest32::new([1; 32]),
        genesis_identity: Digest32::new([2; 32]),
        execution_lineage_identity: Digest32::new([3; 32]),
        manifest: ProjectionManifest::AllRegisteredSchema,
    };
    assert_eq!(offline_start_request(Path::new("."), identity), StartRequest::Continuous { offset: None });
}
