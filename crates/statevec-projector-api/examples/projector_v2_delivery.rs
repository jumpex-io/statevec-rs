//! Public configured exporter and a separate loaded transport conformance probe.
//! The business consumer uses safe lending/control facades; the raw probe also
//! checks mode refusals through the caller's explicitly borrowed C table.
use statevec_projector_api::v1::{self as p, ffi::*, *};
use sha2::{Digest, Sha256};
#[path = "projector_v2_delivery/mode_probe.rs"]
mod mode_probe;
#[path = "projector_v2_delivery/rebuild_probe.rs"]
mod rebuild_probe;

#[repr(C)]
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct Report {
    pub frames: u64, pub empty: u64, pub replay: u64, pub transactions: u64,
    pub events: u64, pub changes: u64, pub mode_checks: u64, pub last_index: u64, pub last_term: u64,
    pub last_tx_seq: u64, pub chain: [u8; 32], pub digest: [u8; 32],
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn statevec_projector_delivery_probe_v2(
    table: *const SvProjectorHostV2, startup: *const SvProjectorStart, count: u64, report: *mut Report, completion: *mut SvProjectorCompletion,
) -> SvStatus {
    if table.is_null() || startup.is_null() || report.is_null() || completion.is_null() { return (&ProjectorAbiError::NullPointer).into(); }
    // SAFETY: test caller lends these complete disjoint outputs and live table
    // through synchronous completion; no pointer survives this entry.
    let mut result = Report::default();
    result.mode_checks = match unsafe { mode_probe::check_continuous_refusals(&*table) } {
        Ok(checked) => checked,
        Err(cause) => return (&cause).into(),
    };
    let run = unsafe { p::host::with_host(&*table, &*startup, |io| {
        let parts = io.split();
        let control = parts.control.clone();
        let cut = consume(parts, count, &mut result)?;
        offer_fixture_checkpoint(control, cut)
    }) };
    match run {
        Ok(value) => { unsafe { report.write(result); completion.write(value); } SvStatus::SUCCESS }
        Err(cause) => (&cause).into(),
    }
}

fn consume(parts: ProjectorIoParts<'_>, count: u64, result: &mut Report) -> Result<RaftEntryToken, ProjectorError> {
        let ProjectorIoParts { mut source, control: _, services, stop: _ } = parts;
        // An actual scoped borrower is joined before its context can retire.
        services.spawn(|stop| {
            while !stop.is_stopping()? { std::thread::park_timeout(std::time::Duration::from_micros(200)); }
            Ok(())
        })?;
        let mut hash = Sha256::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut latest = None;
        while result.frames < count {
            if std::time::Instant::now() >= deadline { return Err(ProjectorError::Application { code: 900 }); }
            match source.next_timeout(std::time::Duration::from_millis(2))? {
                Delivery::RaftEntry(loan) => {
                    let view = loan.view();
                    result.frames += 1;
                    result.empty += u64::from(view.tx_count() == 0);
                    result.replay += u64::from(view.context() == statevec_projector_api::ProjectionContext::Replay);
                    let position = view.position();
                    result.last_index = position.raft_index; result.last_term = position.raft_term;
                    result.last_tx_seq = position.frontier.applied_tx_seq; result.chain = position.frontier.tx_chain_hash;
                    for index in 0..view.tx_count() {
                        let tx = view.tx(index)?;
                        let r = tx.result();
                        result.transactions += 1;
                        hash.update(r.tx_seq.to_le_bytes()); hash.update(r.ordinal.to_le_bytes());
                        hash.update(r.tx_id.bytes()); hash.update(r.tx_chain_hash.bytes());
                        hash.update([r.result_digest_profile]); hash.update(r.result_digest);
                        hash.update(r.system_code.to_le_bytes()); hash.update(r.business_code.to_le_bytes());
                        let s = tx.schema_identity();
                        hash.update([s.schema_version.main(), s.schema_version.minor()]);
                        hash.update(s.types_schema_fingerprint); hash.update(s.record_schema_fingerprint);
                        hash.update(s.command_schema_fingerprint); hash.update(s.event_schema_fingerprint);
                        hash.update(tx.ref_tx_time_ns().to_le_bytes());
                        for index in 0..tx.event_count() {
                            let event = tx.event(index)?; result.events += 1;
                            hash.update(event.event_seq.to_le_bytes()); hash.update(event.event_kind.to_le_bytes());
                            hash.update((event.payload.len() as u64).to_le_bytes()); hash.update(event.payload);
                        }
                        for index in 0..tx.record_delta_count() {
                            result.changes += 1;
                            let (tag, key, revision, before, after) = match tx.record_delta(index)? {
                                RecordDeltaView::Insert { key, revision, after } => (0u8, key, revision, &[][..], after),
                                RecordDeltaView::Update { key, revision, before, after } => (1, key, revision, before, after),
                                RecordDeltaView::Delete { key, revision, before } => (2, key, revision, before, &[][..]),
                            };
                            hash.update([tag]); hash.update(key.kind.to_le_bytes()); hash.update(key.sys_id.to_le_bytes());
                            hash.update(revision.to_le_bytes());
                            hash.update((before.len() as u64).to_le_bytes()); hash.update(before);
                            hash.update((after.len() as u64).to_le_bytes()); hash.update(after);
                        }
                    }
                    latest = Some(loan.token());
                }
                Delivery::Empty | Delivery::TimedOut => {},
                Delivery::Stop | Delivery::Disconnected | Delivery::Snapshot(_) => return Err(ProjectorError::Application { code: 901 }),
            }
        }
        result.digest = hash.finalize().into();
        latest.ok_or(ProjectorError::Application { code: 902 })
}

fn offer_fixture_checkpoint(control: ProjectorControlPort<'_>, cut: RaftEntryToken) -> Result<(), ProjectorError> {
        // Only the raw transport probe calls this synthetic acknowledgement. It
        // tests the control channel, not durable storage or crash recovery.
        // A real author must save effects AND this offset durably before offering
        // it, then return that saved offset from start_request() on the next startup.
        // The report-only configured example below deliberately does neither.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        cut.offset().ok_or(ProjectorError::Application { code: 903 })?.encode_properties().map_err(|_| ProjectorError::Application { code: 903 })?;
        let publication = cut.declare_durable().map_err(|_| ProjectorError::Application { code: 903 })?;
        let mut reply = loop {
            match control.report_durable_offset(&publication)? {
                Offer::Queued(reply) => break reply,
                Offer::OwnerQueueFull => { if std::time::Instant::now() >= deadline { return Err(ProjectorError::Application { code: 904 }); } std::thread::yield_now(); },
                _ => return Err(ProjectorError::Application { code: 905 }),
            }
        };
        loop {
            match reply.poll()? {
                ReplyPoll::Ready(ControlDecision::Accepted) => break,
                ReplyPoll::Pending if std::time::Instant::now() < deadline => std::thread::yield_now(),
                _ => return Err(ProjectorError::Application { code: 906 }),
            }
        }
        Ok(())
}

/// A runnable public-author example. Its report is a conformance observation,
/// not a durable business store/offset implementation. It exits after a bounded
/// input prefix so the loaded test can examine actual worker retirement.
struct DeliveryModule;
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct DeliveryConfig { report_path: std::path::PathBuf, frames: u64 }
struct PreparedDelivery { file: std::fs::File, frames: u64 }
impl ProjectorModule for DeliveryModule {
    const SUPPORTED_HISTORY_MODES: SupportedHistoryModes = SupportedHistoryModes::Continuous;
    type Configured = DeliveryConfig;
    fn configure(&self, input: ProjectorConfig<'_>) -> Result<DeliveryConfig, ConfigureError> {
        let mut parsed: DeliveryConfig = serde_json::from_slice(input.bytes).map_err(|_| ConfigureError::Invalid { code: 1 })?;
        if parsed.frames == 0 { return Err(ConfigureError::Invalid { code: 2 }); }
        if parsed.report_path.is_relative() { parsed.report_path = input.base_dir.join(&parsed.report_path); }
        Ok(parsed)
    }
}
impl ConfiguredProjector for DeliveryConfig {
    type Prepared = PreparedDelivery;
    fn prepare(&self, context: ProjectorPrepare<'_>) -> Result<PreparedDelivery, ProjectorError> {
        if context.history_mode != statevec_projector_api::v1::HistoryMode::Continuous {
            return Err(ProjectorError::Application { code: 1 });
        }
        let file = std::fs::File::create(&self.report_path).map_err(|e| io_failure(e, ProjectorIoStage::StoreOpen))?;
        Ok(PreparedDelivery { file, frames: self.frames })
    }
}
impl PreparedProjector for PreparedDelivery {
    fn start_request(&self) -> statevec_projector_api::v1::StartRequest { statevec_projector_api::v1::StartRequest::Continuous { offset: None } }
    fn delivery_limits(&self) -> DeliveryLimits {
        DeliveryLimits { frame_capacity: std::num::NonZeroUsize::new(1).unwrap(), maximum_accounted_bytes: std::num::NonZeroUsize::new(1024 * 1024).unwrap() }
    }
    fn run(mut self, io: ProjectorIo<'_>) -> Result<(), ProjectorError> {
        use std::io::Write;
        let mut report = Report::default();
        let _cut = consume(io.split(), self.frames, &mut report)?;
        // Writing a report does not establish durable effects plus an offset.
        // Do not call report_durable_offset: start_request() truthfully reports None.
        let bytes = serde_json::to_vec(&report).map_err(|_| ProjectorError::Application { code: 7 })?;
        self.file.write_all(&bytes).map_err(|e| io_failure(e, ProjectorIoStage::StoreWrite))
    }
}
fn io_failure(e: std::io::Error, stage: ProjectorIoStage) -> ProjectorError {
    ProjectorError::Io { stage, kind: ProjectorIoKind::from_io(e.kind()), os_code: e.raw_os_error() }
}
statevec_projector_api::export_projector_plugin_v2!(DeliveryModule);
