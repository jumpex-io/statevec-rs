//! Qualification-only C-entry observations, before SDK/business dispatch.
//! Counters never change a returned status, accepted command or recovery choice.
use super::*;
use std::sync::Mutex;

struct EntryCounts {
    address: usize,
    live: bool,
    unusable: bool,
    calls: [u64; 4], // preflight, run, rules, entries after unusability
}
static INSTANCES: Mutex<Vec<EntryCounts>> = Mutex::new(Vec::new());
// A single loaded fixture in a bounded tape creates only a handful of instances.
const MAX_INSTANCES: usize = 1024;

#[unsafe(no_mangle)]
pub extern "C" fn core_fixture_last_instance() -> u64 {
    INSTANCES.lock().unwrap().len() as u64
}
#[unsafe(no_mangle)]
pub extern "C" fn core_fixture_instance_calls(instance: u64, slot: u32) -> u64 {
    let instances = INSTANCES.lock().unwrap();
    instance.checked_sub(1).and_then(|i| usize::try_from(i).ok())
        .and_then(|i| instances.get(i)).and_then(|row| row.calls.get(slot as usize))
        .copied().unwrap_or(u64::MAX)
}
fn current(instances: &mut [EntryCounts], instance: *const c_void) -> &mut EntryCounts {
    instances.iter_mut().rev().find(|row| row.live && row.address == instance.addr())
        .expect("C entry belongs to an actually created live fixture instance")
}
pub(super) fn entered(instance: *const c_void, slot: usize) {
    let mut instances = INSTANCES.lock().unwrap();
    let row = current(&mut instances, instance);
    row.calls[slot] += 1;
    row.calls[3] += u64::from(row.unusable);
}
pub(super) fn returned(instance: *const c_void, operation: CoreOperation, status: SvStatus) {
    if matches!(status.tag, 7..=9) || status.decode_for(operation).is_err() {
        current(&mut INSTANCES.lock().unwrap(), instance).unusable = true;
    }
}

pub(super) unsafe extern "C" fn create(configured: *const c_void, out: *mut *mut c_void) -> SvStatus {
    // SAFETY: forward the exact host-owned configured handle and output slot.
    let status = unsafe { (tables().original.create_instance.unwrap())(configured, out) };
    if status.is_success() {
        // SAFETY: successful SDK construction publishes an owned non-null handle.
        let instance = unsafe { *out };
        assert!(!instance.is_null());
        let mut instances = INSTANCES.lock().unwrap();
        assert!(instances.len() < MAX_INSTANCES, "fixture observation budget");
        instances.push(EntryCounts { address: instance.addr(), live: true, unusable: false, calls: [0; 4] });
    }
    status
}
pub(super) unsafe extern "C" fn destroy(instance: *mut *mut c_void) -> SvStatus {
    // SAFETY: actual owned handle slot; the SDK retains its normal destruction.
    let address = unsafe { *instance };
    let status = unsafe { (tables().original.destroy_instance.unwrap())(instance) };
    if unsafe { (*instance).is_null() } {
        current(&mut INSTANCES.lock().unwrap(), address).live = false;
    }
    status
}
pub(super) unsafe extern "C" fn preflight(
    instance: *const c_void, commands: *const SvCommand, count: u64, refused: *mut u64,
) -> SvStatus {
    entered(instance, 0);
    // SAFETY: exact synchronous host arguments forwarded unchanged.
    let status = unsafe { (tables().original.preflight_batch.unwrap())(instance, commands, count, refused) };
    returned(instance, CoreOperation::PreflightBatch, status);
    status
}
pub(super) unsafe extern "C" fn rules(instance: *const c_void, candidate: *const SvCandidateV3) -> SvStatus {
    entered(instance, 2);
    // SAFETY: exact synchronous host arguments forwarded unchanged.
    let status = unsafe { (tables().original.validate_tx_invariants.unwrap())(instance, candidate) };
    returned(instance, CoreOperation::ValidateTxInvariants, status);
    status
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reused_address_names_only_the_current_physical_incarnation() {
        let address = 17;
        let mut rows = vec![
            EntryCounts { address, live: false, unusable: true, calls: [1, 2, 3, 0] },
            EntryCounts { address, live: true, unusable: false, calls: [0; 4] },
        ];
        current(&mut rows, std::ptr::without_provenance(address)).calls[0] += 1;
        assert_eq!(rows[0].calls, [1, 2, 3, 0]);
        assert_eq!(rows[1].calls, [1, 0, 0, 0]);
    }
}
