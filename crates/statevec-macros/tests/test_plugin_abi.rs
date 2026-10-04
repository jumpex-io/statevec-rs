// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

use statevec_api::*;
use statevec_model::{SchemaRegistry, Version};
use test_case::test_case;

#[path = "support/abi_host.rs"]
mod abi_host;
use abi_host::Host;

#[derive(Clone, Copy, Debug)]
#[repr(u16)]
enum Command {
    Scan = 1,
    Panic,
    KeyVisitorPanic,
    ReadVisitorPanic,
    CreateVisitorPanic,
    ReadUkVisitorPanic,
    UpdateVisitorPanic,
    Reject,
    HealthyCallbacks,
    PanicWithPanickingDrop,
}

struct ScanPlugin {
    configuration: String,
}

impl RuntimePlugin for ScanPlugin {
    fn name(&self) -> &'static str {
        "abi-regression"
    }
    fn schema_registry(&self) -> SchemaRegistry {
        schema()
    }

    fn run_tx(
        &self,
        tx: &mut dyn RuntimeHostContext,
        command: &dyn RuntimeCommandEnvelope,
    ) -> Result<(), RuntimePluginError> {
        let result = match command.command_kind() {
            kind if kind == Command::Panic as u16 => panic!("plugin transaction panic"),
            kind if kind == Command::KeyVisitorPanic as u16 => {
                tx.for_each_record_key_raw(1, &mut |_| panic!("key visitor panic"))
            }
            kind if kind == Command::ReadVisitorPanic as u16 => {
                tx.with_read_typed_raw(1, 7, &mut |_| panic!("read visitor panic")).map(|_| ())
            }
            kind if kind == Command::ReadUkVisitorPanic as u16 => tx
                .with_read_typed_by_uk_raw(1, b"key", &mut |_| panic!("read visitor panic"))
                .map(|_| ()),
            kind if kind == Command::CreateVisitorPanic as u16 => tx
                .create_typed_raw(1, &mut |bytes| {
                    bytes[0] = 9;
                    panic!("create visitor panic")
                })
                .map(|_| ()),
            kind if kind == Command::UpdateVisitorPanic as u16 => tx
                .update_typed_by_uk_raw(1, b"key", &mut |bytes| {
                    bytes[0] = 9;
                    panic!("update visitor panic")
                })
                .map(|_| ()),
            kind if kind == Command::Reject as u16 => return Err(RuntimePluginError::new("business rejection")),
            kind if kind == Command::PanicWithPanickingDrop as u16 => std::panic::panic_any(PanickingDrop),
            kind if kind == Command::HealthyCallbacks as u16 => {
                tx.with_read_typed_raw(1, 7, &mut |bytes| assert_eq!(bytes, [42]))
                    .map_err(|error| RuntimePluginError::new(error.to_string()))?;
                tx.create_typed_raw(1, &mut |bytes| bytes[0] = 9).map(|_| ())
            }
            _ => return self.scan(tx),
        };
        result.map_err(|error| RuntimePluginError::new(error.to_string()))
    }

    fn validate_biz_invariants(&self, ctx: &dyn BizInvariantReadContext) -> Result<(), String> {
        match self.configuration.as_str() {
            "panic-validate" => panic!("invariant plugin panic"),
            "panic-read" => {
                ctx.with_read_typed_raw(1, 7, &mut |_| panic!("invariant read panic"))
                    .map_err(|error| error.to_string())?;
            }
            "panic-read-uk" => {
                ctx.with_read_typed_by_uk_raw(1, b"key", &mut |_| panic!("invariant read panic"))
                    .map_err(|error| error.to_string())?;
            }
            "panic-keys" => {
                ctx.for_each_record_key_raw(1, &mut |_| panic!("invariant key panic"))
                    .map_err(|error| error.to_string())?;
            }
            _ => {}
        }
        Ok(())
    }

    fn on_unload(&mut self) -> Result<(), RuntimePluginUnloadError> {
        if self.configuration == "panic-unload" {
            panic!("unload panic");
        }
        Ok(())
    }
}

impl ScanPlugin {
    fn scan(&self, tx: &mut dyn RuntimeHostContext) -> Result<(), RuntimePluginError> {
        let mut keys = Vec::new();
        TypedTxContext::for_each_record_key(tx, 1, &mut |key| keys.push(key))
            .map_err(|error| RuntimePluginError::new(error.to_string()))?;
        let expected = match self.configuration.as_str() {
            "empty-scan" => vec![],
            "two-key-scan" => vec![RecordKey { kind: 1, sys_id: 7 }, RecordKey { kind: 1, sys_id: 8 }],
            _ => vec![RecordKey { kind: 1, sys_id: 7 }],
        };
        assert_eq!(keys, expected);
        Ok(())
    }
}

struct Factory;

