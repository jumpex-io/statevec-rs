//! Read-only, one-shot projection of the worker's first published failure.
//! The terminal result and resource custody remain with the actual join handle.
use super::ffi::SvProjectorFailureObservation;
use std::{cell::UnsafeCell, sync::atomic::{AtomicBool, AtomicU8, Ordering}};

pub(super) struct WorkerObservation {
    state: AtomicU8,
    first: UnsafeCell<SvProjectorFailureObservation>,
    finished: AtomicBool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin_v1::{ProjectorFailure, SvStatus};
    #[test]
    fn incomplete_publication_never_spins_or_reports_a_partial_failure() {
        let observation = WorkerObservation::new();
        // The real one-shot projection's publication-in-progress state has no
        // readable record; this is not a simulation of an owner transition.
        observation.state.store(1, Ordering::Relaxed);
        assert_eq!(observation.read(), (false, SvProjectorFailureObservation::NONE));
    }
    #[test]
    fn concurrent_publishers_never_tear_or_replace_the_first_record() {
        let observation = WorkerObservation::new();
        std::thread::scope(|scope| {
            for code in 1..=8 {
                let observation = &observation;
                scope.spawn(move || observation.publish(SvProjectorFailureObservation {
                    slot: 1, reserved: 0, cause: SvStatus::from(&ProjectorFailure::Application { code }),
                }));
            }
        });
        let (_, first) = observation.read();
        assert_eq!((first.slot, first.reserved, first.cause.tag, first.cause.cause), (1, 0, 12, 1));
        assert!((1..=8).contains(&first.cause.detail[0]));
        assert_eq!(&first.cause.detail[1..], &[0, 0]);
        observation.publish(SvProjectorFailureObservation { slot: 1, reserved: 0, cause: (&ProjectorFailure::PluginPanicked).into() });
        observation.finish();
        assert_eq!(observation.read(), (true, first));
    }
}
// SAFETY: exactly one CAS winner writes first, then publishes state=2 with
// Release. All readers copy only after Acquire sees 2; no writer revisits it.
unsafe impl Sync for WorkerObservation {}
impl WorkerObservation {
    pub fn new() -> Self {
        Self { state: AtomicU8::new(0), first: UnsafeCell::new(SvProjectorFailureObservation::NONE), finished: AtomicBool::new(false) }
    }
    pub fn publish(&self, failure: SvProjectorFailureObservation) {
        if self.state.compare_exchange(0, 1, Ordering::Relaxed, Ordering::Relaxed).is_ok() {
            // SAFETY: the unique writer; an incomplete publication is not read.
            unsafe { self.first.get().write(failure); }
            self.state.store(2, Ordering::Release);
        }
    }
    pub fn finish(&self) { self.finished.store(true, Ordering::Release); }
    pub fn read(&self) -> (bool, SvProjectorFailureObservation) {
        let finished = self.finished.load(Ordering::Acquire);
        let first = if self.state.load(Ordering::Acquire) == 2 {
            // SAFETY: immutable record initialized by the sole release publisher.
            unsafe { *self.first.get() }
        } else { SvProjectorFailureObservation::NONE };
        (finished, first)
    }
}
