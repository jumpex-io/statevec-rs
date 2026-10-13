use super::{borrow::*, ffi::*, ProjectorAbiFailure as A, ProjectorFailure as F, ProjectorOperation as Op};
use std::{cell::Cell, marker::PhantomData, rc::Rc, sync::Mutex};

#[derive(Clone, Copy, Default)]
pub(super) struct Held { pub cookie: u64, pub attempted: bool }
pub(super) struct ScopeData {
    pub table: SvProjectorHostV1,
    pub projector_id: crate::ProjectorId,
    pub frame: Mutex<Held>,
    pub replies: Mutex<[Held; 4]>,
    completion: Mutex<SvProjectorCompletion>,
    observation: super::observation::WorkerObservation,
    startup: super::ProjectorStartupBase,
    initialization: crate::ProjectorInitialization,
}
// SAFETY: a validated host promises concurrent control/stop/notify calls.
// Source facades/loans are !Send + !Sync; all remaining shared state is locked.
unsafe impl Send for ScopeData {}
unsafe impl Sync for ScopeData {}

impl ScopeData {
    /// Callback/DTO contract failures cannot be hidden by an author returning
    /// Ok. Cleanup has separate slots and must not preempt a later run error.
    pub fn checked<T>(&self, result: Result<T, F>) -> Result<T, F> {
        if let Err(failure @ (F::HostPanicked | F::AbiContract(_))) = &result {
            self.record(1, *failure);
        }
        result
    }
    pub fn acquisition_allowed(&self) -> Result<(), F> {
        let completion = *self.completion.lock().unwrap_or_else(|e| e.into_inner());
        for status in [completion.run_failure, completion.frame_release_failure,
            completion.state_release_failure, completion.reply_release_failure] {
            if let Some(super::ProjectorCallFailure::Failure(cause)) = super::decode_failure(status)? {
                return Err(cause);
            }
        }
        Ok(())
    }
    pub fn record(&self, slot: u32, failure: F) {
        let status = SvStatus::from(&failure);
        let first = {
            let mut completion = self.completion.lock().unwrap_or_else(|e| e.into_inner());
            let place = match slot {
                1 => &mut completion.run_failure, 2 => &mut completion.frame_release_failure,
                3 => &mut completion.state_release_failure, 4 => &mut completion.reply_release_failure,
                6 => &mut completion.notification_failure, _ => unreachable!("fixed SDK failure slot"),
            };
            if *place == SvStatus::SUCCESS { *place = status; true } else { false }
        };
        if first { self.observation.publish(SvProjectorFailureObservation { slot, reserved: 0, cause: status }); }
        if first && matches!(slot, 1..=4) {
            // Revoke acquisition using the same physical stop latch. Preserve
            // this release kind's first cause if the consequent wake also fails.
            if let Err(failure) = self.stop() { self.record(slot, failure); }
        }
        if first && slot != 6 {
            let observation = SvProjectorFailureObservation { slot, reserved: 0, cause: status };
            // SAFETY: checked table and live scope; wake only after stop_receive.
            if let Err(failure) = call(unsafe { self.table.notify.unwrap()(self.table.context, 1, &observation) }, Op::Notify) { self.record(6, failure); }
        }
    }
    pub fn release_frame(&self) {
        let cookie = {
            let mut held = self.frame.lock().unwrap_or_else(|e| e.into_inner());
            if held.cookie == 0 || held.attempted { return; }
            held.attempted = true; held.cookie
        };
        let mut returned = cookie;
        // SAFETY: this scope retains the exact source loan; only its worker
        // may release it and the local in/out cookie is disjoint writable data.
        let result = call(unsafe { self.table.release_frame.unwrap()(self.table.context, &mut returned) }, Op::ReleaseFrame);
        if returned == 0 { *self.frame.lock().unwrap_or_else(|e| e.into_inner()) = Held::default(); }
        if let Err(failure) = result { self.record(2, failure); }
        else if returned != 0 { self.record(2, A::InvalidHandle.into()); }
    }
    pub fn release_reply(&self, cookie: u64) {
        let index = {
            let mut held = self.replies.lock().unwrap_or_else(|e| e.into_inner());
            let Some(index) = held.iter().position(|item| item.cookie == cookie && !item.attempted && cookie != 0) else { return; };
            held[index].attempted = true; index
        };
        let mut returned = cookie;
        // SAFETY: one move-only reply owns this cookie, still covered by scope.
        let result = call(unsafe { self.table.release_reply.unwrap()(self.table.context, &mut returned) }, Op::ReleaseReply);
        if returned == 0 { self.replies.lock().unwrap_or_else(|e| e.into_inner())[index] = Held::default(); }
        if let Err(failure) = result { self.record(4, failure); }
        else if returned != 0 { self.record(4, A::InvalidHandle.into()); }
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
/// use statevec_projector_api::plugin_v1::ProjectorIo;
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
        self.host.checked(self.read_stopping())
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
                Ok(Err(failure)) => host.record(1, failure),
                Err(payload) => {
                    std::mem::forget(payload);
                    let mut completion = host.completion.lock().unwrap_or_else(|e| e.into_inner());
                    let first = completion.failed_service_joins == 0;
                    completion.failed_service_joins += 1;
                    completion.service_join_failure = SvStatus::from(&super::ProjectorCallFailure::ServiceJoinPanicked);
                    let observation = SvProjectorFailureObservation { slot: 5, reserved: 0, cause: completion.service_join_failure };
                    drop(completion);
                    if first { host.observation.publish(observation); }
                    if let Err(failure) = host.stop() { host.record(1, failure); }
                    if first {
                        if let Err(cause) = call(unsafe { host.table.notify.unwrap()(host.table.context, 1, &observation) }, Op::Notify) { host.record(6, cause); }
                    }
                }
            }
            if let Err(failure) = host.stop() { host.record(1, failure); }
        })).map_err(|source| F::Io { stage: super::ProjectorIoStage::ServiceSpawn, kind: super::ProjectorIoKind::Other, os_code: source.raw_os_error() })
    }
}