impl RuntimePluginFactory for Factory {
    fn plugin_name(&self) -> &'static str {
        "abi-regression"
    }
    fn schema_registry(&self) -> SchemaRegistry {
        schema()
    }
    fn create(&self, configuration: &str) -> Result<Box<dyn RuntimePlugin>, RuntimePluginLoadError> {
        if configuration == "panic-create" {
            panic!("factory creation panic");
        }
        Ok(Box::new(ScanPlugin { configuration: configuration.to_owned() }))
    }
}

statevec_macros::export_runtime_plugin!(Box::new(Factory));

fn schema() -> SchemaRegistry {
    SchemaRegistry::with_records(Version::new(1, 0), &[])
}

fn empty_error() -> RuntimeErrorBuf {
    RuntimeErrorBuf::new(RuntimeErrorPhase::Load, runtime_error_kind::HOST_INTERNAL_ERROR, RuntimeBytesRef::empty())
}

struct PluginHandle {
    api: RuntimePluginApiV2,
    raw: *mut std::ffi::c_void,
}

impl PluginHandle {
    fn create() -> Self {
        Self::configured("")
    }

    fn configured(configuration: &str) -> Self {
        let api = statevec_runtime_plugin_entry_v2();
        let mut raw = std::ptr::null_mut();
        let mut error = empty_error();
        let status = unsafe {
            (api.create_runtime)(RuntimeBytesRef::from_slice(configuration.as_bytes()), &mut raw, &mut error)
        };
        assert_eq!(status, RuntimeCallStatus::Success, "valid plugin creation: {error:?}");
        assert!(!raw.is_null());
        Self { api, raw }
    }

    fn run(&mut self, host: &mut Host) -> (RuntimeCallStatus, RuntimeErrorBuf) {
        self.run_command(host, Command::Scan)
    }

    fn run_command(&mut self, host: &mut Host, command: Command) -> (RuntimeCallStatus, RuntimeErrorBuf) {
        let mut error = empty_error();
        let command = RuntimeCommandView {
            command_kind: command as u16,
            ext_seq: 1,
            ref_ext_time_us: 1,
            payload: RuntimeBytesRef::empty(),
        };
        // The handle, stack host, static vtable and output remain valid for
        // this synchronous ABI call; the export macro is production code.
        let status = unsafe { (self.api.run_tx)(self.raw, host.raw(), command, &mut error) };
        (status, error)
    }
}

impl Drop for PluginHandle {
    fn drop(&mut self) {
        unsafe {
            (self.api.destroy_runtime)(self.raw);
        }
    }
}

#[test]
fn typed_iteration_failure_after_a_prefix_cannot_become_plugin_success() {
    let mut plugin = PluginHandle::create();
    let mut host = Host::new(RuntimeCallStatus::Failure);

    let (status, error) = plugin.run(&mut host);

    assert_eq!(host.visits.get(), 1, "failure follows a real visitor invocation");
    assert_eq!(status, RuntimeCallStatus::Failure, "partial iteration must not become successful execution");
    assert_eq!(error.phase, RuntimeErrorPhase::RunTx);
    assert!(runtime_error_text(&error).contains("iteration failed"));
}

#[test]
fn completed_typed_iteration_remains_successful() {
    let mut plugin = PluginHandle::create();
    let mut host = Host::new(RuntimeCallStatus::Success);

    let (status, _) = plugin.run(&mut host);

    assert_eq!(host.visits.get(), 1);
    assert_eq!(status, RuntimeCallStatus::Success);
}

#[test_case("empty-scan", &[], RuntimeCallStatus::Failure; "before_first_visit")]
#[test_case("two-key-scan", &[7, 8], RuntimeCallStatus::Failure; "after_final_visit")]
#[test_case("empty-scan", &[], RuntimeCallStatus::Success; "completed_empty")]
#[test_case("two-key-scan", &[7, 8], RuntimeCallStatus::Success; "completed_multiple")]
fn traversal_completion_not_callback_count_authorizes_success(
    configuration: &str,
    keys: &[u64],
    completion: RuntimeCallStatus,
) {
    let mut plugin = PluginHandle::configured(configuration);
    let mut host = Host::new(completion);
    host.keys = keys.to_vec();
    let (status, error) = plugin.run(&mut host);

    assert_eq!(host.visits.get(), keys.len());
    assert_eq!(host.returned_callbacks.get(), keys.len());
    assert_eq!(status, completion, "even the final key does not certify traversal completion");
    if completion == RuntimeCallStatus::Failure {
        assert_eq!(error.phase, RuntimeErrorPhase::RunTx);
        assert!(runtime_error_text(&error).contains("iteration failed"));
        // A handled host error is not a poisoned ABI handle. Retry only a new,
        // independent read; no transaction is silently replayed by the adapter.
        host.completion = RuntimeCallStatus::Success;
        assert_eq!(plugin.run(&mut host).0, RuntimeCallStatus::Success);
        assert_eq!(host.visits.get(), 2 * keys.len());
    }
}

struct PanickingDrop;
impl Drop for PanickingDrop {
    fn drop(&mut self) {
        panic!("panic payload destructor");
    }
}

