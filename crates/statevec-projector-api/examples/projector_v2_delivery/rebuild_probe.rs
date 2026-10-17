//! Test-only loaded ABI probe, not a configured latest-state application.
//! The model is intentionally local; native source, EOF, token and owner reply
//! all come through the public SDK. No durable offset or fixture cut is used.
use statevec_projector_api::v1::host::*;
use super::*;
use std::{collections::BTreeMap, time::{Duration, Instant}};

type Model = BTreeMap<(u16, u64), (u64, Vec<u8>)>;
#[repr(C)]
#[derive(Default)]
pub struct RebuildReport {
    pub baseline: u64,
    pub records: u64,
    pub model_records: u64,
    pub suffix_index: u64,
    pub baseline_digest: [u8; 32],
    pub suffix_digest: [u8; 32],
}
fn fail(code: u32) -> ProjectorError { ProjectorError::Application { code } }
fn digest(model: &Model) -> [u8; 32] {
    let mut hash = Sha256::new();
    for ((kind, id), (revision, bytes)) in model {
        hash.update(kind.to_le_bytes()); hash.update(id.to_le_bytes());
        hash.update(revision.to_le_bytes()); hash.update((bytes.len() as u64).to_le_bytes()); hash.update(bytes);
    }
    hash.finalize().into()
}
fn accept(control: &ProjectorControlPort<'_>, cut: &SnapshotToken) -> Result<(), ProjectorError> {
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut reply = loop {
        match control.offer_snapshot_confirmation(cut)? {
            Offer::Queued(reply) => break reply,
            Offer::OwnerQueueFull if Instant::now() < deadline => std::thread::yield_now(),
            _ => return Err(fail(920)),
        }
    };
    loop {
        match reply.poll()? {
            ReplyPoll::Ready(ControlDecision::Accepted) => return Ok(()),
            ReplyPoll::Pending if Instant::now() < deadline => std::thread::yield_now(),
            _ => return Err(fail(921)),
        }
    }
}
fn run(io: ProjectorIo<'_>, suffix: bool, report: &mut RebuildReport, acquired: impl FnOnce() -> u32) -> Result<(), ProjectorError> {
    let statevec_projector_api::StartInstruction::Rebuild { target } = io.start_instruction() else { return Err(fail(922)); };
    let ProjectorIoParts { mut source, control, .. } = io.split();
    let mut model = Model::new();
    let cut = {
        let Delivery::Snapshot(mut loan) = source.next_timeout(Duration::from_secs(20))? else { return Err(fail(923)); };
        let info = loan.info();
        if info.position.raft_index != target { return Err(fail(924)); }
        // Physical test rendezvous: hold the actual SDK borrow while the driver
        // persists a suffix. It supplies no record, cut or owner decision.
        if acquired() != 0 { return Err(fail(938)); }
        report.baseline = target;
        // Local premature finish must not reach the host or consume the borrow.
        if !matches!(loan.finish(), Err(ProjectorError::SnapshotRead(ProjectorSnapshotReadError::Incomplete))) { return Err(fail(925)); }
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if Instant::now() >= deadline { return Err(fail(926)); }
            match loan.next_record()? {
                SnapshotRead::Record(record) => {
                    if model.insert((record.key.kind, record.key.sys_id), (record.revision, record.bytes.to_vec())).is_some() { return Err(fail(927)); }
                }
                SnapshotRead::Progress => (),
                SnapshotRead::End => break,
                SnapshotRead::Stop => return Err(fail(928)),
            }
        }
        if model.len() as u64 != info.record_count { return Err(fail(929)); }
        report.records = info.record_count;
        report.baseline_digest = digest(&model);
        loan.finish()?
    };
    // Only the complete, installed in-memory model precedes this ACK. It is not
    // a persisted history checkpoint and does not issue report_durable_offset.
    accept(&control, &cut)?;
    if suffix {
        let Delivery::RaftEntry(loan) = source.next_timeout(Duration::from_secs(20))? else { return Err(fail(930)); };
        if loan.token().offset().is_some() { return Err(fail(931)); }
        let view = loan.view();
        report.suffix_index = view.position().raft_index;
        if report.suffix_index != target + 1 { return Err(fail(932)); }
        for i in 0..view.tx_count() {
            let tx = view.tx(i)?;
            for j in 0..tx.record_delta_count() {
                match tx.record_delta(j)? {
                    RecordDeltaView::Insert { key, revision, after } => {
                        if model.insert((key.kind, key.sys_id), (revision, after.to_vec())).is_some() { return Err(fail(933)); }
                    }
                    RecordDeltaView::Update { key, revision, before, after } => {
                        let Some(previous) = model.get_mut(&(key.kind, key.sys_id)) else { return Err(fail(934)); };
                        if previous.1 != before { return Err(fail(935)); }
                        *previous = (revision, after.to_vec());
                    }
                    RecordDeltaView::Delete { key, before, .. } => {
                        let Some(previous) = model.remove(&(key.kind, key.sys_id)) else { return Err(fail(936)); };
                        if previous.1 != before { return Err(fail(937)); }
                    }
                }
            }
        }
    }
    // An exact repeated ACK remains accepted after the live suffix.
    accept(&control, &cut)?;
    report.model_records = model.len() as u64;
    report.suffix_digest = digest(&model);
    Ok(())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn statevec_projector_rebuild_probe_v2(
    table: *const SvProjectorHostV2, startup: *const SvProjectorStart, suffix: u32,
    acquired: unsafe extern "C" fn(*mut std::ffi::c_void) -> u32, context: *mut std::ffi::c_void,
    report: *mut RebuildReport, completion: *mut SvProjectorCompletion,
) -> SvStatus {
    if table.is_null() || startup.is_null() || report.is_null() || completion.is_null() { return (&ProjectorAbiError::NullPointer).into(); }
    // SAFETY: fixture lends aligned complete DTOs and the actual table through
    // this synchronous call; with_host catches author unwind and joins borrows.
    unsafe { report.write(RebuildReport::default()); completion.write(SvProjectorCompletion::EMPTY); }
    let mut result = RebuildReport::default();
    match unsafe { with_host(&*table, &*startup, |io| run(io, suffix != 0, &mut result, || acquired(context))) } {
        Ok(value) => { unsafe { report.write(result); completion.write(value); } SvStatus::SUCCESS }
        Err(cause) => (&cause).into(),
    }
}