/// Synchronous entry shared by the eventual worker exporter and loaded delivery
/// fixtures. It owns all host borrows outside author code and joins every scoped
/// service before releasing forgotten loans/replies. It does not configure or
/// start a product node and does not implement latest-state delivery yet.
///
/// # Safety
/// The table/context and every callback must honor V1, remain valid through this
/// call, and supply valid borrowed payloads until release. No other source worker
/// may use this context. `startup` must be the actual admitted Startup view.
pub unsafe fn with_host(
    table: &SvProjectorHostV1, startup: &SvProjectorStart,
    run: impl for<'io> FnOnce(ProjectorIo<'io>) -> Result<(), F>,
) -> Result<SvProjectorCompletion, A> {
    let host = ScopeData::new(table, startup)?;
    Ok(host.run(run))
}

impl ScopeData {
pub(super) fn new(table: &SvProjectorHostV1, raw: &SvProjectorStart) -> Result<Self, A> {
    validate_host(table)?;
    let (startup, initialization) = super::inputs::startup(raw)?;
    // The delivery phase activates continuous history only. Native latest-state
    // admission/state lending belongs to the reviewed later rebuild phase.
    if initialization != crate::ProjectorInitialization::Frames {
        return Err(A::UnsupportedCapabilities { required: LATEST_STATE_CAPABILITY });
    }
    let projector_id = crate::ProjectorId::new(raw.projector_id.decode()?).map_err(|_| A::InvalidHandle)?;
    Ok(Self { table: *table, projector_id, frame: Mutex::new(Held::default()), replies: Mutex::new([Held::default(); 4]), completion: Mutex::new(SvProjectorCompletion::EMPTY), observation: super::observation::WorkerObservation::new(), startup, initialization })
}
pub(super) fn poll(&self) -> (bool, SvProjectorFailureObservation) { self.observation.read() }
pub(super) fn terminal(&self) -> SvProjectorCompletion { *self.completion.lock().unwrap_or_else(|e| e.into_inner()) }
pub(super) fn run(&self, run: impl for<'io> FnOnce(ProjectorIo<'io>) -> Result<(), F>) -> SvProjectorCompletion {
    let host = self;
    std::thread::scope(|services| {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(ProjectorIo { host: &host, services }))) {
            Ok(Ok(())) => {}, Ok(Err(failure)) => host.record(1, failure),
            Err(payload) => { std::mem::forget(payload); host.record(1, F::PluginPanicked); }
        }
        if let Err(failure) = host.stop() { host.record(1, failure); }
        // Scope joins even routines holding control/reply facades. It cannot be
        // bypassed by forgetting a public facade, and no new service is admitted.
    });
    host.release_frame();
    let cookies = host.replies.lock().unwrap_or_else(|e| e.into_inner()).map(|item| item.cookie);
    for cookie in cookies { host.release_reply(cookie); }
    if let Err(cause) = call(unsafe { host.table.notify.unwrap()(host.table.context, 2, &SvProjectorFailureObservation::NONE) }, Op::Notify) { host.record(6, cause); }
    let joined = host.frame.lock().unwrap_or_else(|e| e.into_inner()).cookie == 0
        && host.replies.lock().unwrap_or_else(|e| e.into_inner()).iter().all(|item| item.cookie == 0);
    let mut completion = host.completion.lock().unwrap_or_else(|e| e.into_inner());
    completion.joined = u32::from(joined);
    let result = *completion;
    drop(completion);
    host.observation.finish();
    result
}
}