#[test_case(Command::Panic; "plugin")]
#[test_case(Command::KeyVisitorPanic; "keys")]
#[test_case(Command::ReadVisitorPanic; "read")]
#[test_case(Command::CreateVisitorPanic; "create")]
#[test_case(Command::ReadUkVisitorPanic; "read_by_uk")]
#[test_case(Command::UpdateVisitorPanic; "update")]
#[test_case(Command::PanicWithPanickingDrop; "payload_destructor")]
fn unwind_panics_return_internal_failure_before_the_c_boundary(command: Command) {
    let mut plugin = PluginHandle::create();
    let mut host = Host::new(RuntimeCallStatus::Success);

    let (status, error) = plugin.run_command(&mut host, command);

    assert_eq!(status, RuntimeCallStatus::Failure, "an unwind panic must never report success");
    assert_eq!(error.phase, RuntimeErrorPhase::RunTx);
    assert_eq!(error.kind, runtime_error_kind::PLUGIN_INTERNAL_ERROR, "panic is not a business rejection");
    if matches!(command, Command::CreateVisitorPanic | Command::UpdateVisitorPanic) {
        assert_eq!(host.last_byte.get(), 9, "panic containment must not claim rollback");
    }
    assert_eq!(host.visits.get(), host.returned_callbacks.get(), "every C callback frame must return normally");
}

#[test_case("panic-validate"; "plugin")]
#[test_case("panic-read"; "read")]
#[test_case("panic-read-uk"; "read_by_uk")]
#[test_case("panic-keys"; "keys")]
fn invariant_panics_return_typed_failure(configuration: &str) {
    let plugin = PluginHandle::configured(configuration);
    let host = Host::new(RuntimeCallStatus::Success);
    let mut error = empty_error();

    let status = unsafe { (plugin.api.validate_biz_invariants)(plugin.raw, host.read_raw(), &mut error) };

    assert_eq!(status, RuntimeCallStatus::Failure);
    assert_eq!(error.phase, RuntimeErrorPhase::ValidateBizInvariants);
    assert_eq!(error.kind, runtime_error_kind::PLUGIN_INTERNAL_ERROR);
    assert_eq!(host.visits.get(), host.returned_callbacks.get());
}

#[test]
fn create_panic_returns_failure_without_publishing_a_handle() {
    let api = statevec_runtime_plugin_entry_v2();
    let mut raw = std::ptr::null_mut();
    let mut error = empty_error();

    let status = unsafe { (api.create_runtime)(RuntimeBytesRef::from_slice(b"panic-create"), &mut raw, &mut error) };

    assert_eq!(status, RuntimeCallStatus::Failure);
    assert!(raw.is_null());
    assert_eq!(error.phase, RuntimeErrorPhase::Create);
    assert_eq!(error.kind, runtime_error_kind::PLUGIN_INTERNAL_ERROR);
}

#[test]
fn unload_panic_returns_failure_and_keeps_the_handle_destroyable() {
    let plugin = PluginHandle::configured("panic-unload");
    let mut error = empty_error();

    let status = unsafe { (plugin.api.on_unload)(plugin.raw, &mut error) };

    assert_eq!(status, RuntimeCallStatus::Failure);
    assert_eq!(error.phase, RuntimeErrorPhase::Unload);
    assert_eq!(error.kind, runtime_error_kind::PLUGIN_INTERNAL_ERROR);
}

#[test]
fn successful_callbacks_and_business_rejection_keep_distinct_dispositions() {
    let mut plugin = PluginHandle::create();
    let mut host = Host::new(RuntimeCallStatus::Success);
    let (status, _) = plugin.run_command(&mut host, Command::HealthyCallbacks);
    assert_eq!(status, RuntimeCallStatus::Success);
    assert_eq!(host.visits.get(), 2);
    assert_eq!(host.last_byte.get(), 9);

    let (status, error) = plugin.run_command(&mut host, Command::Reject);
    assert_eq!(status, RuntimeCallStatus::Failure);
    assert_eq!(error.kind, runtime_error_kind::PLUGIN_REJECTED);
    assert_eq!(runtime_error_text(&error), "business rejection");
}

#[test]
fn a_caught_visitor_panic_is_not_reentered_or_erased_by_host_success() {
    let mut host = Host::new(RuntimeCallStatus::Success);
    host.keys.push(8);
    let mut raw = host.raw();
    let adapter = unsafe { RuntimeHostContextV1Adapter::from_raw(&mut raw) };
    let mut callbacks = 0;

    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        adapter.for_each_record_key_raw(1, &mut |_| {
            callbacks += 1;
            panic!("first key fails");
        })
    }));

    assert!(outcome.is_err(), "panic resumes only after the host C frame returns");
    assert_eq!(callbacks, 1, "a failed Rust visitor must not be invoked again");
    assert_eq!(host.visits.get(), 2, "host still attempted the next key");
    assert_eq!(host.returned_callbacks.get(), 2);
}
