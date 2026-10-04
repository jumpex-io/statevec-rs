// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

use std::collections::BTreeMap;

use statevec_api::{
    RecordKey, RuntimeBytesRef, RuntimeCallStatus, RuntimeCommandEnvelope, RuntimeCommandView, RuntimeErrorBuf,
    RuntimeErrorPhase, RuntimeHostContext, RuntimeHostContextExt, RuntimeHostError, RuntimePlugin, RuntimePluginError,
    RuntimePluginFactory, RuntimePluginLoadError, RuntimePluginUnloadError, RuntimeReadContextV1, runtime_error_kind,
    runtime_error_message, runtime_plugin_create_runtime_v1, runtime_plugin_destroy_runtime_v1,
    runtime_plugin_on_unload_v1, runtime_plugin_run_tx_v1, runtime_plugin_validate_biz_invariants_v1,
};
use statevec_model::event::{EventKind, GeneratedEventAccess};
use statevec_model::record::{RecordKind, UkCodec};
use statevec_model::{
    EventDefinition, EventSchema, FieldDefinition, FieldType, GeneratedRecordAccess, KeyBytes, RecordDefinition,
    RecordSchema, SchemaRegistry, Version,
};

pub struct Asset;

pub struct AssetAccess<'a> {
    data: &'a [u8],
}

pub struct NewAssetBuilder<'a> {
    data: &'a mut [u8],
}

pub struct UpdateAssetBuilder<'a> {
    data: &'a mut [u8],
}

impl Asset {
    fn uk(asset_id: u64) -> KeyBytes {
        let mut uk = KeyBytes::new();
        uk.extend_from_slice(&asset_id.to_be_bytes());
        uk
    }
}

impl AssetAccess<'_> {
    const LEN: usize = 64;

    fn new(data: &[u8]) -> AssetAccess<'_> {
        AssetAccess { data }
    }

    fn precision(&self) -> u8 {
        self.data[8]
    }
}

impl NewAssetBuilder<'_> {
    fn init_asset_id(&mut self, asset_id: u64) -> &mut Self {
        self.data[0..8].copy_from_slice(&asset_id.to_le_bytes());
        self
    }

    fn set_precision(&mut self, precision: u8) -> &mut Self {
        self.data[8] = precision;
        self
    }
}

impl UpdateAssetBuilder<'_> {
    fn set_precision(&mut self, precision: u8) -> &mut Self {
        self.data[8] = precision;
        self
    }
}

impl RecordSchema for Asset {
    const KIND: RecordKind = 1;
    const RECORD_LEN: usize = 64;
    const FIELD_COUNT: usize = 2;

    fn definition() -> &'static RecordDefinition {
        static FIELDS: [FieldDefinition; 2] = [
            FieldDefinition {
                name: "asset_id",
                field_index: 1,
                offset: 0,
                ty: FieldType::U64,
                len: 8,
                rust_type_name: "u64",
                enum_type_name: None,
                decimal_scale: None,
                semantic: None,
                immutable: true,
            },
            FieldDefinition {
                name: "precision",
                field_index: 2,
                offset: 8,
                ty: FieldType::U8,
                len: 1,
                rust_type_name: "u8",
                enum_type_name: None,
                decimal_scale: None,
                semantic: None,
                immutable: false,
            },
        ];
        static PK_FIELDS: [&str; 1] = ["asset_id"];
        static DEF: RecordDefinition = RecordDefinition {
            kind: Asset::KIND,
            name: "Asset",
            data_size: 64,
            version: 1,
            unique_keys: &[statevec_model::UniqueKeyDefinition {
                id: 0,
                name: "",
                encode: Some(Asset::encode_uk_from_bytes),
                fields: &PK_FIELDS,
            }],
            canonical_indexes: &[],
            fields: &FIELDS,
            reserved_fields: &[],
        };
        &DEF
    }
}

impl UkCodec for Asset {
    fn encode_uk_from_bytes(data: &[u8]) -> KeyBytes {
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(&data[0..8]);
        Asset::uk(u64::from_le_bytes(bytes))
    }
}

impl GeneratedRecordAccess for Asset {
    const DATA_LEN: usize = 64;
    type Access<'a> = AssetAccess<'a>;
    type NewBuilder<'a> = NewAssetBuilder<'a>;
    type UpdateBuilder<'a> = UpdateAssetBuilder<'a>;

