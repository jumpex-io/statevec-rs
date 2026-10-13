//! Loaded transport conformance probe, not a configured product exporter/store.
//! Consumer code below uses only safe public lending/control facades. The one
//! unsafe entry adapts the caller's explicitly borrowed C table for this run.
use statevec_projector_api::plugin_v1::{self as p, ffi::*, *};
use sha2::{Digest, Sha256};

#[repr(C)]
pub struct Report {
    pub frames: u64, pub empty: u64, pub replay: u64, pub transactions: u64,
    pub events: u64, pub changes: u64, pub last_index: u64, pub last_term: u64,
    pub last_tx_seq: u64, pub chain: [u8; 32], pub digest: [u8; 32],
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn statevec_projector_delivery_probe_v1(
    table: *const SvProjectorHostV1, count: u64, report: *mut Report, completion: *mut SvProjectorCompletion,
) -> SvStatus {
    if table.is_null() || report.is_null() || completion.is_null() { return (&ProjectorAbiFailure::NullPointer).into(); }
    // SAFETY: test caller lends these complete disjoint outputs and live table
    // through synchronous completion; no pointer survives this entry.
    let mut result: Report = unsafe { std::mem::zeroed() };
    let run = unsafe { p::with_host(&*table, "bank-history", |io| {
        let ProjectorIoParts { mut source, control, services, stop: _ } = io.split();
        // An actual scoped borrower is joined before its context can retire.
        services.spawn(|stop| {
            while !stop.is_stopping()? { std::thread::park_timeout(std::time::Duration::from_micros(200)); }
            Ok(())
        })?;
        let mut hash = Sha256::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut latest = None;
        while result.frames < count {
            if std::time::Instant::now() >= deadline { return Err(ProjectorFailure::Application { code: 900 }); }
            match source.next_timeout(std::time::Duration::from_millis(2))? {
                Delivery::Frame(loan) => {
                    let view = loan.view();
                    result.frames += 1;
                    result.empty += u64::from(view.transaction_count() == 0);
                    result.replay += u64::from(view.context() == statevec_projector_api::ProjectionContext::Replay);
                    let position = view.position();
                    result.last_index = position.raft_index; result.last_term = position.raft_term;
                    result.last_tx_seq = position.frontier.applied_tx_seq; result.chain = position.frontier.tx_chain_hash;
                    for index in 0..view.transaction_count() {
                        let tx = view.transaction(index)?;
                        let r = tx.result();
                        result.transactions += 1;
                        hash.update(r.tx_seq.to_le_bytes()); hash.update(r.ordinal.to_le_bytes());
                        hash.update(r.tx_id.bytes()); hash.update(r.tx_chain_hash.bytes());
                        hash.update([r.result_digest_profile]); hash.update(r.result_digest);
                        hash.update(r.sys_result_code.to_le_bytes()); hash.update(r.biz_code.to_le_bytes());
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
                        for index in 0..tx.change_count() {
                            result.changes += 1;
                            let (tag, key, revision, after) = match tx.change(index)? {
                                RecordChangeView::Insert { key, revision, after } => (1u8, key, revision, after),
                                RecordChangeView::Update { key, revision, after } => (2, key, revision, after),
                                RecordChangeView::Delete { key, revision } => (3, key, revision, &[][..]),
                            };
                            hash.update([tag]); hash.update(key.kind.to_le_bytes()); hash.update(key.sys_id.to_le_bytes());
                            hash.update(revision.to_le_bytes()); hash.update((after.len() as u64).to_le_bytes()); hash.update(after);
                        }
                    }
                    latest = Some(loan.checkpoint_cut());
                }
                Delivery::Empty | Delivery::TimedOut => {},
                Delivery::Stop | Delivery::Disconnected => return Err(ProjectorFailure::Application { code: 901 }),
            }
        }
        result.digest = hash.finalize().into();
        // This transport probe asserts an in-memory fixture prefix only. Durable
        // business-store qualification belongs to Durability and idle progress.
        let cut = latest.ok_or(ProjectorFailure::Application { code: 902 })?;
        cut.offset().encode_json().map_err(|_| ProjectorFailure::Application { code: 903 })?;
        let publication = cut.checkpointed();
        let mut reply = loop {
            match control.offer_checkpoint(&publication)? {
                Offer::Queued(reply) => break reply,
                Offer::OwnerQueueFull => { if std::time::Instant::now() >= deadline { return Err(ProjectorFailure::Application { code: 904 }); } std::thread::yield_now(); },
                _ => return Err(ProjectorFailure::Application { code: 905 }),
            }
        };
        loop {
            match reply.poll()? {
                ReplyPoll::Ready(Ok(())) => break,
                ReplyPoll::Pending if std::time::Instant::now() < deadline => std::thread::yield_now(),
                _ => return Err(ProjectorFailure::Application { code: 906 }),
            }
        }
        Ok(())
    }) };
    match run {
        Ok(value) => { unsafe { report.write(result); completion.write(value); } SvStatus::SUCCESS }
        Err(cause) => (&cause).into(),
    }
}