#[must_use]
pub enum Delivery<'a> { Frame(FrameLoan<'a>), Empty, TimedOut, Stop, Disconnected }
#[must_use]
/// ```compile_fail,E0505
/// use statevec_projector_api::plugin_v1::FrameLoan;
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
        let host = self.host;
        host.checked(self.receive(wait))
    }
    fn receive(&mut self, wait: std::time::Duration) -> Result<Delivery<'_>, F> {
        let nanos = u64::try_from(wait.as_nanos()).map_err(|_| A::Length { actual: u64::MAX, maximum: u64::MAX })?;
        if self.host.frame.lock().unwrap_or_else(|e| e.into_inner()).cookie != 0 { return Err(A::BorrowConflict.into()); }
        let (mut tag, mut cookie) = (0, 0);
        // SAFETY: the sole !Send source facade calls the live table; outputs are
        // disjoint and all returned bytes stay borrowed under the resulting loan.
        let result = call(unsafe { self.host.table.receive.unwrap()(self.host.table.context, nanos, &mut tag, &mut cookie) }, Op::Receive);
        if (result.is_err() || tag != 0) && cookie != 0 {
            *self.host.frame.lock().unwrap_or_else(|e| e.into_inner()) = Held { cookie, attempted: true };
            self.host.record(2, A::InvalidHandle.into());
            return Err(result.err().unwrap_or_else(|| A::InvalidHandle.into()));
        }
        result?;
        match tag {
            0 => {
                if cookie == 0 { return Err(A::InvalidHandle.into()); }
                *self.host.frame.lock().unwrap_or_else(|e| e.into_inner()) = Held { cookie, attempted: false };
                let mut info = SvFrameInfo::default();
                let result = call(unsafe { self.host.table.frame_info.unwrap()(self.host.table.context, cookie, &mut info) }, Op::FrameInfo)
                    .and_then(|()| super::views::validate_frame(&info).map_err(Into::into));
                if let Err(cause) = result { self.host.release_frame(); return Err(cause); }
                Ok(Delivery::Frame(FrameLoan { host: self.host, cookie, info, _local: PhantomData }))
            }
            1 => Ok(Delivery::Empty), 2 => Ok(Delivery::Disconnected), 3 => Ok(Delivery::Stop),
            4 if nanos != 0 => Ok(Delivery::TimedOut), actual => Err(A::Flag { actual }.into()),
        }
    }
}
