use super::*;
use statevec::api::plugin_v3::{__export, ffi::*};
use std::ffi::c_void;
use std::sync::OnceLock;

#[cfg(panic = "abort")]
compile_error!("StateVec Core plugins require panic = \"unwind\"");

unsafe extern "C" fn configure(config: SvBytes, out: *mut *mut c_void) -> SvStatus {
    // SAFETY: exact host input and handle output forwarded to the SDK.
    unsafe { __export::configure(&Module, config, out) }
}
static ORIGINAL: __export::CoreExportTable = __export::CoreExportTable::new::<Module>(configure);
struct Tables { original: SvCoreV3, exposed: SvCoreV3 }
// SAFETY: immutable copied SDK tables contain only static target bytes and
// functions. No runtime, configured handle or borrowed callback is stored here.
unsafe impl Send for Tables {}
unsafe impl Sync for Tables {}
fn tables() -> &'static Tables {
    static TABLES: OnceLock<Tables> = OnceLock::new();
    TABLES.get_or_init(|| {
        let mut discovery = SvDiscovery { table: std::ptr::null(), table_bytes: 0 };
        // SAFETY: aligned, exact-size local output; ORIGINAL is static.
        unsafe { ORIGINAL.discover(size_of::<SvDiscovery>() as u64, &mut discovery) };
        // SAFETY: the SDK returned its immutable full table, retained forever.
        let original = unsafe { *discovery.table.cast::<SvCoreV3>() };
        let exposed = SvCoreV3 { run_tx: Some(run), ..original };
        Tables { original, exposed }
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn statevec_core_plugin_entry_v3(bytes: u64, out: *mut SvDiscovery) -> u32 {
    if bytes != size_of::<SvDiscovery>() as u64 || out.is_null() || !out.is_aligned() { return 1; }
    // SAFETY: host's valid writable discovery slot; full immutable table.
    unsafe { out.write(SvDiscovery { table: (&tables().exposed as *const SvCoreV3).cast(), table_bytes: size_of::<SvCoreV3>() as u64 }) };
    0
}

unsafe extern "C" fn conflicting_write(context: *mut c_void, _: SvBytes) -> SvStatus {
    // SAFETY: run's synchronous read visitor carries the live table, not the
    // borrowed record. This asks the host to mutate while its read loan lives;
    // the host must refuse before creating a mutable Rust borrow.
    let tx = unsafe { &*context.cast::<SvTxV3>() };
    let mut found = 0;
    let key = SvKey { kind: 1, reserved16: 0, reserved32: 0, sys_id: 1 };
    // SAFETY: live context and independent writable flag. This is a checked
    // logical borrow conflict; no mutable alias is created by the fixture.
    let status = unsafe { (tx.delete_record.unwrap())(tx.context, key, &mut found) };
    if status == (&ExecutionFailure::AbiContract(AbiFailure::BorrowConflict)).into() && found == 0 {
        CALLS[5].fetch_add(1, Ordering::Relaxed);
    }
    SvStatus::SUCCESS // Deliberately try to hide the callback failure.
}

unsafe extern "C" fn run(instance: *const c_void, tx: *const SvTxV3, command: *const SvCommand) -> SvStatus {
    // SAFETY: the production host loans valid V3 pointers and the complete
    // preflight-checked two-u64 payload for this synchronous call.
    let (input, table) = unsafe { (&*command, &*tx) };
    let account = if input.payload.len == 16 {
        // SAFETY: host's bounded command payload, exactly 16 readable bytes.
        u64::from_le_bytes(unsafe { std::slice::from_raw_parts(input.payload.ptr, 8) }.try_into().unwrap())
    } else { 0 };
    // SAFETY: original SDK adapter uses the same live instance/table/command.
    let status = unsafe { (tables().original.run_tx.unwrap())(instance, tx, command) };
    if input.kind != 1 { return status; }
    match account {
        77 if status == (&ExecutionFailure::HostPanicked).into() => {
            CALLS[6].fetch_add(1, Ordering::Relaxed);
            SvStatus::SUCCESS // The native host's first fault must still win.
        }
        88 if status.is_success() => {
            let mut found = 0;
            let key = SvKey { kind: 1, reserved16: 0, reserved32: 0, sys_id: 1 };
            // SAFETY: synchronous shared read; the callback only invokes the
            // checked host API. Both flag and table outlive the callback.
            let _ = unsafe { (table.read.unwrap())(table.context, key, Some(conflicting_write), tx.cast_mut().cast(), &mut found) };
            SvStatus::SUCCESS
        }
        89 if status.is_success() => SvStatus { tag: 999, cause: 0, detail: [0; 3] },
        _ => status,
    }
}
