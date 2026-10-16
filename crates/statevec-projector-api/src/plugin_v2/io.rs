use super::completion::FailureSlot;
use super::{borrow::*, ffi::*, ProjectorAbiFailure as A, ProjectorFailure as F, ProjectorOperation as Op};
use std::{cell::Cell, marker::PhantomData, rc::Rc, sync::Mutex};

#[derive(Clone, Copy, Default)]
pub(super) struct Held { pub cookie: u64, pub release: ReleaseDisposition }
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum ReleaseDisposition {
    #[default]
    Available,
    Attempted,
    /// Malformed transfer: no destructor has been attempted or authorized.
    UntrustedTransfer,
}
#[derive(Clone, Copy, Default)]
pub(super) enum ReplySlot {
    #[default]
    Free,
    Offering,
    Held(Held),
}
pub(super) struct ScopeData {
    pub table: SvProjectorHostV2,
    pub projector_id: crate::ProjectorId,
    pub frame: Mutex<Held>,
    pub state: Mutex<Held>,
    pub abandoned: std::sync::atomic::AtomicBool,
    pub replies: Mutex<[ReplySlot; 4]>,
    completion: Mutex<SvProjectorCompletion>,
    observation: super::observation::WorkerObservation,
    pub startup: super::ProjectorStartupBase,
    pub initialization: crate::ProjectorInitialization,
}
// SAFETY: a validated host promises concurrent control/stop/notify calls.
// Source facades/loans are !Send + !Sync; all remaining shared state is locked.
unsafe impl Send for ScopeData {}
unsafe impl Sync for ScopeData {}

impl ScopeData {
    /// Latch callback/DTO failures, stop acquisition and notify the host. This
    /// is not a pure validator. Cleanup slots cannot preempt a later run error.
    pub fn latch_contract_failure<T>(&self, result: Result<T, F>) -> Result<T, F> {
        if let Err(failure @ (F::HostPanicked | F::AbiContract(_))) = &result {
            self.record(FailureSlot::Run, *failure);
        }
        result
    }
    pub fn acquisition_allowed(&self) -> Result<(), F> {
        let completion = *self.completion.lock().unwrap_or_else(|e| e.into_inner());
        for status in [completion.run_failure, completion.frame_release_failure,
            completion.state_release_failure, completion.reply_release_failure] {
            if let Some(super::ProjectorCallFailure::Operation(cause)) = super::decode_failure(status)? {
                return Err(cause);
            }
        }
        Ok(())
    }
    pub fn record(&self, slot: FailureSlot, failure: F) {
        let status = SvStatus::from(&failure);
        let first = {
            let mut completion = self.completion.lock().unwrap_or_else(|e| e.into_inner());
            let place = match slot {
                FailureSlot::Run => &mut completion.run_failure, FailureSlot::FrameRelease => &mut completion.frame_release_failure,
                FailureSlot::StateRelease => &mut completion.state_release_failure, FailureSlot::ReplyRelease => &mut completion.reply_release_failure,
                FailureSlot::Notification => &mut completion.notification_failure, _ => unreachable!("fixed SDK failure slot"),
            };
            if *place == SvStatus::SUCCESS { *place = status; true } else { false }
        };
        if first { self.observation.publish(SvProjectorFailureObservation { slot: slot.code(), reserved: 0, cause: status }); }
        if first && matches!(slot, FailureSlot::Run | FailureSlot::FrameRelease | FailureSlot::StateRelease | FailureSlot::ReplyRelease) {
            // Revoke acquisition using the same physical stop latch. Preserve
            // this release kind's first cause if the consequent wake also fails.
            if let Err(failure) = self.stop() { self.record(slot, failure); }
        }
        if first && slot != FailureSlot::Notification {
            let observation = SvProjectorFailureObservation { slot: slot.code(), reserved: 0, cause: status };
            // SAFETY: checked table and live scope; wake only after stop_receive.
            if let Err(failure) = call(unsafe { self.table.notify.unwrap()(self.table.context, 1, &observation) }, Op::Notify) { self.record(FailureSlot::Notification, failure); }
        }
    }
    pub fn release_frame(&self) {
        let cookie = {
            let mut held = self.frame.lock().unwrap_or_else(|e| e.into_inner());
            if held.cookie == 0 || held.release != ReleaseDisposition::Available { return; }
            held.release = ReleaseDisposition::Attempted; held.cookie
        };
        let mut returned = cookie;
        // SAFETY: this scope retains the exact source loan; only its worker
        // may release it and the local in/out cookie is disjoint writable data.
        let result = call(unsafe { self.table.release_frame.unwrap()(self.table.context, &mut returned) }, Op::ReleaseFrame);
        if returned == 0 { *self.frame.lock().unwrap_or_else(|e| e.into_inner()) = Held::default(); }
        if let Err(failure) = result { self.record(FailureSlot::FrameRelease, failure); }
        else if returned != 0 { self.record(FailureSlot::FrameRelease, A::InvalidHandle.into()); }
    }
    pub fn release_reply(&self, cookie: u64) {
        let index = {
            let mut held = self.replies.lock().unwrap_or_else(|e| e.into_inner());
            let Some(index) = held.iter().position(|item| matches!(item, ReplySlot::Held(item) if item.cookie == cookie && item.release == ReleaseDisposition::Available && cookie != 0)) else { return; };
            held[index] = ReplySlot::Held(Held { cookie, release: ReleaseDisposition::Attempted }); index
        };
        let mut returned = cookie;
        // SAFETY: one move-only reply owns this cookie, still covered by scope.
        let result = call(unsafe { self.table.release_reply.unwrap()(self.table.context, &mut returned) }, Op::ReleaseReply);
        if returned == 0 { self.replies.lock().unwrap_or_else(|e| e.into_inner())[index] = ReplySlot::Free; }
        if let Err(failure) = result { self.record(FailureSlot::ReplyRelease, failure); }
        else if returned != 0 { self.record(FailureSlot::ReplyRelease, A::InvalidHandle.into()); }
    }
    pub fn stop(&self) -> Result<(), F> {
        // SAFETY: concurrent stop is promised by the validated host table.
        call(unsafe { self.table.stop_receive.unwrap()(self.table.context) }, Op::StopReceive)
    }
}

