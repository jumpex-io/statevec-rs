//! Executes SDK-generated entrypoints, not a private engine or recovery oracle.
use super::*;
use crate::{RuntimeCommandEnvelope, TxReadContext, TxWriteContext};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use test_case::test_case;

#[derive(Default)]
struct Counts {
    configure: AtomicUsize,
    create: AtomicUsize,
    run: AtomicUsize,
    drop_instance: AtomicUsize,
    drop_configured: AtomicUsize,
}
struct Module {
    counts: Arc<Counts>,
}
struct Bound {
    schema: statevec_model::SchemaRegistry,
    counts: Arc<Counts>,
    fault: u8,
}
struct Runtime {
    counts: Arc<Counts>,
    fault: u8,
}
impl CoreModule for Module {
    type Configured = Bound;
    fn configure(&self, config: CoreConfig<'_>) -> Result<Bound, super::super::ConfigureError> {
        self.counts.configure.fetch_add(1, Ordering::Relaxed);
        Ok(Bound {
            schema: statevec_model::SchemaRegistry::new(
                statevec_model::Version::new(1, 2),
                &[],
                &[],
                &[
                    statevec_model::EventDefinition {
                        kind: 7,
                        name: "Inline",
                        version: 1,
                        fields: &[],
                        inline_response: true,
                    },
                    statevec_model::EventDefinition {
                        kind: 8,
                        name: "Projected",
                        version: 1,
                        fields: &[],
                        inline_response: false,
                    },
                ],
                &[],
            ),
            counts: self.counts.clone(),
            fault: config.bytes[0],
        })
    }
}
impl ConfiguredCore for Bound {
    type Runtime = Runtime;
    fn metadata(&self) -> CoreMetadata<'_> {
        CoreMetadata {
            name: "safe-rust-fixture",
            revision: "rules-v1",
            schema: &self.schema,
        }
    }
    fn create_instance(&self) -> Result<Runtime, super::super::CoreCreateError> {
        self.counts.create.fetch_add(1, Ordering::Relaxed);
        if self.fault == 1 {
            panic!("injected constructor panic");
        }
        Ok(Runtime {
            counts: self.counts.clone(),
            fault: self.fault,
        })
    }
}
impl CoreRuntime for Runtime {
    fn initialize_core(&self, tx: &mut PluginTx<'_>, input: &[u8]) -> Result<(), super::super::CoreTxError> {
        self.run_tx(tx, &RuntimeCommandRef::new(1, 0, 0, input))
    }
    fn migrate(
        &self, _: &mut super::super::MigrationTx<'_>, _: statevec_model::CoreVersion, _: &[u8],
    ) -> Result<(), super::super::CoreTxError> { Ok(()) }
    const TX_INVARIANTS: Option<TxInvariantHook<Self>> = Some(|_, _| Ok(()));
    fn preflight_command(
        &self,
        command: &RuntimeCommandRef<'_>,
    ) -> Result<(), statevec_model::CommandSchemaError> {
        match command.command_kind() {
            1 => Ok(()),
            2 => panic!("injected preflight panic"),
            kind => Err(statevec_model::CommandSchemaError::UnsupportedKind { kind }),
        }
    }
    fn run_tx(
        &self,
        tx: &mut PluginTx<'_>,
        _: &RuntimeCommandRef<'_>,
    ) -> Result<(), super::super::CoreTxError> {
        self.counts.run.fetch_add(1, Ordering::Relaxed);
        if self.fault == 2 {
            panic!("injected handler panic");
        }
        // A void callback failure must survive success and a later unwind.
        tx.emit_event_raw(7, vec![42]);
        if self.fault == 5 {
            panic!("later handler panic");
        }
        let _ = tx.ref_tx_time_ns_raw();
        Ok(())
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        self.counts.drop_instance.fetch_add(1, Ordering::Relaxed);
        if self.fault == 3 {
            panic!("injected instance destructor panic");
        }
    }
}
impl Drop for Bound {
    fn drop(&mut self) {
        self.counts.drop_configured.fetch_add(1, Ordering::Relaxed);
        if self.fault == 4 {
            panic!("injected configured destructor panic");
        }
    }
}

unsafe extern "C" fn unused(_: SvBytes, _: *mut *mut c_void) -> SvStatus {
    panic!("configure is called with this test's module directly")
}
static TABLE: CoreExportTable = CoreExportTable::new::<Module>(unused);
struct Fixture {
    counts: Arc<Counts>,
    configured: *mut c_void,
    instance: *mut c_void,
}
impl Fixture {
    fn bound(fault: u8) -> Self {
        let counts = Arc::new(Counts::default());
        let module = Module {
            counts: counts.clone(),
        };
        let mut configured = std::ptr::null_mut();
        // SAFETY: scoped input/output and real SDK-generated configure adapter.
        assert_eq!(
            unsafe { configure(&module, SvBytes::borrowed(&[fault]), &mut configured) },
            SvStatus::SUCCESS
        );
        Self {
            counts,
            configured,
            instance: std::ptr::null_mut(),
        }
    }
    fn create(&mut self) -> SvStatus {
        // SAFETY: exact live configured holding and writable instance slot.
        unsafe { (TABLE.0.create_instance.unwrap())(self.configured, &mut self.instance) }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        // SAFETY: only non-consumed allocations returned by SDK are retired.
        unsafe {
            if !self.instance.is_null() {
                (TABLE.0.destroy_instance.unwrap())(&mut self.instance);
            }
            if !self.configured.is_null() {
                (TABLE.0.destroy_configured.unwrap())(&mut self.configured);
            }
        }
    }
}