    fn wrap<'a>(buf: &'a [u8]) -> Self::Access<'a> {
        AssetAccess::new(buf)
    }

    fn wrap_new<'a>(buf: &'a mut [u8]) -> Self::NewBuilder<'a> {
        NewAssetBuilder { data: buf }
    }

    fn wrap_update<'a>(buf: &'a mut [u8]) -> Self::UpdateBuilder<'a> {
        UpdateAssetBuilder { data: buf }
    }
}

pub struct AssetCreated;

pub mod event {
    pub struct AssetCreatedAccess<'a> {
        data: &'a [u8],
    }

    impl<'a> AssetCreatedAccess<'a> {
        pub fn new(data: &'a [u8]) -> Self {
            Self { data }
        }

        pub fn asset_id(&self) -> u64 {
            let mut bytes = [0u8; 8];
            bytes.copy_from_slice(&self.data[0..8]);
            u64::from_le_bytes(bytes)
        }
    }
}

pub struct AssetCreatedBuilder {
    asset_id: Option<u64>,
}

impl AssetCreatedBuilder {
    fn set_asset_id(mut self, asset_id: u64) -> Self {
        self.asset_id = Some(asset_id);
        self
    }

    fn build(self) -> Vec<u8> {
        self.asset_id.expect("asset_id must be set").to_le_bytes().to_vec()
    }
}

impl EventSchema for AssetCreated {
    const KIND: EventKind = 1;

    fn definition() -> &'static EventDefinition {
        static DEF: EventDefinition = EventDefinition {
            kind: AssetCreated::KIND,
            name: "AssetCreated",
            version: 1,
            fields: &[],
            inline_response: false,
        };
        &DEF
    }
}

impl GeneratedEventAccess for AssetCreated {
    type Access<'a> = event::AssetCreatedAccess<'a>;
    type Builder = AssetCreatedBuilder;

    fn wrap(data: &[u8]) -> Self::Access<'_> {
        event::AssetCreatedAccess::new(data)
    }

    fn builder() -> Self::Builder {
        AssetCreatedBuilder { asset_id: None }
    }
}

fn registry() -> SchemaRegistry {
    SchemaRegistry::new(Version::new(1, 0), &[*Asset::definition()], &[], &[*AssetCreated::definition()], &[])
}

#[derive(Default)]
struct MockRuntimeHostContext {
    next_sys_id: u64,
    records: BTreeMap<(RecordKind, u64), Vec<u8>>,
    events: Vec<(EventKind, Vec<u8>)>,
}

impl MockRuntimeHostContext {
    fn new() -> Self {
        Self { next_sys_id: 1, ..Self::default() }
    }

    fn asset_record_mut_by_uk(&mut self, uk: &[u8]) -> Option<&mut Vec<u8>> {
        self.records.iter_mut().find_map(|((kind, _), data)| {
            if *kind == Asset::KIND && Asset::encode_uk_from_bytes(data).as_slice() == uk { Some(data) } else { None }
        })
    }

    fn asset_record_by_uk(&self, uk: &[u8]) -> Option<&Vec<u8>> {
        self.records.iter().find_map(|((kind, _), data)| {
            if *kind == Asset::KIND && Asset::encode_uk_from_bytes(data).as_slice() == uk { Some(data) } else { None }
        })
    }
}

impl RuntimeHostContext for MockRuntimeHostContext {
    fn with_read_typed_raw(
        &self,
        record_kind: RecordKind,
        sys_id: u64,
        f: &mut dyn FnMut(&[u8]),
    ) -> Result<bool, RuntimeHostError> {
        let Some(data) = self.records.get(&(record_kind, sys_id)) else {
            return Ok(false);
        };
        f(data);
        Ok(true)
    }

    fn with_read_typed_by_uk_raw(
        &self,
        record_kind: RecordKind,
        uk: &[u8],
        f: &mut dyn FnMut(&[u8]),
    ) -> Result<bool, RuntimeHostError> {
        if record_kind != Asset::KIND {
            return Err(RuntimeHostError::new("unexpected record kind"));
        }
        let Some(data) = self.asset_record_by_uk(uk) else {
            return Ok(false);
        };
        f(data);
        Ok(true)
    }