trait SpawnServices<'io>: Sync {
    fn spawn(&'io self, task: Box<dyn FnOnce() + Send + 'io>) -> std::io::Result<()>;
}
impl<'scope, 'env> SpawnServices<'scope> for std::thread::Scope<'scope, 'env> {
    fn spawn(&'scope self, task: Box<dyn FnOnce() + Send + 'scope>) -> std::io::Result<()> {
        // Scope retains actual join custody even if the facade/handle is forgotten.
        std::thread::Builder::new().name("projector-service".into()).spawn_scoped(self, task).map(drop)
    }
}

pub struct ProjectorIo<'io> { host: &'io ScopeData, services: &'io dyn SpawnServices<'io> }
pub struct ProjectorIoParts<'io> {
    pub source: ProjectorSource<'io>, pub control: ProjectorControlPort<'io>,
    pub services: ProjectorServices<'io>, pub stop: ProjectorStop<'io>,
}
pub struct ProjectorSource<'io> { pub(super) host: &'io ScopeData, _local: PhantomData<Rc<()>> }
#[derive(Clone)]
/// ```compile_fail,E0521
/// use statevec_projector_api::plugin_v2::ProjectorIo;
/// fn escape(io: ProjectorIo<'_>) {
///     let control = io.split().control;
///     std::thread::spawn(move || { let _ = control.offer_status(); });
/// }
/// ```
pub struct ProjectorControlPort<'io> { pub(super) host: &'io ScopeData, pub(super) _borrow: PhantomData<&'io mut &'io ()> }
#[derive(Clone)]
pub struct ProjectorStop<'io> { host: &'io ScopeData, _borrow: PhantomData<&'io mut &'io ()> }
pub struct ProjectorServices<'io> { host: &'io ScopeData, services: &'io dyn SpawnServices<'io> }

