//! Test-only physical callback cuts around the unchanged public history author.
//! No frame, publication, persisted offset or native acceptance is manufactured.
use statevec_projector_api::{ProjectorCheckpointStatus, plugin_v2::{host::ControlReply, ffi::*}};
use std::{io::{Read, Write}, os::unix::net::UnixStream, path::{Path, PathBuf},
    sync::{Mutex, OnceLock, atomic::{AtomicBool, Ordering}}, time::{Duration, Instant}};

static SOCKET: OnceLock<PathBuf> = OnceLock::new();
pub fn configure(base: &Path) {
    assert!(SOCKET.set(base.join("history-probe.sock")).is_ok());
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Boundary { BeforeOffer, LostAccepted, FailSave }
pub struct Cut {
    boundary: Boundary,
    index: u64,
    frame: Mutex<Option<SvFrameInfo>>,
    channel: Mutex<UnixStream>,
    lost: AtomicBool,
    paused: AtomicBool,
}
impl Cut {
    pub fn connect() -> Option<Self> {
        let path = SOCKET.get().unwrap();
        if !path.try_exists().unwrap() { return None; }
        let mut channel = UnixStream::connect(path).unwrap();
        channel.set_read_timeout(Some(Duration::from_secs(8))).unwrap();
        channel.set_write_timeout(Some(Duration::from_secs(8))).unwrap();
        let mut command = [0; 9];
        channel.read_exact(&mut command).unwrap();
        let boundary = match command[0] {
            b'B' => Boundary::BeforeOffer,
            b'L' => Boundary::LostAccepted,
            b'F' => Boundary::FailSave,
            _ => panic!("unknown history cut"),
        };
        Some(Self { boundary, index: u64::from_le_bytes(command[1..].try_into().unwrap()),
            frame: Mutex::new(None), channel: Mutex::new(channel),
            lost: AtomicBool::new(false), paused: AtomicBool::new(false) })
    }
    pub fn frame(&self, host: &SvProjectorHostV2, info: SvFrameInfo) {
        *self.frame.lock().unwrap() = Some(info);
        if self.boundary == Boundary::FailSave && info.position.raft_index == self.index
            && !self.paused.swap(true, Ordering::SeqCst) {
            self.pause(host, "before-save");
            // This affects only the actual author thread, after prepare/open.
            // SQLite effects still commit; the file helper's real renameat
            // fails with EIO. No application result or native reply is faked.
            refuse_offset_replace();
            eprintln!("history-cut renameat EIO armed index={}", self.index);
        }
    }
    pub fn before_offer(&self, host: &SvProjectorHostV2, publication: SvDurableOffsetReport) {
        let frame = self.frame.lock().unwrap().expect("report follows real frame metadata");
        assert_eq!(publication.cut, frame.cut, "retry must keep the exact issued cut");
        if frame.position.raft_index != self.index { return; }
        eprintln!("history-cut report index={}", self.index);
        if self.boundary == Boundary::BeforeOffer || self.lost.load(Ordering::SeqCst) {
            if self.paused.swap(true, Ordering::SeqCst) { return; }
            // This query goes through the real native control queue. It does
            // not infer durability from either the report or the application DB.
            let label = if self.boundary == Boundary::BeforeOffer { "before-offer" } else { "lost-accepted-before-retry" };
            self.pause(host, label);
        }
    }
    fn pause(&self, host: &SvProjectorHostV2, label: &str) {
        let durable = durable_index(host);
        let mut channel = self.channel.lock().unwrap();
        writeln!(channel, "{}", serde_json::json!({"cut":label,"index":self.index,"native_durable":durable})).unwrap();
        let mut release = [0];
        channel.read_exact(&mut release).expect("parent releases or SIGKILLs held callback");
        assert_eq!(release, [b'G']);
    }
    pub fn lose_accepted(&self) -> bool {
        let frame = self.frame.lock().unwrap().expect("accepted report has a real frame");
        self.boundary == Boundary::LostAccepted && frame.position.raft_index == self.index
            && !self.lost.swap(true, Ordering::SeqCst)
    }
}
fn refuse_offset_replace() {
    let instruction = |code: u32, jt, jf, k| libc::sock_filter { code: code as u16, jt, jf, k };
    let arch = if cfg!(target_arch = "x86_64") { 0xc000003e } else if cfg!(target_arch = "aarch64") { 0xc00000b7 }
        else { panic!("unsupported qualification architecture") };
    let mut instructions = [
        instruction(libc::BPF_LD | libc::BPF_W | libc::BPF_ABS, 0, 0, 4),
        instruction(libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K, 1, 0, arch),
        instruction(libc::BPF_RET | libc::BPF_K, 0, 0, libc::SECCOMP_RET_KILL_PROCESS),
        instruction(libc::BPF_LD | libc::BPF_W | libc::BPF_ABS, 0, 0, 0),
        instruction(libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K, 0, 1, libc::SYS_renameat as u32),
        instruction(libc::BPF_RET | libc::BPF_K, 0, 0, libc::SECCOMP_RET_ERRNO | libc::EIO as u32),
        instruction(libc::BPF_RET | libc::BPF_K, 0, 0, libc::SECCOMP_RET_ALLOW),
    ];
    let program = libc::sock_fprog { len: instructions.len() as u16, filter: instructions.as_mut_ptr() };
    // SAFETY: the kernel copies the bounded filter; no TSYNC, no other thread
    // or process is changed. Reopen starts a fresh unfiltered child.
    assert_eq!(unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) }, 0);
    assert_eq!(unsafe { libc::prctl(libc::PR_SET_SECCOMP, libc::SECCOMP_MODE_FILTER, &program) }, 0);
}
pub(super) fn durable_index(host: &SvProjectorHostV2) -> u64 {
    let mut tag = 0;
    let mut reply = 0;
    assert_eq!(unsafe { host.offer_status.unwrap()(host.context, &mut tag, &mut reply) }, SvStatus::SUCCESS);
    assert_eq!(tag, 0);
    let deadline = Instant::now() + Duration::from_secs(4);
    let index = loop {
        let mut out = SvControlReply::default();
        assert_eq!(unsafe { host.poll_reply.unwrap()(host.context, reply, &mut out) }, SvStatus::SUCCESS);
        match out.decode().unwrap() {
            ControlReply::Pending => { assert!(Instant::now() < deadline); std::thread::yield_now(); }
            ControlReply::Status(ProjectorCheckpointStatus::Active { durable_raft_index, .. }) => break durable_raft_index,
            other => panic!("continuous attachment must be active: {other:?}"),
        }
    };
    assert_eq!(unsafe { host.release_reply.unwrap()(host.context, &mut reply) }, SvStatus::SUCCESS);
    assert_eq!(reply, 0);
    index
}