    fn create_typed_raw(
        &mut self,
        record_kind: RecordKind,
        init: &mut dyn FnMut(&mut [u8]),
    ) -> Result<RecordKey, RuntimeHostError> {
        if record_kind != Asset::KIND {
            return Err(RuntimeHostError::new("unexpected record kind"));
        }

        let sys_id = self.next_sys_id;
        self.next_sys_id += 1;

        let mut data = vec![0u8; AssetAccess::LEN];
        init(&mut data);
        self.records.insert((record_kind, sys_id), data);

        Ok(RecordKey { kind: record_kind, sys_id })
    }

    fn update_typed_by_uk_raw(
        &mut self,
        record_kind: RecordKind,
        uk: &[u8],
        f: &mut dyn FnMut(&mut [u8]),
    ) -> Result<bool, RuntimeHostError> {
        if record_kind != Asset::KIND {
            return Err(RuntimeHostError::new("unexpected record kind"));
        }
        let Some(data) = self.asset_record_mut_by_uk(uk) else {
            return Ok(false);
        };
        f(data);
        Ok(true)
    }

    fn delete_by_uk_raw(&mut self, record_kind: RecordKind, uk: &[u8]) -> Result<bool, RuntimeHostError> {
        let Some((&key, _)) = self
            .records
            .iter()
            .find(|((kind, _), data)| *kind == record_kind && Asset::encode_uk_from_bytes(data).as_slice() == uk)
        else {
            return Ok(false);
        };
        self.records.remove(&key);
        Ok(true)
    }

    fn emit_typed_event_raw(&mut self, event_kind: EventKind, payload: &[u8]) -> Result<(), RuntimeHostError> {
        self.events.push((event_kind, payload.to_vec()));
        Ok(())
    }

    fn for_each_record_key_raw(&self, kind: RecordKind, f: &mut dyn FnMut(RecordKey)) -> Result<(), RuntimeHostError> {
        for (record_kind, sys_id) in self.records.keys().filter(|(record_kind, _)| *record_kind == kind) {
            f(RecordKey { kind: *record_kind, sys_id: *sys_id });
        }
        Ok(())
    }

    fn count_index_prefix_capped_raw(
        &self,
        _record_kind: RecordKind,
        _index_id: u8,
        _prefix: &[u8],
        _cap: usize,
    ) -> Result<statevec_api::CanonicalIndexCount, RuntimeHostError> {
        Err(RuntimeHostError::new("mock does not support canonical indexes"))
    }
}

// Covers CE-01..CE-06 and PLG-01..PLG-08 for runtime-host context boundaries.
#[test]
fn runtime_tx_context_is_decoupled_from_engine_tx_access() -> Result<(), RuntimeHostError> {
    let mut raw = MockRuntimeHostContext::new();
    {
        let tx = &mut raw as &mut dyn RuntimeHostContext;

        let created_key = RuntimeHostContextExt::create_typed::<Asset, _>(tx, |builder| {
            builder.init_asset_id(7);
            builder.set_precision(8);
        })?;
        assert_eq!(created_key, RecordKey { kind: Asset::KIND, sys_id: 1 });

        let precision =
            RuntimeHostContextExt::with_read_typed::<Asset, _, _>(tx, created_key.sys_id, |asset| asset.precision())?
                .expect("record missing after create");
        assert_eq!(precision, 8);

        let precision_by_uk =
            RuntimeHostContextExt::with_read_typed_by_uk::<Asset, _, _, _>(tx, Asset::uk(7), |asset| {
                asset.precision()
            })?
            .expect("record missing by uk");
        assert_eq!(precision_by_uk, 8);

        let updated = RuntimeHostContextExt::update_typed_by_uk::<Asset, _, _, _>(tx, Asset::uk(7), |builder| {
            builder.set_precision(9);
            9u8
        })?
        .expect("update should find record");
        assert_eq!(updated, 9);

        let mut listed = Vec::new();
        RuntimeHostContextExt::for_each_record_key(tx, Asset::KIND, &mut |key| listed.push(key))?;
        assert_eq!(listed, vec![created_key]);

        let payload = <AssetCreated as GeneratedEventAccess>::builder().set_asset_id(7).build();
        RuntimeHostContextExt::emit_typed_event::<AssetCreated>(tx, payload)?;

        let deleted = RuntimeHostContextExt::delete_by_uk::<Asset, _>(tx, Asset::uk(7))?;
        assert!(deleted);
    }

    assert_eq!(raw.events.len(), 1);
    assert_eq!(raw.events[0].0, AssetCreated::KIND);
    let event = event::AssetCreatedAccess::new(&raw.events[0].1);
    assert_eq!(event.asset_id(), 7);
    assert!(raw.records.is_empty());

    Ok(())
}