impl<'io> ProjectorIo<'io> {
    pub fn startup_base(&self) -> super::ProjectorStartupBase { self.host.startup }
    pub fn initialization(&self) -> crate::ProjectorInitialization { self.host.initialization }
    pub fn split(self) -> ProjectorIoParts<'io> {
        ProjectorIoParts {
            source: ProjectorSource { host: self.host, _local: PhantomData },
            control: ProjectorControlPort { host: self.host, _borrow: PhantomData },
            services: ProjectorServices { host: self.host, services: self.services },
            stop: ProjectorStop { host: self.host, _borrow: PhantomData },
        }
    }
}
impl ProjectorStop<'_> {
    pub fn is_stopping(&self) -> Result<bool, F> {
        self.host.latch_contract_failure(self.read_stopping())
    }
    fn read_stopping(&self) -> Result<bool, F> {
        let mut flag = 0;
        // SAFETY: live scoped table and disjoint output; callback is thread-safe.
        call(unsafe { self.host.table.stopping.unwrap()(self.host.table.context, &mut flag) }, Op::Stopping)?;
        match flag { 0 => Ok(false), 1 => Ok(true), actual => Err(A::Flag { actual }.into()) }
    }
}
impl<'io> ProjectorServices<'io> {
    pub fn spawn<Func>(&self, service: Func) -> Result<(), F>
    where Func: FnOnce(ProjectorStop<'io>) -> Result<(), F> + Send + 'io {
        let host = self.host;
        self.services.spawn(Box::new(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| service(ProjectorStop { host, _borrow: PhantomData })));
            match result {
                Ok(Ok(())) => return,
                Ok(Err(failure)) => host.record(FailureSlot::Run, failure),
                Err(payload) => {
                    std::mem::forget(payload);
                    let mut completion = host.completion.lock().unwrap_or_else(|e| e.into_inner());
                    let first = completion.failed_service_joins == 0;
                    completion.failed_service_joins += 1;
                    completion.service_join_failure = SvStatus::from(&super::ProjectorCallFailure::ServiceJoinPanicked);
                    let observation = SvProjectorFailureObservation { slot: FailureSlot::ServiceJoin.code(), reserved: 0, cause: completion.service_join_failure };
                    drop(completion);
                    if first { host.observation.publish(observation); }
                    if let Err(failure) = host.stop() { host.record(FailureSlot::Run, failure); }
                    if first {
                        if let Err(cause) = call(unsafe { host.table.notify.unwrap()(host.table.context, 1, &observation) }, Op::Notify) { host.record(FailureSlot::Notification, cause); }
                    }
                }
            }
            if let Err(failure) = host.stop() { host.record(FailureSlot::Run, failure); }
        })).map_err(|source| F::Io { stage: super::ProjectorIoStage::ServiceSpawn, kind: super::ProjectorIoKind::Other, os_code: source.raw_os_error() })
    }
}

/// Synchronous entry shared by the generated worker exporter and loaded delivery
/// fixtures. It owns all host borrows outside author code and joins every scoped
/// service before releasing forgotten loans/replies. It does not configure or
/// start a product node. Native ownership alone selects and admits recovery.
///
/// # Safety
/// The table/context and every callback must honor V1, remain valid through this
/// call, and supply valid borrowed payloads until release. No other source worker
/// may use this context. `startup` must be the actual admitted Startup view.
pub unsafe fn with_host(
    table: &SvProjectorHostV2, startup: &SvProjectorStart,
    run: impl for<'io> FnOnce(ProjectorIo<'io>) -> Result<(), F>,
) -> Result<SvProjectorCompletion, A> {
    let host = ScopeData::new(table, startup)?;
    Ok(host.run(run))
}

