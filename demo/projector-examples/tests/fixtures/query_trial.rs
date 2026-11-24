//! Native Bank trial. Consume the live frames. From the imported population
//! onward, release every fourth frame before offering a real learner query.
use super::{bank_decoder::Account, failure};
use statevec::{GeneratedRecordAccess, RecordSchema};
use statevec_projector_api::plugin::*;
use std::io::{BufWriter, Write};
use std::time::{Duration, Instant};

fn nanos() -> Result<u64, ProjectorError> {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: ts is a live writable timespec for the synchronous Linux call.
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) } != 0 {
        return Err(ProjectorError::Application { code: 954 });
    }
    Ok(ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64)
}

pub(super) fn run(io: ProjectorIo<'_>, file: std::fs::File, after: u64) -> Result<(), ProjectorError> {
    let ProjectorIoParts { mut source, control, .. } = io.split();
    let mut log = BufWriter::new(file);
    let mut queries = 0;
    let bound = ffi::MAX_ENGINE_QUERY_KEYS as usize;
    let sweep = [1, 16, 256, 1024, bound];
    loop {
        let position = match source.next_timeout(Duration::from_secs(1))? {
            Delivery::RaftEntry(entry) => entry.view().position(),
            Delivery::Stop | Delivery::Disconnected => break,
            Delivery::Empty | Delivery::TimedOut => continue,
            Delivery::Snapshot(_) => return Err(ProjectorError::Application { code: 955 }),
        };
        let index = position.raft_index;
        writeln!(log, "frame {} {index}", nanos()?).map_err(failure)?;
        if position.frontier.applied_tx_seq < after || index % 4 != 0 { continue; }
        // Sweep key counts during warmup, then sustain the published maximum.
        let count = if queries < 40 { sweep[queries % sweep.len()] } else { bound };
        let base = ((queries * 131) as u64) % after.saturating_sub(bound as u64).max(1);
        let encoded: Vec<_> = (1..=count).map(|i| Account::unique_key(base+i as u64)).collect();
        let keys: Vec<_> = encoded.iter().map(|key| EngineQueryKey::UniqueKey {
            kind: Account::KIND, uk_id: 0, key: key.as_slice(),
        }).collect();
        queries += 1;
        let offered = nanos()?;
        let offered_query = match control.offer_engine_query(&keys)? {
            QueryOffer::Queued(handle) => Ok(handle),
            QueryOffer::Unsupported => Err("unsupported"),
            QueryOffer::OwnerQueueFull => Err("owner_queue_full"),
            QueryOffer::Closed => Err("closed"),
            QueryOffer::CapacityExhausted => Err("capacity_exhausted"),
        };
        let mut handle = match offered_query {
            Ok(handle) => handle,
            Err(reason) => {
                writeln!(log, "error {offered} {index} {count} offer {reason}").map_err(failure)?;
                continue;
            }
        };
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match handle.poll()? {
                EngineQueryPoll::Pending if Instant::now() < deadline => std::thread::sleep(Duration::from_micros(100)),
                EngineQueryPoll::Ready(answer) => {
                    let returned = nanos()?;
                    let at = answer.position().raft_index;
                    let mut bytes = 0;
                    let mut matches = 0;
                    for i in 0..answer.len() {
                        if let Some(r) = answer.record(i)? {
                            bytes += r.record.len();
                            matches += usize::from(r.kind == Account::KIND && r.record.len() == Account::DATA_LEN
                                && Account::wrap(r.record).account_id() == base+i as u64+1);
                        }
                    }
                    writeln!(log, "answer {offered} {returned} {index} {at} {count} {matches} {bytes}").map_err(failure)?;
                    break;
                }
                EngineQueryPoll::Rejected(reason) => {
                    writeln!(log, "error {offered} {index} {count} rejected {reason:?}").map_err(failure)?;
                    break;
                }
                EngineQueryPoll::Disconnected | EngineQueryPoll::Pending => {
                    writeln!(log, "error {offered} {index} {count} no_answer").map_err(failure)?;
                    break;
                }
            }
        }
    }
    log.flush().map_err(failure)
}