#[test]
fn runtime_host_context_ext_typed_reads_return_none_when_missing() -> Result<(), RuntimeHostError> {
    let mut raw = MockRuntimeHostContext::new();
    let tx = &mut raw as &mut dyn RuntimeHostContext;

    let created_key = RuntimeHostContextExt::create_typed::<Asset, _>(tx, |builder| {
        builder.init_asset_id(9);
        builder.set_precision(3);
    })?;

    let precision =
        RuntimeHostContextExt::with_read_typed::<Asset, _, _>(tx, created_key.sys_id, |asset| asset.precision())?;
    assert_eq!(precision, Some(3));

    let precision_by_uk =
        RuntimeHostContextExt::with_read_typed_by_uk::<Asset, _, _, _>(tx, Asset::uk(9), |asset| asset.precision())?;
    assert_eq!(precision_by_uk, Some(3));

    let missing =
        RuntimeHostContextExt::with_read_typed::<Asset, _, _>(tx, created_key.sys_id + 1, |asset| asset.precision())?;
    assert_eq!(missing, None);

    Ok(())
}

#[derive(Default)]
struct TestFactory;

struct TestPlugin {
    unload_error: bool,
}

impl RuntimePlugin for TestPlugin {
    fn name(&self) -> &'static str {
        "test-plugin"
    }

    fn schema_registry(&self) -> statevec_model::SchemaRegistry {
        registry()
    }

    fn run_tx(
        &self,
        _tx: &mut dyn RuntimeHostContext,
        _command: &dyn RuntimeCommandEnvelope,
    ) -> Result<(), RuntimePluginError> {
        Ok(())
    }

    fn validate_biz_invariants(&self, _ctx: &dyn statevec_api::BizInvariantReadContext) -> Result<(), String> {
        Ok(())
    }

    fn on_unload(&mut self) -> Result<(), RuntimePluginUnloadError> {
        if self.unload_error { Err(RuntimePluginUnloadError::new("unload rejected by test plugin")) } else { Ok(()) }
    }
}

impl RuntimePluginFactory for TestFactory {
    fn plugin_name(&self) -> &'static str {
        "test-plugin"
    }

    fn schema_registry(&self) -> statevec_model::SchemaRegistry {
        registry()
    }

    fn create(&self, plugin_config_text: &str) -> Result<Box<dyn RuntimePlugin>, RuntimePluginLoadError> {
        match plugin_config_text {
            "bad-create" => Err(RuntimePluginLoadError::new("factory rejected config")),
            "unload-error" => Ok(Box::new(TestPlugin { unload_error: true })),
            _ => Ok(Box::new(TestPlugin { unload_error: false })),
        }
    }
}

fn test_factory() -> Box<dyn RuntimePluginFactory> {
    Box::new(TestFactory)
}

fn empty_error() -> RuntimeErrorBuf {
    RuntimeErrorBuf::new(RuntimeErrorPhase::Load, runtime_error_kind::HOST_INTERNAL_ERROR, RuntimeBytesRef::empty())
}

#[test]
fn runtime_plugin_create_runtime_v1_rejects_null_output_pointer() {
    let mut error = empty_error();
    let status = unsafe {
        runtime_plugin_create_runtime_v1(
            test_factory,
            RuntimeBytesRef::from_slice(b""),
            std::ptr::null_mut(),
            &mut error,
        )
    };

    assert_eq!(status, RuntimeCallStatus::Failure);
    assert_eq!(error.phase, RuntimeErrorPhase::Create);
    assert_eq!(error.kind, runtime_error_kind::ABI_CONTRACT_VIOLATION);
    assert!(runtime_error_message(&error).contains("out_runtime is null"));
}