impl ScopeData {
pub(super) fn new(table: &SvProjectorHostV2, raw: &SvProjectorStart) -> Result<Self, A> {
    validate_host(table)?;
    let (startup, initialization) = super::inputs::startup(raw)?;
    let required = PROJECTOR_BASE_CAPABILITIES | match initialization {
        crate::ProjectorInitialization::Frames => CONTINUOUS_HISTORY_CAPABILITY,
        crate::ProjectorInitialization::Rebuild { .. } => LATEST_STATE_CAPABILITY,
    };
    if table.h.required_caps != required || table.h.provided_caps & required != required {
        return Err(A::UnsupportedCapabilities { required });
    }
    let projector_id = crate::ProjectorId::new(raw.projector_id.decode()?).map_err(|_| A::InvalidHandle)?;
    Ok(Self { table: *table, projector_id, frame: Mutex::new(Held::default()), state: Mutex::new(Held::default()), abandoned: std::sync::atomic::AtomicBool::new(false), replies: Mutex::new([ReplySlot::Free; 4]), completion: Mutex::new(SvProjectorCompletion::EMPTY), observation: super::observation::WorkerObservation::new(), startup, initialization })
}
pub(super) fn poll(&self) -> (bool, SvProjectorFailureObservation) { self.observation.read() }
pub(super) fn terminal(&self) -> SvProjectorCompletion { *self.completion.lock().unwrap_or_else(|e| e.into_inner()) }
pub(super) fn run(&self, run: impl for<'io> FnOnce(ProjectorIo<'io>) -> Result<(), F>) -> SvProjectorCompletion {
    let host = self;
    std::thread::scope(|services| {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(ProjectorIo { host: &host, services }))) {
            Ok(Ok(())) => {}, Ok(Err(failure)) => host.record(FailureSlot::Run, failure),
            Err(payload) => { std::mem::forget(payload); host.record(FailureSlot::Run, F::PluginPanicked); }
        }
        // Release a forgotten state loan before our internal stop can mask
        // abandonment. It cannot have a safe borrower after run returns.
        host.release_state();
        if let Err(failure) = host.stop() { host.record(FailureSlot::Run, failure); }
        // Scope joins even routines holding control/reply facades. It cannot be
        // bypassed by forgetting a public facade, and no new service is admitted.
    });
    host.release_frame();
    let cookies = host.replies.lock().unwrap_or_else(|e| e.into_inner()).map(|item| match item { ReplySlot::Held(item) => item.cookie, _ => 0 });
    for cookie in cookies { host.release_reply(cookie); }
    if host.abandoned.load(std::sync::atomic::Ordering::Acquire) {
        let result = host.terminal();
        if [result.run_failure, result.frame_release_failure, result.state_release_failure,
            result.reply_release_failure, result.service_join_failure, result.notification_failure,
            result.worker_join_failure].iter().all(|cause| *cause == SvStatus::SUCCESS) {
            host.record(FailureSlot::Run, F::RebuildSource(super::RebuildSourceFailure::RebuildAbandoned));
        }
    }
    // Publish the readiness hint before waking its observer. Actual join still
    // waits for this callback and reads the terminal result, including a failed
    // Finished notification; readiness alone never releases host dependencies.
    host.observation.finish();
    if let Err(cause) = call(unsafe { host.table.notify.unwrap()(host.table.context, 2, &SvProjectorFailureObservation::NONE) }, Op::Notify) { host.record(FailureSlot::Notification, cause); }
    let joined = host.frame.lock().unwrap_or_else(|e| e.into_inner()).cookie == 0
        && host.state.lock().unwrap_or_else(|e| e.into_inner()).cookie == 0
        && host.replies.lock().unwrap_or_else(|e| e.into_inner()).iter().all(|item| matches!(item, ReplySlot::Free));
    let mut completion = host.completion.lock().unwrap_or_else(|e| e.into_inner());
    completion.joined = u32::from(joined);
    let result = *completion;
    drop(completion);
    result
}
}

#[must_use]
pub enum Delivery<'a> {
    Frame(FrameLoan<'a>), State(super::StateLoan<'a>),
    /// Nonblocking receive found no item (try_next or a zero-duration wait).
    Empty,
    /// A positive bounded wait expired; neither Stop nor source disconnection.
    TimedOut,
    Stop, Disconnected,
}
#[must_use]
/// ```compile_fail,E0505
/// use statevec_projector_api::plugin_v2::FrameLoan;
/// fn escape(loan: FrameLoan<'_>) {
///     let view = loan.view();
///     drop(loan);
///     let _ = view.transaction_count();
/// }
/// ```
pub struct FrameLoan<'a> { pub(super) host: &'a ScopeData, pub(super) cookie: u64, pub(super) info: SvFrameInfo, _local: PhantomData<Rc<Cell<()>>> }
impl Drop for FrameLoan<'_> { fn drop(&mut self) { self.host.release_frame(); } }

