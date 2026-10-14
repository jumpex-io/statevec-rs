//! Linux qualification only. The real generated exporter owns every worker,
//! loan and service. This physical interposer forwards the actual native host
//! and substitutes only declared release/notification outcomes. It neither
//! constructs frames/cuts nor fabricates completion reports.
#![cfg(target_os = "linux")]
use statevec_projector_api::{plugin_v1::{ffi::*, *}, ProjectorRecovery};
use std::{ffi::c_void, io::{Read, Write}, num::NonZeroUsize,
    os::unix::net::UnixStream, path::PathBuf, sync::{Arc, Mutex, atomic::{AtomicBool, AtomicUsize, Ordering}}, time::{Duration, Instant}};

#[path = "lifecycle_probe_callbacks.rs"]
mod callbacks;
#[path = "lifecycle_probe_export.rs"]
mod export;

#[derive(Clone, Copy, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Case { FrameError, FrameRetained, ReplyError, ReplyRetained, ServiceFrame, HeldService }
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Config { case: Case, report: PathBuf, control: PathBuf, lease: PathBuf }
struct Harness {
    config: Config, report: Mutex<std::fs::File>, release_observed: AtomicBool,
    polls: AtomicUsize, allocations: AtomicUsize, releases: AtomicUsize,
    author_calls: AtomicUsize,
}
impl Harness {
    fn event(&self, line: impl std::fmt::Display) { writeln!(self.report.lock().unwrap(), "{line}").unwrap(); }
}
struct Module(Arc<Harness>);
struct Configured(Arc<Harness>);
struct Prepared { harness: Arc<Harness>, control: UnixStream }
impl ProjectorModule for Module {
    const RECOVERY_SUPPORT: ProjectorRecoverySupport = ProjectorRecoverySupport::ContinuousHistory;
    type Configured = Configured;
    fn configure(&self, _: ProjectorConfig<'_>) -> Result<Configured, ConfigureFailure> { Ok(Configured(self.0.clone())) }
}
impl ConfiguredProjector for Configured {
    type Prepared = Prepared;
    fn prepare(&self, _: ProjectorPrepare<'_>) -> Result<Prepared, ProjectorFailure> {
        let control = UnixStream::connect(&self.0.config.control).unwrap();
        control.set_read_timeout(Some(Duration::from_secs(8))).unwrap(); // harness safety bound
        Ok(Prepared { harness: self.0.clone(), control })
    }
}
impl PreparedProjector for Prepared {
    fn recovery(&self) -> ProjectorRecovery { ProjectorRecovery::Continuous { offset: None } }
    fn delivery_limits(&self) -> DeliveryLimits { DeliveryLimits {
        frame_capacity: NonZeroUsize::new(2).unwrap(), maximum_accounted_bytes: NonZeroUsize::new(1024 * 1024).unwrap(),
    } }
    fn run(self, io: ProjectorIo<'_>) -> Result<(), ProjectorFailure> {
        let Self { harness, control: mut channel } = self;
        harness.author_calls.fetch_add(1, Ordering::Relaxed);
        let ProjectorIoParts { mut source, control, services, stop } = io.split();
        match harness.config.case {
            Case::HeldService => {
                let held = harness.clone();
                services.spawn(move |_| {
                    held.author_calls.fetch_add(1, Ordering::Relaxed);
                    held.event("service-held");
                    let mut command = [0];
                    channel.read_exact(&mut command).unwrap();
                    match command[0] { b'F' => Err(ProjectorFailure::Application { code: 301 }), b'Q' => Ok(()), _ => panic!("held service failure") }
                })?;
                Ok(()) // SDK now really joins the still-held service.
            }
            Case::ReplyError | Case::ReplyRetained => {
                let mut held = Vec::new();
                let deadline = Instant::now() + Duration::from_secs(5);
                while held.len() != 4 {
                    match control.offer_status()? {
                        Offer::Queued(reply) => held.push(reply),
                        Offer::OwnerQueueFull if Instant::now() < deadline => std::thread::yield_now(),
                        _ => panic!("four real native replies must be acquired"),
                    }
                }
                harness.event("four-replies-held");
                let last = held.pop().unwrap();
                // Return with one real ReplyHandle Drop; SDK cleanup must still
                // attempt the other three, whose facades have been forgotten.
                for reply in held { std::mem::forget(reply); }
                let _last = last;
                Err(ProjectorFailure::Application { code: 301 })
            }
            Case::FrameError | Case::FrameRetained | Case::ServiceFrame => {
                let loan = loop {
                    match source.next_timeout(Duration::from_secs(5))? {
                        Delivery::Frame(loan) if loan.view().transaction_count() != 0 => break loan,
                        Delivery::Frame(_) => continue, // real Raft-only prefix, no business output
                        _ => panic!("test must deliver one actual committed frame"),
                    }
                };
                let position = loan.view().position();
                harness.event(format_args!("frame-held {} {}", position.raft_term, position.raft_index));
                if harness.config.case == Case::ServiceFrame {
                    let held = harness.clone();
                    services.spawn(move |_| {
                        held.author_calls.fetch_add(1, Ordering::Relaxed);
                        held.event("service-held");
                        let mut command = [0];
                        channel.read_exact(&mut command).unwrap();
                        panic!("scoped service while a real frame is held")
                    })?;
                    let deadline = Instant::now() + Duration::from_secs(5);
                    while !stop.is_stopping()? {
                        assert!(Instant::now() < deadline, "service panic must stop source");
                        std::thread::yield_now();
                    }
                    drop(loan);
                    Ok(())
                } else {
                    let _held_until_return = loan;
                    Err(ProjectorFailure::Application { code: 301 })
                }
            }
        }
    }
}
impl Drop for Configured {
    fn drop(&mut self) {
        use std::os::fd::AsRawFd;
        let lease = std::fs::OpenOptions::new().read(true).write(true).open(&self.0.config.lease).unwrap();
        assert_eq!(unsafe { libc::flock(lease.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) }, 0, "native must retire before configured destruction");
        self.0.event("configured-destroyed-after-native");
    }
}
