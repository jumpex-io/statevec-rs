//! Scripted host callbacks qualify the SDK loan/return boundary, not native
//! migration, canonical indexes, history or atomic installation.
use super::*;
use crate::{BusinessRejectCode, RuntimeCommandRef, v1::*};
use statevec_model::{CommandSchemaError, CoreVersion, SchemaRegistry, Version};
use std::cell::{Cell, RefCell};
use std::mem::size_of;
use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};
use test_case::test_case;

struct Module(Arc<AtomicUsize>);
struct Configured { calls: Arc<AtomicUsize>, schema: SchemaRegistry }
struct Runtime(Arc<AtomicUsize>);
impl CoreModule for Module {
    type Configured = Configured;
    fn configure(&self, _: CoreConfig<'_>) -> Result<Configured, ConfigureError> {
        Ok(Configured { calls: Arc::clone(&self.0), schema: SchemaRegistry::with_records(Version::new(1, 0), &[]) })
    }
}
impl ConfiguredCore for Configured {
    type Runtime = Runtime;
    fn metadata(&self) -> CoreMetadata<'_> {
        CoreMetadata { name: "migration-abi", revision: "migration-abi-v1", schema: &self.schema }
    }
    fn create_instance(&self) -> Result<Runtime, CoreCreateError> { Ok(Runtime(Arc::clone(&self.calls))) }
}
impl CoreRuntime for Runtime {
    const TX_INVARIANTS: Option<TxInvariantHook<Self>> = None;
    fn preflight_command(&self, _: &RuntimeCommandRef<'_>) -> Result<(), CommandSchemaError> { Ok(()) }
    fn run_tx(&self, _: &mut PluginTx<'_>, _: &RuntimeCommandRef<'_>) -> Result<(), CoreTxError> { Ok(()) }
    fn initialize_core(&self, _: &mut PluginTx<'_>, _: &[u8]) -> Result<(), CoreTxError> { Ok(()) }
    fn migrate(&self, tx: &mut MigrationTx<'_>, source: CoreVersion, input: &[u8]) -> Result<(), CoreTxError> {
        self.0.fetch_add(1, Ordering::Relaxed);
        assert_eq!(source.digest().bytes(), [7; 32]);
        assert_eq!((tx.tx_seq()?, tx.ref_tx_time_ns()?), (17, 123));
        let (source, mut target) = tx.records();
        let mut visited = Vec::new();
        source.for_each_key(1, &mut |key| visited.push(key))?;
        assert_eq!(visited, [RecordKey { kind: 1, sys_id: 7 }]);
        // Deliberately ignore the facade error for the hidden-failure case.
        let result = source.with_record(visited[0], |bytes| {
            if input == b"visitor-panic" { panic!("scripted migration visitor panic"); }
            assert_eq!(bytes, &[1, 2, 3]);
            target.preserve_record(visited[0], 2, bytes)
        });
        if input != b"hide" { result?.unwrap()?; }
        let created = target.create_record(3, b"new");
        if input != b"hide" { assert_eq!(created?, RecordKey { kind: 3, sys_id: 9 }); }
        target.emit_event(4, input)?;
        match input {
            b"panic" => panic!("scripted migration handler panic"),
            b"reject" => Err(CoreTxError::Rejected(BusinessRejectCode::new(42).unwrap())),
            _ => Ok(()),
        }
    }
}

unsafe extern "C" fn unused_configure(_: SvBytes, _: *mut *mut c_void) -> SvStatus {
    SvStatus::from(&CoreAbiError::InvalidHandle)
}
static EXPORT: __export::CoreExportTable = __export::CoreExportTable::new::<Module>(unused_configure);

struct Plugin {
    table: SvCoreV4,
    configured: *mut c_void,
    instance: *mut c_void,
    calls: Arc<AtomicUsize>,
}
impl Plugin {
    fn open() -> Self {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut discovery = SvDiscovery { table: std::ptr::null(), table_bytes: 0 };
        let (mut configured, mut instance) = (std::ptr::null_mut(), std::ptr::null_mut());
        // SAFETY: exact SDK discovery/configuration and uniquely owned outputs.
        let table = unsafe {
            assert_eq!(EXPORT.discover(size_of::<SvDiscovery>() as u64, &mut discovery), 0);
            let table = *discovery.table.cast::<SvCoreV4>();
            assert_eq!(__export::configure(&Module(Arc::clone(&calls)), SvBytes::EMPTY, &mut configured), SvStatus::SUCCESS);
            assert_eq!((table.create_instance.unwrap())(configured, &mut instance), SvStatus::SUCCESS);
            table
        };
        Self { table, configured, instance, calls }
    }
    fn run(&self, table: &SvMigrationV4, input: &[u8]) -> SvStatus {
        // SAFETY: exact live instance plus this synchronous scripted host loan.
        unsafe { (self.table.migrate.unwrap())(self.instance, table, &[7; 32], SvBytes::borrowed(input)) }
    }
}
impl Drop for Plugin {
    fn drop(&mut self) {
        // SAFETY: each SDK-owned handle is consumed exactly once.
        unsafe {
            assert_eq!((self.table.destroy_instance.unwrap())(&mut self.instance), SvStatus::SUCCESS);
            assert_eq!((self.table.destroy_configured.unwrap())(&mut self.configured), SvStatus::SUCCESS);
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum HostFault { None, WrongKey, HostPanic, DoubleRead, WalkCount, Storage(CoreStorageError), Missing, EventSequence, Payload }
struct Host {
    fault: HostFault,
    writes: RefCell<Vec<(RecordKey, Vec<u8>)>>,
    creates: Cell<usize>,
}
impl Host {
    fn new(fault: HostFault) -> Self { Self { fault, writes: RefCell::new(Vec::new()), creates: Cell::new(0) } }
    fn table(&self) -> SvMigrationV4 {
        SvMigrationV4 {
            h: SvHeader { abi_major: 4, kind: 6, table_bytes: 96, provided_caps: 255, required_caps: 223 },
            context: (self as *const Self).cast(),
            time_ns: Some(time), tx_seq: Some(sequence), source_read: Some(read), source_keys: Some(keys),
            preserve_record: Some(preserve), create_record: Some(create), emit_event: Some(event),
        }
    }
}
// SAFETY for these mechanical fixture callbacks: table() supplies this exact
// live Host and the SDK supplies valid scoped outputs/visitors for every call.
unsafe extern "C" fn time(_: *const c_void, out: *mut u64) -> SvStatus {
    unsafe { out.write(123) }; SvStatus::SUCCESS
}
unsafe extern "C" fn sequence(_: *const c_void, out: *mut u64) -> SvStatus {
    unsafe { out.write(17) }; SvStatus::SUCCESS
}
unsafe extern "C" fn keys(raw: *const c_void, kind: u16, visit: SvKeyVisitor, context: *mut c_void, out: *mut SvWalk) -> SvStatus {
    let host = unsafe { &*raw.cast::<Host>() };
    let mut flow = 0;
    let status = unsafe { visit.unwrap()(context, wire_key(RecordKey { kind, sys_id: 7 }), &mut flow) };
    unsafe { out.write(SvWalk { tag: 0, reserved: 0, count: if matches!(host.fault, HostFault::WalkCount) { 2 } else { 1 } }) };
    status
}
unsafe extern "C" fn read(raw: *const c_void, _: SvKey, visit: SvReadVisitor, context: *mut c_void, found: *mut u32) -> SvStatus {
    let host = unsafe { &*raw.cast::<Host>() };
    unsafe {
        found.write(1);
        let status = visit.unwrap()(context, SvBytes::borrowed(&[1, 2, 3]));
        if matches!(host.fault, HostFault::DoubleRead) {
            let _ = visit.unwrap()(context, SvBytes::borrowed(&[1, 2, 3]));
            return SvStatus::SUCCESS; // hostile-to-contract success cannot hide the visitor failure
        }
        status
    }
}
unsafe extern "C" fn preserve(raw: *const c_void, source: SvKey, kind: u16, bytes: SvBytes, out: *mut SvKey) -> SvStatus {
    let host = unsafe { &*raw.cast::<Host>() };
    if matches!(host.fault, HostFault::HostPanic) { return SvStatus::from(&CoreExecutionError::HostPanicked); }
    if let HostFault::Storage(source) = host.fault {
        return SvStatus::from(&CoreAccessError::Storage(source));
    }
    match host.fault {
        HostFault::Missing => return SvStatus::from(&CoreAccessError::MissingRecord { key: RecordKey { kind: source.kind, sys_id: source.sys_id } }),
        HostFault::EventSequence => return SvStatus::from(&CoreAccessError::EventSequenceOverflow),
        HostFault::Payload => return SvStatus::from(&CoreAccessError::PayloadLength { kind, expected: 4, actual: 3 }),
        _ => {}
    }
    let key = RecordKey { kind, sys_id: source.sys_id };
    let data = unsafe { std::slice::from_raw_parts(bytes.ptr, bytes.len as usize) };
    host.writes.borrow_mut().push((key, data.to_vec()));
    let returned = if matches!(host.fault, HostFault::WrongKey) { RecordKey { kind, sys_id: 8 } } else { key };
    unsafe { out.write(wire_key(returned)) };
    SvStatus::SUCCESS
}
unsafe extern "C" fn create(raw: *const c_void, kind: u16, _: SvBytes, out: *mut SvKey) -> SvStatus {
    let host = unsafe { &*raw.cast::<Host>() };
    host.creates.set(host.creates.get() + 1);
    unsafe { out.write(wire_key(RecordKey { kind, sys_id: 9 })) };
    SvStatus::SUCCESS
}
unsafe extern "C" fn event(_: *const c_void, _: u16, _: SvBytes) -> SvStatus { SvStatus::SUCCESS }

#[test_case(b"copy", None; "successful_copy")]
#[test_case(b"reject", Some(CoreTxError::Rejected(BusinessRejectCode::new(42).unwrap())); "rejection_after_writes")]
#[test_case(b"panic", Some(CoreTxError::Execution(CoreExecutionError::PluginPanicked)); "panic_after_writes")]
fn exported_migration_preserves_result_and_exact_callback_inputs(input: &[u8], expected: Option<CoreTxError>) {
    let plugin = Plugin::open();
    let host = Host::new(HostFault::None);
    let result = plugin.run(&host.table(), input).decode_for(CoreOperation::Migrate).unwrap();
    assert_eq!(result, expected.map(CoreError::Transaction));
    assert_eq!(plugin.calls.load(Ordering::Relaxed), 1);
    assert_eq!(*host.writes.borrow(), [(RecordKey { kind: 2, sys_id: 7 }, vec![1, 2, 3])]);
    assert_eq!(host.creates.get(), 1);
}

#[test_case(HostFault::WrongKey, CoreExecutionError::AbiContract(CoreAbiError::InvalidHandle); "wrong_preserved_id")]
#[test_case(HostFault::HostPanic, CoreExecutionError::HostPanicked; "host_panic")]
#[test_case(HostFault::DoubleRead, CoreExecutionError::AbiContract(CoreAbiError::CallbackCount { expected: 1, actual: 2 }); "repeated_read_visitor")]
#[test_case(HostFault::WalkCount, CoreExecutionError::AbiContract(CoreAbiError::CallbackCount { expected: 1, actual: 2 }); "walk_count")]
#[test_case(HostFault::Storage(CoreStorageError::Capacity), CoreExecutionError::Access(CoreAccessError::Storage(CoreStorageError::Capacity)); "ignored_capacity")]
#[test_case(HostFault::Storage(CoreStorageError::Io), CoreExecutionError::Access(CoreAccessError::Storage(CoreStorageError::Io)); "ignored_io")]
#[test_case(HostFault::Storage(CoreStorageError::Internal), CoreExecutionError::Access(CoreAccessError::Storage(CoreStorageError::Internal)); "ignored_internal")]
fn ignored_host_failure_fences_later_writes_and_final_success(fault: HostFault, expected: CoreExecutionError) {
    let plugin = Plugin::open();
    let host = Host::new(fault);
    assert_eq!(plugin.run(&host.table(), b"hide").decode_for(CoreOperation::Migrate),
        Ok(Some(CoreError::Transaction(CoreTxError::Execution(expected)))));
    assert_eq!(host.creates.get(), 0);
}

#[test_case(HostFault::Missing; "handled missing record")]
#[test_case(HostFault::EventSequence; "handled deterministic bound")]
#[test_case(HostFault::Payload; "handled bad payload")]
fn handled_deterministic_access_error_does_not_poison_the_migration(fault: HostFault) {
    let plugin = Plugin::open();
    let host = Host::new(fault);
    assert_eq!(plugin.run(&host.table(), b"hide"), SvStatus::SUCCESS);
    assert!(host.writes.borrow().is_empty(), "failed preserve did not publish output");
    assert_eq!(host.creates.get(), 1, "subsequent author work is still permitted");
}

#[test]
fn source_visitor_panic_cannot_escape_or_be_hidden() {
    let plugin = Plugin::open();
    let host = Host::new(HostFault::None);
    assert_eq!(plugin.run(&host.table(), b"visitor-panic").decode_for(CoreOperation::Migrate),
        Ok(Some(CoreError::Transaction(CoreTxError::Execution(CoreExecutionError::PluginPanicked)))));
    assert!(host.writes.borrow().is_empty());
    assert_eq!(host.creates.get(), 0);
}

#[derive(Clone, Copy, Debug)]
enum BadTable { OldMajor, CandidateKind, MissingSourceRead, MissingTargetWrite, MissingCapability, NullContext }
#[test_case(BadTable::OldMajor, CoreAbiError::Version { expected: 4, actual: 3 }; "old_major")]
#[test_case(BadTable::MissingSourceRead, CoreAbiError::NullPointer; "source_callback")]
#[test_case(BadTable::MissingTargetWrite, CoreAbiError::NullPointer; "target_callback")]
#[test_case(BadTable::MissingCapability, CoreAbiError::UnsupportedCapabilities { required: 128 }; "capability")]
#[test_case(BadTable::NullContext, CoreAbiError::NullPointer; "context")]
#[test_case(BadTable::CandidateKind, CoreAbiError::Kind { expected: 6, actual: 4 }; "candidate_is_not_migration")]
fn bad_migration_host_table_refuses_before_author_callback(fault: BadTable, expected: CoreAbiError) {
    let plugin = Plugin::open();
    let host = Host::new(HostFault::None);
    let mut table = host.table();
    match fault {
        BadTable::OldMajor => table.h.abi_major = 3,
        BadTable::CandidateKind => table.h.kind = 4,
        BadTable::MissingSourceRead => table.source_read = None,
        BadTable::MissingTargetWrite => table.preserve_record = None,
        BadTable::MissingCapability => table.h.provided_caps &= !128,
        BadTable::NullContext => table.context = std::ptr::null(),
    }
    assert_eq!(plugin.run(&table, b"copy").decode_for(CoreOperation::Migrate),
        Ok(Some(CoreError::Transaction(CoreTxError::Execution(CoreExecutionError::AbiContract(expected))))));
    assert_eq!(plugin.calls.load(Ordering::Relaxed), 0);
    assert!(host.writes.borrow().is_empty());
}