impl ProjectorSource<'_> {
    pub fn try_next(&mut self) -> Result<Delivery<'_>, F> { self.next_timeout(std::time::Duration::ZERO) }
    pub fn next_timeout(&mut self, wait: std::time::Duration) -> Result<Delivery<'_>, F> {
        // Do not reclassify an already-recorded release failure as a run error.
        self.host.acquisition_allowed()?;
        // These are checked local author errors, not host callback evidence.
        // Handling them does not revoke the scope or manufacture a host fault.
        let nanos = u64::try_from(wait.as_nanos()).map_err(|_| A::Length { actual: u64::MAX, maximum: u64::MAX })?;
        if self.host.frame.lock().unwrap_or_else(|e| e.into_inner()).cookie != 0
            || self.host.state.lock().unwrap_or_else(|e| e.into_inner()).cookie != 0 { return Err(A::BorrowConflict.into()); }
        let host = self.host;
        host.latch_contract_failure(self.receive(nanos))
    }
    fn receive(&mut self, nanos: u64) -> Result<Delivery<'_>, F> {
        let (mut tag, mut cookie) = (0, 0);
        // SAFETY: the sole !Send source facade calls the live table; outputs are
        // disjoint and all returned bytes stay borrowed under the resulting loan.
        let result = call(unsafe { self.host.table.receive.unwrap()(self.host.table.context, nanos, &mut tag, &mut cookie) }, Op::Receive);
        if (result.is_err() || !matches!(tag, receive_tag::FRAME | receive_tag::STATE)) && cookie != 0 {
            let (held, slot) = if tag == receive_tag::STATE { (&self.host.state, FailureSlot::StateRelease) } else { (&self.host.frame, FailureSlot::FrameRelease) };
            *held.lock().unwrap_or_else(|e| e.into_inner()) = Held { cookie, release: ReleaseDisposition::UntrustedTransfer };
            self.host.record(slot, A::InvalidHandle.into());
            return Err(result.err().unwrap_or_else(|| A::InvalidHandle.into()));
        }
        result?;
        match tag {
            receive_tag::FRAME => {
                if cookie == 0 { return Err(A::InvalidHandle.into()); }
                *self.host.frame.lock().unwrap_or_else(|e| e.into_inner()) = Held { cookie, release: ReleaseDisposition::Available };
                let mut info = SvFrameInfo::default();
                let result = call(unsafe { self.host.table.frame_info.unwrap()(self.host.table.context, cookie, &mut info) }, Op::FrameInfo)
                    .and_then(|()| super::views::validate_frame(&info).map_err(Into::into));
                if let Err(cause) = result { self.host.release_frame(); return Err(cause); }
                Ok(Delivery::Frame(FrameLoan { host: self.host, cookie, info, _local: PhantomData }))
            }
            receive_tag::EMPTY => Ok(Delivery::Empty), receive_tag::DISCONNECTED => Ok(Delivery::Disconnected), receive_tag::STOP => Ok(Delivery::Stop),
            receive_tag::STATE => {
                if cookie == 0 { return Err(A::InvalidHandle.into()); }
                *self.host.state.lock().unwrap_or_else(|e| e.into_inner()) = Held { cookie, release: ReleaseDisposition::Available };
                match super::state::StateLoan::acquire(self.host, cookie) {
                    Ok(loan) => Ok(Delivery::State(loan)),
                    Err(cause) => { self.host.record(FailureSlot::Run, cause); self.host.release_state(); Err(cause) }
                }
            }
            receive_tag::TIMED_OUT if nanos != 0 => Ok(Delivery::TimedOut), actual => Err(A::Flag { actual }.into()),
        }
    }
}