#[test]
fn immutable_metadata_and_independent_instances_come_from_one_binding() {
    let mut fixture = Fixture::bound(0);
    let mut metadata = std::mem::MaybeUninit::<SvCoreMetadata>::uninit();
    // SAFETY: configured handle is alive; complete uninitialized output accepted.
    assert_eq!(
        unsafe { (TABLE.0.metadata.unwrap())(fixture.configured, metadata.as_mut_ptr()) },
        SvStatus::SUCCESS
    );
    let metadata = unsafe { metadata.assume_init() };
    assert_eq!(metadata.tx_rules, 1);
    let schema = unsafe { bytes(metadata.schema_idl).unwrap() };
    let restored =
        statevec_model::SchemaRegistry::from_idl_bytes_with_limits(schema, Default::default())
            .unwrap();
    assert_eq!(SvSchemaIdentity::from(restored.identity()), metadata.schema);
    assert!(
        restored.try_get_event(7).unwrap().inline_response,
        "module metadata must preserve delivery policy outside canonical identity"
    );
    assert!(!restored.try_get_event(8).unwrap().inline_response);
    assert_eq!(fixture.create(), SvStatus::SUCCESS);
    let mut second = std::ptr::null_mut();
    assert_eq!(
        unsafe { (TABLE.0.create_instance.unwrap())(fixture.configured, &mut second) },
        SvStatus::SUCCESS
    );
    assert_ne!(fixture.instance, second);
    assert_eq!(
        unsafe { (TABLE.0.destroy_instance.unwrap())(&mut second) },
        SvStatus::SUCCESS
    );
    let counts = fixture.counts.clone();
    drop(fixture);
    assert_eq!(counts.configure.load(Ordering::Relaxed), 1);
    assert_eq!(counts.create.load(Ordering::Relaxed), 2);
    assert_eq!(counts.drop_instance.load(Ordering::Relaxed), 2);
    assert_eq!(counts.drop_configured.load(Ordering::Relaxed), 1);
}

#[test_case(2, 7; "preflight unwind")]
#[test_case(3, 2; "schema refusal")]
fn preflight_returns_first_failing_ordinal_without_dispatch(kind: u16, tag: u32) {
    let mut fixture = Fixture::bound(0);
    assert!(fixture.create().is_success());
    let command = |kind| SvCommand {
        kind,
        reserved16: 0,
        reserved32: 0,
        ext_seq: 1,
        ref_ext_time_us: 5,
        payload: SvBytes::EMPTY,
    };
    let commands = [command(1), command(kind), command(1)];
    let mut ordinal = u64::MAX;
    let status = unsafe {
        (TABLE.0.preflight_batch.unwrap())(fixture.instance, commands.as_ptr(), 3, &mut ordinal)
    };
    assert_eq!(status.tag, tag);
    assert_eq!(ordinal, 1);
    assert_eq!(fixture.counts.run.load(Ordering::Relaxed), 0);
}

#[derive(Clone, Copy)]
enum LifecyclePanic {
    Construction = 1,
    InstanceDestruction = 3,
    ConfiguredDestruction = 4,
}
#[test_case(LifecyclePanic::Construction; "construction")]
#[test_case(LifecyclePanic::InstanceDestruction; "instance destruction")]
#[test_case(LifecyclePanic::ConfiguredDestruction; "configured destruction")]
fn caught_lifecycle_panic_never_leaves_a_retryable_output(fault: LifecyclePanic) {
    let mut fixture = Fixture::bound(fault as u8);
    let status = match fault {
        LifecyclePanic::Construction => fixture.create(),
        LifecyclePanic::InstanceDestruction => {
            assert!(fixture.create().is_success());
            unsafe { (TABLE.0.destroy_instance.unwrap())(&mut fixture.instance) }
        }
        LifecyclePanic::ConfiguredDestruction => {
            unsafe { (TABLE.0.destroy_configured.unwrap())(&mut fixture.configured) }
        }
    };
    assert_eq!(
        status,
        SvStatus {
            tag: 7,
            cause: 0,
            detail: [0; 3]
        }
    );
    assert!(fixture.instance.is_null());
    if matches!(fault, LifecyclePanic::ConfiguredDestruction) {
        assert!(fixture.configured.is_null());
    }
}

