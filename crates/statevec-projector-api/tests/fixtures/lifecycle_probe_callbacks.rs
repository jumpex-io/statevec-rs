//! Per-worker physical callback interposer. All cookies, frames and replies
//! still come from the actual bridge. No registry or shared ambient fault state.
use super::*;
pub(super) struct Callbacks { pub original: SvProjectorHostV2, pub harness: Arc<Harness> }
impl Callbacks {
    pub fn table(&mut self) -> SvProjectorHostV2 { SvProjectorHostV2 {
        h: self.original.h, context: (self as *mut Self).cast(),
        receive: Some(receive), stop_receive: Some(stop_receive), frame_info: Some(frame_info),
        transaction: Some(transaction), event: Some(event), change: Some(change), release_frame: Some(release_frame),
        report_durable_offset: Some(report_durable_offset), offer_status: Some(offer_status), poll_reply: Some(poll_reply),
        release_reply: Some(release_reply), stopping: Some(stopping), notify: Some(notify),
        state_info: Some(state_info), state_next: Some(state_next), finish_state: Some(finish_state),
        release_state: Some(release_state), offer_rebuild: Some(offer_rebuild),
    } }
}
macro_rules! forward {
    ($name:ident($($arg:ident: $ty:ty),*)) => {
        unsafe extern "C" fn $name(raw: *mut c_void, $($arg: $ty),*) -> SvStatus {
            let c = unsafe { &*raw.cast::<Callbacks>() };
            unsafe { c.original.$name.unwrap()(c.original.context, $($arg),*) }
        }
    };
}
forward!(receive(wait: u64, tag: *mut u32, cookie: *mut u64));
forward!(stop_receive());
forward!(frame_info(cookie: u64, out: *mut SvFrameInfo));
forward!(transaction(cookie: u64, tx: u64, out: *mut SvTransactionInfo));
forward!(event(cookie: u64, tx: u64, index: u64, out: *mut SvEvent));
forward!(change(cookie: u64, tx: u64, index: u64, out: *mut SvRecordChange));
forward!(report_durable_offset(cut: *const SvDurableOffsetReport, tag: *mut u32, reply: *mut u64));
forward!(offer_status(tag: *mut u32, reply: *mut u64));
forward!(poll_reply(cookie: u64, out: *mut SvControlReply));
forward!(state_info(cookie: u64, out: *mut SvStateInfo));
forward!(state_next(cookie: u64, tag: *mut u32, out: *mut SvStateRecord));
forward!(finish_state(cookie: *mut u64, out: *mut SvRebuildCut));
unsafe extern "C" fn release_state(raw: *mut c_void, cookie: *mut u64) -> SvStatus {
    let c = unsafe { &*raw.cast::<Callbacks>() };
    c.harness.releases.fetch_add(1, Ordering::Relaxed);
    if c.harness.config.case == Case::StateRetained { return (&ProjectorFailure::HostPanicked).into(); }
    let result = unsafe { c.original.release_state.unwrap()(c.original.context, cookie) };
    if c.harness.config.case == Case::StateError && unsafe { *cookie == 0 }
        && result == SvStatus::from(&ProjectorFailure::RebuildSource(RebuildSourceFailure::RebuildAbandoned)) {
        (&ProjectorFailure::HostPanicked).into()
    } else { result }
}
forward!(offer_rebuild(cut: *const SvRebuildCut, tag: *mut u32, reply: *mut u64));
unsafe extern "C" fn stopping(raw: *const c_void, out: *mut u32) -> SvStatus {
    let c = unsafe { &*raw.cast::<Callbacks>() };
    unsafe { c.original.stopping.unwrap()(c.original.context, out) }
}
unsafe extern "C" fn release_frame(raw: *mut c_void, cookie: *mut u64) -> SvStatus {
    let c = unsafe { &*raw.cast::<Callbacks>() };
    let mut info = SvFrameInfo::default();
    let read = unsafe { c.original.frame_info.unwrap()(c.original.context, *cookie, &mut info) };
    if !read.is_success() { return read; }
    if info.transaction_count == 0 {
        return unsafe { c.original.release_frame.unwrap()(c.original.context, cookie) };
    }
    c.harness.releases.fetch_add(1, Ordering::Relaxed);
    if c.harness.config.case == Case::FrameRetained { return (&ProjectorFailure::HostPanicked).into(); }
    let result = unsafe { c.original.release_frame.unwrap()(c.original.context, cookie) };
    if result.is_success() && c.harness.config.case == Case::FrameError { (&ProjectorFailure::HostPanicked).into() } else { result }
}
unsafe extern "C" fn release_reply(raw: *mut c_void, cookie: *mut u64) -> SvStatus {
    let c = unsafe { &*raw.cast::<Callbacks>() };
    let index = c.harness.releases.fetch_add(1, Ordering::Relaxed);
    if index == 0 && c.harness.config.case == Case::ReplyRetained { return (&ProjectorFailure::HostPanicked).into(); }
    let result = unsafe { c.original.release_reply.unwrap()(c.original.context, cookie) };
    if !result.is_success() { return result; }
    if index == 0 { (&ProjectorFailure::HostPanicked).into() }
    else { (&ProjectorAbiFailure::InvalidHandle).into() } // must not overwrite first B
}
unsafe extern "C" fn notify(raw: *mut c_void, kind: u32, out: *const SvProjectorFailureObservation) -> SvStatus {
    let c = unsafe { &*raw.cast::<Callbacks>() };
    // Deliberately lose all wake notifications in the held-service row. The
    // real Node pump must still discover failure via its regular SDK poll.
    if c.harness.config.case == Case::HeldService { return SvStatus::SUCCESS; }
    let result = unsafe { c.original.notify.unwrap()(c.original.context, kind, out) };
    if kind == 1 && matches!(unsafe { (*out).slot }, 2 | 3 | 4) {
        // Hold the *real* callback after the native observation, before user
        // run can return A. An actual Node poll must first observe release B.
        let deadline = Instant::now() + Duration::from_secs(5);
        while !c.harness.release_observed.load(Ordering::Acquire) {
            if Instant::now() >= deadline { return (&ProjectorFailure::HostPanicked).into(); }
            std::thread::yield_now();
        }
    }
    result
}