#[test]
fn runtime_plugin_create_runtime_v1_rejects_non_utf8_config() {
    let invalid = [0xffu8];
    let mut runtime = std::ptr::null_mut();
    let mut error = empty_error();
    let status = unsafe {
        runtime_plugin_create_runtime_v1(
            test_factory,
            RuntimeBytesRef { ptr: invalid.as_ptr(), len: invalid.len() },
            &mut runtime,
            &mut error,
        )
    };

    assert_eq!(status, RuntimeCallStatus::Failure);
    assert!(runtime.is_null());
    assert_eq!(error.phase, RuntimeErrorPhase::Create);
    assert_eq!(error.kind, runtime_error_kind::CONFIG_ERROR);
    assert!(runtime_error_message(&error).contains("not valid UTF-8"));
}

#[test]
fn runtime_plugin_create_runtime_v1_surfaces_factory_error() {
    let mut runtime = std::ptr::null_mut();
    let mut error = empty_error();
    let status = unsafe {
        runtime_plugin_create_runtime_v1(
            test_factory,
            RuntimeBytesRef::from_slice(b"bad-create"),
            &mut runtime,
            &mut error,
        )
    };

    assert_eq!(status, RuntimeCallStatus::Failure);
    assert!(runtime.is_null());
    assert_eq!(error.phase, RuntimeErrorPhase::Create);
    assert_eq!(error.kind, runtime_error_kind::CONFIG_ERROR);
    assert!(runtime_error_message(&error).contains("factory rejected config"));
}

#[test]
fn runtime_plugin_run_tx_v1_rejects_null_runtime() {
    let mut error = empty_error();
    let status = unsafe {
        runtime_plugin_run_tx_v1(
            std::ptr::null_mut(),
            statevec_api::RuntimeHostContextV1 { ctx_ptr: std::ptr::null_mut(), vtable: std::ptr::null() },
            RuntimeCommandView {
                command_kind: 1,
                ext_seq: 17,
                ref_ext_time_us: 19,
                payload: RuntimeBytesRef::from_slice(&[]),
            },
            &mut error,
        )
    };

    assert_eq!(status, RuntimeCallStatus::Failure);
    assert_eq!(error.phase, RuntimeErrorPhase::RunTx);
    assert_eq!(error.kind, runtime_error_kind::ABI_CONTRACT_VIOLATION);
    assert!(runtime_error_message(&error).contains("runtime handle is null"));
}

#[test]
fn runtime_plugin_validate_biz_invariants_v1_rejects_null_runtime() {
    let mut error = empty_error();
    let status = unsafe {
        runtime_plugin_validate_biz_invariants_v1(
            std::ptr::null_mut(),
            RuntimeReadContextV1 { ctx_ptr: std::ptr::null_mut(), vtable: std::ptr::null() },
            &mut error,
        )
    };

    assert_eq!(status, RuntimeCallStatus::Failure);
    assert_eq!(error.phase, RuntimeErrorPhase::ValidateBizInvariants);
    assert_eq!(error.kind, runtime_error_kind::ABI_CONTRACT_VIOLATION);
    assert!(runtime_error_message(&error).contains("runtime handle is null"));
}

#[test]
fn runtime_plugin_on_unload_v1_surfaces_plugin_error() {
    let mut runtime = std::ptr::null_mut();
    let mut error = empty_error();
    let status = unsafe {
        runtime_plugin_create_runtime_v1(
            test_factory,
            RuntimeBytesRef::from_slice(b"unload-error"),
            &mut runtime,
            &mut error,
        )
    };
    assert_eq!(status, RuntimeCallStatus::Success);

    let status = unsafe { runtime_plugin_on_unload_v1(runtime, &mut error) };
    assert_eq!(status, RuntimeCallStatus::Failure);
    assert_eq!(error.phase, RuntimeErrorPhase::Unload);
    assert_eq!(error.kind, runtime_error_kind::PLUGIN_INTERNAL_ERROR);
    assert!(runtime_error_message(&error).contains("unload rejected by test plugin"));

    unsafe { runtime_plugin_destroy_runtime_v1(runtime) };
}