#[test]
fn oversized_configuration_is_refused_without_following_its_pointer() {
    let counts = Arc::new(Counts::default());
    let module = Module {
        counts: counts.clone(),
    };
    let mut out = std::ptr::dangling_mut();
    let status = unsafe {
        configure(
            &module,
            SvBytes {
                ptr: std::ptr::null(),
                len: 1024 * 1024 + 1,
            },
            &mut out,
        )
    };
    assert_eq!(
        status,
        SvStatus::from(&CoreAbiError::Length {
            actual: 1024 * 1024 + 1,
            maximum: 1024 * 1024
        })
    );
    assert!(out.is_null());
    assert_eq!(counts.configure.load(Ordering::Relaxed), 0);
}

#[test]
fn huge_command_count_refuses_before_array_access() {
    let mut fixture = Fixture::bound(0);
    assert!(fixture.create().is_success());
    let mut bad = u64::MAX;
    let status = unsafe {
        (TABLE.0.preflight_batch.unwrap())(fixture.instance, std::ptr::null(), u64::MAX, &mut bad)
    };
    assert_eq!(status.tag, 9);
    assert_eq!(status.cause, 3);
    assert_eq!(bad, 0);
}

#[test]
fn non_utf8_configuration_is_refused_before_user_configuration() {
    let counts = Arc::new(Counts::default());
    let module = Module {
        counts: counts.clone(),
    };
    let mut out = std::ptr::null_mut();
    let status = unsafe { configure(&module, SvBytes::borrowed(&[0xff]), &mut out) };
    assert_eq!(status, SvStatus::from(&CoreAbiError::Utf8));
    assert!(out.is_null());
    assert_eq!(counts.configure.load(Ordering::Relaxed), 0);
}

#[derive(Clone, Copy)]
enum HandlerExit {
    Success = 0,
    Panic = 5,
}
#[test_case(HandlerExit::Success; "handler returns success")]
#[test_case(HandlerExit::Panic; "later panic cannot be downgraded to an ordinary callback error")]
fn exported_run_preserves_void_callback_failure_without_hiding_unusability(exit: HandlerExit) {
    use crate::v1::tx::tests::{Fault, Host, table};
    use crate::v1::{CoreExecutionError, HostError, ResourceError};
    let mut fixture = Fixture::bound(exit as u8);
    assert!(fixture.create().is_success());
    let mut host = Host::default();
    host.fault = Fault::EventFailure;
    let table = table(&host);
    let command = SvCommand {
        kind: 1,
        reserved16: 0,
        reserved32: 0,
        ext_seq: 1,
        ref_ext_time_us: 100,
        payload: SvBytes::EMPTY,
    };
    let status = unsafe { (TABLE.0.run_tx.unwrap())(fixture.instance, &table, &command) };
    let expected = match exit {
        HandlerExit::Panic => CoreExecutionError::PluginPanicked,
        HandlerExit::Success => CoreExecutionError::Host(HostError::Resource(ResourceError::Os { code: 5 })),
    };
    assert_eq!(status, SvStatus::from(&expected));
    assert_eq!(fixture.counts.run.load(Ordering::Relaxed), 1);
}

#[test_case(HandlerExit::Success; "callback_error_then_success")]
#[test_case(HandlerExit::Panic; "callback_error_then_panic")]
fn exported_initialization_preserves_the_transaction_failure_latch(exit: HandlerExit) {
    use crate::v1::tx::tests::{Fault, Host, table};
    use crate::v1::{CoreExecutionError, CoreOperation, HostError, ResourceError};
    let mut fixture = Fixture::bound(exit as u8);
    assert!(fixture.create().is_success());
    let mut host = Host::default();
    host.fault = Fault::EventFailure;
    let tx = table(&host);
    // SAFETY: scoped scripted host table and exact SDK-owned instance.
    let status = unsafe { (TABLE.0.initialize_core.unwrap())(fixture.instance, &tx, SvBytes::borrowed(b"init")) };
    let expected = match exit {
        HandlerExit::Success => CoreExecutionError::Host(HostError::Resource(ResourceError::Os { code: 5 })),
        HandlerExit::Panic => CoreExecutionError::PluginPanicked,
    };
    assert_eq!(status, SvStatus::from(&expected));
    assert!(status.decode_for(CoreOperation::InitializeCore).is_ok());
    assert_eq!(fixture.counts.run.load(Ordering::Relaxed), 1);
}

#[test]
fn initialization_refuses_old_host_major_before_calling_author() {
    use crate::v1::tx::tests::{Host, table};
    let mut fixture = Fixture::bound(0);
    assert!(fixture.create().is_success());
    let host = Host::default();
    let mut tx = table(&host);
    tx.h.abi_major = 3;
    // SAFETY: complete table with an intentionally unsupported header.
    let status = unsafe { (TABLE.0.initialize_core.unwrap())(fixture.instance, &tx, SvBytes::EMPTY) };
    assert_eq!(status, SvStatus::from(&CoreAbiError::Version { expected: 4, actual: 3 }));
    assert_eq!(fixture.counts.run.load(Ordering::Relaxed), 0);
}
