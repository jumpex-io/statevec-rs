// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

//! Lightweight in-memory test host for StateVec domain plugins.
//!
//! This crate is intentionally a unit-test harness, not a runtime engine. It
//! stores record bytes and emitted events in ordinary Rust collections so
//! domain crates can exercise [`statevec_api::RuntimePlugin`] logic without
//! linking the production runtime. It does not implement durable storage,
//! recovery replay, Kafka ingress, snapshot publication, or runtime scheduling
//! semantics. Records and events live in plain Rust collections for the
//! duration of the test.
//!
//! TestHost behaviors that are not part of the public runtime contract:
//!
//! - system ids start at `1` and increment by `1`;
//! - [`TestHost::record_keys`] returns keys in sorted [`RecordKey`] order;
//! - events are returned in emission order with no transaction sequence frame;
//! - host storage errors panic in high-level test helpers, while a real runtime
//!   may surface host failures differently.
//!
//! Tests should assert on domain-level facts, such as record fields, event
//! payloads, and business invariants, rather than these implementation details.

use std::collections::BTreeMap;
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};

use statevec_api::{
    BizInvariantReadContext, InvariantReadContextExt, RecordKey, RuntimeCommandRef,
    RuntimeHostContext, RuntimeHostContextExt, RuntimeHostError, RuntimePlugin, RuntimePluginError,
};
use statevec_model::SchemaRegistry;
use statevec_model::command::GeneratedCommandAccess;
use statevec_model::event::GeneratedEventAccess;
use statevec_model::record::{GeneratedRecordAccess, RecordKind, SysId};

/// Event payload captured by [`TestHost`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestEvent {
    /// Stable event kind from the domain schema.
    pub event_kind: u8,
    /// Encoded event payload bytes.
    pub payload: Vec<u8>,
}

/// Typed borrowed view over one captured event payload.
#[derive(Debug, Clone, Copy)]
pub struct EventView<'a, E> {
    payload: &'a [u8],
    _marker: PhantomData<E>,
}

impl<'a, E> EventView<'a, E>
where
    E: GeneratedEventAccess,
{
    /// Returns the encoded event payload bytes.
    pub fn payload(&self) -> &'a [u8] {
        self.payload
    }

    /// Reads this event through its generated typed accessor.
    pub fn read<T>(&self, f: impl FnOnce(E::Access<'_>) -> T) -> T {
        f(E::wrap(self.payload))
    }
}

/// A [`TestHost`] with a plugin bound to it.
///
/// This is the ergonomic API for most business tests. It owns the plugin,
/// assigns `ext_seq` automatically, and lets commands be run by command type so
/// tests do not repeat `KIND` values by hand.
#[derive(Debug, Clone)]
pub struct PluginTestHost<P> {
    inner: TestHost,
    plugin: P,
    next_ext_seq: u64,
    ref_time_us: u64,
}

impl<P> PluginTestHost<P>
where
    P: RuntimePlugin,
{
    /// Sets the reference external time used by subsequent [`run`](Self::run)
    /// calls.
    pub fn with_ref_time(mut self, ref_time_us: u64) -> Self {
        self.ref_time_us = ref_time_us;
        self
    }

    /// Sets the next source sequence used by subsequent [`run`](Self::run)
    /// calls.
    pub fn with_starting_ext_seq(mut self, ext_seq: u64) -> Self {
        self.next_ext_seq = ext_seq;
        self
    }

    /// Returns the bound plugin.
    pub fn plugin(&self) -> &P {
        &self.plugin
    }

    /// Returns the underlying low-level test host.
    pub fn inner(&self) -> &TestHost {
        &self.inner
    }

    /// Returns the underlying low-level test host mutably.
    pub fn inner_mut(&mut self) -> &mut TestHost {
        &mut self.inner
    }

    /// Consumes this wrapper and returns the underlying low-level host.
    pub fn into_inner(self) -> TestHost {
        self.inner
    }

    /// Runs one typed command payload.
    ///
    /// The command kind comes from `C::KIND`, `ext_seq` increments
    /// automatically, and `ref_time_us` uses the value configured on the host.
    pub fn run<C>(&mut self, payload: impl AsRef<[u8]>) -> Result<(), RuntimePluginError>
    where
        C: GeneratedCommandAccess,
    {
        let ext_seq = self.next_ext_seq;
        self.next_ext_seq = self.next_ext_seq.saturating_add(1);
        self.run_with_envelope::<C>(ext_seq, self.ref_time_us, payload)
    }

    /// Runs one typed command payload with explicit envelope fields.
    pub fn run_with_envelope<C>(
        &mut self,
        ext_seq: u64,
        ref_time_us: u64,
        payload: impl AsRef<[u8]>,
    ) -> Result<(), RuntimePluginError>
    where
        C: GeneratedCommandAccess,
    {
        let command = RuntimeCommandRef::new(C::KIND, ext_seq, ref_time_us, payload.as_ref());
        let snapshot = self.inner.clone();
        match self.plugin.run_tx(&mut self.inner, &command) {
            Ok(()) => Ok(()),
            Err(err) => {
                self.inner = snapshot;
                Err(err)
            }
        }
    }

    /// Runs business invariant validation through the bound plugin.
    pub fn validate_invariants(&self) -> Result<(), String> {
        self.plugin.validate_biz_invariants(&self.inner)
    }
}

impl<P> Deref for PluginTestHost<P> {
    type Target = TestHost;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<P> DerefMut for PluginTestHost<P> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

/// Minimal in-memory host context for plugin unit tests.
///
/// `TestHost` implements both [`RuntimeHostContext`] and
/// [`BizInvariantReadContext`]. It uses the supplied [`SchemaRegistry`] to size
/// new records and maintain primary-key lookup indexes.
#[derive(Debug, Clone)]
pub struct TestHost {
    registry: SchemaRegistry,
    next_sys_id: SysId,
    records: BTreeMap<RecordKey, Vec<u8>>,
    pk_index: BTreeMap<(RecordKind, Vec<u8>), SysId>,
    events: Vec<TestEvent>,
    debug_logs: Vec<String>,
}

impl TestHost {
    /// Creates an empty test host for a domain schema registry.
    pub fn new(registry: SchemaRegistry) -> Self {
        Self {
            registry,
            next_sys_id: 1,
            records: BTreeMap::new(),
            pk_index: BTreeMap::new(),
            events: Vec::new(),
            debug_logs: Vec::new(),
        }
    }

    /// Creates a plugin-bound test host.
    pub fn for_plugin<P>(plugin: P) -> PluginTestHost<P>
    where
        P: RuntimePlugin,
    {
        PluginTestHost {
            inner: Self::new(plugin.schema_registry()),
            plugin,
            next_ext_seq: 1,
            ref_time_us: 0,
        }
    }

    /// Returns the schema registry used by this host.
    pub fn schema_registry(&self) -> &SchemaRegistry {
        &self.registry
    }

    /// Returns the number of materialized records.
    pub fn record_count(&self) -> usize {
        self.records.len()
    }

    /// Returns captured events in emission order.
    pub fn events(&self) -> &[TestEvent] {
        &self.events
    }

    /// Removes and returns captured events.
    pub fn take_events(&mut self) -> Vec<TestEvent> {
        std::mem::take(&mut self.events)
    }

    /// Clears captured events.
    pub fn clear_events(&mut self) {
        self.events.clear();
    }

    /// Returns host-side debug log messages captured in order.
    pub fn debug_logs(&self) -> &[String] {
        &self.debug_logs
    }

    /// Runs one command transaction against a plugin.
    pub fn run_tx(
        &mut self,
        plugin: &dyn RuntimePlugin,
        command: &dyn statevec_api::RuntimeCommandEnvelope,
    ) -> Result<(), RuntimePluginError> {
        plugin.run_tx(self, command)
    }

    /// Runs one encoded command payload against a plugin.
    pub fn run_command_payload(
        &mut self,
        plugin: &dyn RuntimePlugin,
        command_kind: u8,
        ext_seq: u64,
        ref_ext_time_us: u64,
        payload: &[u8],
    ) -> Result<(), RuntimePluginError> {
        let command = RuntimeCommandRef::new(command_kind, ext_seq, ref_ext_time_us, payload);
        self.run_tx(plugin, &command)
    }

    /// Runs a plugin's business invariant validation against this host.
    pub fn validate_biz_invariants(&self, plugin: &dyn RuntimePlugin) -> Result<(), String> {
        plugin.validate_biz_invariants(self)
    }

    /// Reads a generated record by primary-key bytes.
    ///
    /// This high-level helper panics on host implementation errors and returns
    /// `None` only when the record is absent. Prefer this method in business
    /// tests over the `_raw` trait methods.
    pub fn read<R, T>(&self, pk: impl AsRef<[u8]>, f: impl FnOnce(R::Access<'_>) -> T) -> Option<T>
    where
        R: GeneratedRecordAccess,
    {
        self.read_typed_by_pk::<R, _, _, _>(pk, f)
            .expect("test host failed to read record by primary key")
    }

    /// Reads a generated record by primary-key bytes or panics when absent.
    pub fn expect<R, T>(&self, pk: impl AsRef<[u8]>, f: impl FnOnce(R::Access<'_>) -> T) -> T
    where
        R: GeneratedRecordAccess,
    {
        self.read::<R, T>(pk, f)
            .expect("expected record to exist in test host")
    }

    /// Counts records of one generated record type.
    pub fn count<R>(&self) -> usize
    where
        R: GeneratedRecordAccess,
    {
        self.records
            .keys()
            .filter(|key| key.kind == R::KIND)
            .count()
    }

    /// Reads all records of one generated record type in system id order.
    pub fn all<R, T>(&self, f: impl Fn(R::Access<'_>) -> T) -> Vec<T>
    where
        R: GeneratedRecordAccess,
    {
        self.record_keys(R::KIND)
            .into_iter()
            .filter_map(|key| {
                self.read_typed::<R, _, _>(key.sys_id, &f)
                    .expect("test host failed to read record by system id")
            })
            .collect()
    }

    /// Reads a generated record by system id.
    pub fn read_typed<R, T, F>(&self, sys_id: SysId, f: F) -> Result<Option<T>, RuntimeHostError>
    where
        R: GeneratedRecordAccess,
        F: FnOnce(R::Access<'_>) -> T,
    {
        RuntimeHostContextExt::with_read_typed::<R, T, F>(self, sys_id, f)
    }

    /// Reads a generated record by primary-key bytes.
    pub fn read_typed_by_pk<R, P, T, F>(&self, pk: P, f: F) -> Result<Option<T>, RuntimeHostError>
    where
        R: GeneratedRecordAccess,
        P: AsRef<[u8]>,
        F: FnOnce(R::Access<'_>) -> T,
    {
        RuntimeHostContextExt::with_read_typed_by_pk::<R, P, T, F>(self, pk, f)
    }

    /// Reads an emitted generated event by index.
    pub fn read_event<E, T, F>(&self, index: usize, f: F) -> Option<T>
    where
        E: GeneratedEventAccess,
        F: FnOnce(E::Access<'_>) -> T,
    {
        let event = self.events.get(index)?;
        if event.event_kind != E::KIND {
            return None;
        }
        Some(f(E::wrap(&event.payload)))
    }

    /// Returns captured events of one generated event type.
    pub fn events_of<E>(&self) -> Vec<EventView<'_, E>>
    where
        E: GeneratedEventAccess,
    {
        self.events
            .iter()
            .filter(|event| event.event_kind == E::KIND)
            .map(|event| EventView {
                payload: event.payload.as_slice(),
                _marker: PhantomData,
            })
            .collect()
    }

    /// Returns the last captured event of one generated event type.
    pub fn last_event_of<E>(&self) -> Option<EventView<'_, E>>
    where
        E: GeneratedEventAccess,
    {
        self.events
            .iter()
            .rev()
            .find(|event| event.event_kind == E::KIND)
            .map(|event| EventView {
                payload: event.payload.as_slice(),
                _marker: PhantomData,
            })
    }

    /// Counts captured events of one generated event type.
    pub fn count_events_of<E>(&self) -> usize
    where
        E: GeneratedEventAccess,
    {
        self.events
            .iter()
            .filter(|event| event.event_kind == E::KIND)
            .count()
    }

    /// Finds a captured event of one type matching a predicate, or panics.
    pub fn expect_event<E, F>(&self, predicate: F) -> EventView<'_, E>
    where
        E: GeneratedEventAccess,
        F: for<'p> Fn(E::Access<'p>) -> bool,
    {
        self.events_of::<E>()
            .into_iter()
            .find(|event| predicate(E::wrap(event.payload)))
            .expect("expected matching event in test host")
    }

    /// Iterates visible record keys for one record kind.
    pub fn record_keys(&self, kind: RecordKind) -> Vec<RecordKey> {
        self.records
            .keys()
            .copied()
            .filter(|key| key.kind == kind)
            .collect()
    }

    fn record_len(&self, record_kind: RecordKind) -> Result<usize, RuntimeHostError> {
        let Some(def) = self.registry.try_get(record_kind) else {
            return Err(RuntimeHostError::new(format!(
                "unknown record kind {record_kind}"
            )));
        };
        Ok(def.data_size as usize)
    }

    fn encode_pk(
        &self,
        record_kind: RecordKind,
        data: &[u8],
    ) -> Result<Option<Vec<u8>>, RuntimeHostError> {
        if self.registry.try_get(record_kind).is_none() {
            return Err(RuntimeHostError::new(format!(
                "unknown record kind {record_kind}"
            )));
        }
        Ok(self
            .registry
            .encode_pk(record_kind, data)
            .map(|pk| pk.as_slice().to_vec()))
    }

    fn resolve_pk(&self, record_kind: RecordKind, pk: &[u8]) -> Option<RecordKey> {
        let sys_id = self.pk_index.get(&(record_kind, pk.to_vec())).copied()?;
        Some(RecordKey {
            kind: record_kind,
            sys_id,
        })
    }

    fn ensure_pk_is_free(
        &self,
        record_kind: RecordKind,
        pk: &[u8],
        current_sys_id: Option<SysId>,
    ) -> Result<(), RuntimeHostError> {
        let existing = self.pk_index.get(&(record_kind, pk.to_vec())).copied();
        if existing.is_some() && existing != current_sys_id {
            return Err(RuntimeHostError::new(format!(
                "duplicate primary key for record kind {record_kind}"
            )));
        }
        Ok(())
    }
}

impl RuntimeHostContext for TestHost {
    fn with_read_typed_raw(
        &self,
        record_kind: RecordKind,
        sys_id: SysId,
        f: &mut dyn FnMut(&[u8]),
    ) -> Result<bool, RuntimeHostError> {
        let key = RecordKey {
            kind: record_kind,
            sys_id,
        };
        let Some(data) = self.records.get(&key) else {
            return Ok(false);
        };
        f(data);
        Ok(true)
    }

    fn with_read_typed_by_pk_raw(
        &self,
        record_kind: RecordKind,
        pk: &[u8],
        f: &mut dyn FnMut(&[u8]),
    ) -> Result<bool, RuntimeHostError> {
        let Some(key) = self.resolve_pk(record_kind, pk) else {
            return Ok(false);
        };
        let Some(data) = self.records.get(&key) else {
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
        let mut data = vec![0u8; self.record_len(record_kind)?];
        init(&mut data);

        let pk = self.encode_pk(record_kind, &data)?;
        if let Some(pk) = pk.as_deref() {
            self.ensure_pk_is_free(record_kind, pk, None)?;
        }

        let sys_id = self.next_sys_id;
        self.next_sys_id += 1;
        let key = RecordKey {
            kind: record_kind,
            sys_id,
        };

        if let Some(pk) = pk {
            self.pk_index.insert((record_kind, pk), sys_id);
        }
        self.records.insert(key, data);
        Ok(key)
    }

    fn update_typed_by_pk_raw(
        &mut self,
        record_kind: RecordKind,
        pk: &[u8],
        f: &mut dyn FnMut(&mut [u8]),
    ) -> Result<bool, RuntimeHostError> {
        let Some(key) = self.resolve_pk(record_kind, pk) else {
            return Ok(false);
        };

        let old_data = self
            .records
            .get(&key)
            .ok_or_else(|| RuntimeHostError::new("primary-key index points to no record"))?;
        let old_pk = self.encode_pk(record_kind, old_data)?;
        let mut new_data = old_data.clone();
        f(&mut new_data);
        let new_pk = self.encode_pk(record_kind, &new_data)?;

        if old_pk != new_pk {
            if let Some(new_pk) = new_pk.as_deref() {
                self.ensure_pk_is_free(record_kind, new_pk, Some(key.sys_id))?;
            }
            if let Some(old_pk) = old_pk {
                self.pk_index.remove(&(record_kind, old_pk));
            }
            if let Some(new_pk) = new_pk {
                self.pk_index.insert((record_kind, new_pk), key.sys_id);
            }
        }

        self.records.insert(key, new_data);
        Ok(true)
    }

    fn delete_by_pk_raw(
        &mut self,
        record_kind: RecordKind,
        pk: &[u8],
    ) -> Result<bool, RuntimeHostError> {
        let Some(key) = self.resolve_pk(record_kind, pk) else {
            return Ok(false);
        };
        let Some(data) = self.records.remove(&key) else {
            self.pk_index.remove(&(record_kind, pk.to_vec()));
            return Ok(false);
        };
        if let Some(encoded_pk) = self.encode_pk(record_kind, &data)? {
            self.pk_index.remove(&(record_kind, encoded_pk));
        } else {
            self.pk_index.remove(&(record_kind, pk.to_vec()));
        }
        Ok(true)
    }

    fn emit_typed_event_raw(
        &mut self,
        event_kind: u8,
        payload: &[u8],
    ) -> Result<(), RuntimeHostError> {
        self.events.push(TestEvent {
            event_kind,
            payload: payload.to_vec(),
        });
        Ok(())
    }

    fn for_each_record_key_raw(
        &self,
        kind: RecordKind,
        f: &mut dyn FnMut(RecordKey),
    ) -> Result<(), RuntimeHostError> {
        for key in self.records.keys().copied().filter(|key| key.kind == kind) {
            f(key);
        }
        Ok(())
    }

    fn debug_log(&mut self, message: String) -> Result<(), RuntimeHostError> {
        self.debug_logs.push(message);
        Ok(())
    }
}

impl BizInvariantReadContext for TestHost {
    fn with_read_typed_raw(
        &self,
        record_kind: RecordKind,
        sys_id: SysId,
        f: &mut dyn FnMut(&[u8]),
    ) -> Result<bool, RuntimeHostError> {
        RuntimeHostContext::with_read_typed_raw(self, record_kind, sys_id, f)
    }

    fn with_read_typed_by_pk_raw(
        &self,
        record_kind: RecordKind,
        pk: &[u8],
        f: &mut dyn FnMut(&[u8]),
    ) -> Result<bool, RuntimeHostError> {
        RuntimeHostContext::with_read_typed_by_pk_raw(self, record_kind, pk, f)
    }

    fn for_each_record_key_raw(
        &self,
        kind: RecordKind,
        f: &mut dyn FnMut(RecordKey),
    ) -> Result<(), RuntimeHostError> {
        RuntimeHostContext::for_each_record_key_raw(self, kind, f)
    }
}

/// Reads a generated record from any invariant context by primary-key bytes.
pub fn read_invariant_by_pk<C, R, P, T, F>(
    ctx: &C,
    pk: P,
    f: F,
) -> Result<Option<T>, RuntimeHostError>
where
    C: BizInvariantReadContext + ?Sized,
    R: GeneratedRecordAccess,
    P: AsRef<[u8]>,
    F: FnOnce(R::Access<'_>) -> T,
{
    InvariantReadContextExt::with_read_typed_by_pk::<R, P, T, F>(ctx, pk, f)
}

#[cfg(test)]
mod tests {
    use statevec_api::{RuntimeCommandEnvelope, RuntimeHostContextExt};
    use statevec_model::event::{EventSchema, GeneratedEventAccess};
    use statevec_model::record::PkCodec;
    use statevec_model::{
        CommandDefinition, CommandSchema, EventDefinition, FieldDefinition, FieldType,
        GeneratedCommandAccess, GeneratedRecordAccess, PkBytes, RecordDefinition, RecordSchema,
        Version,
    };

    use super::*;

    struct Asset;

    struct AssetAccess<'a> {
        data: &'a [u8],
    }

    struct NewAssetBuilder<'a> {
        data: &'a mut [u8],
    }

    struct UpdateAssetBuilder<'a> {
        data: &'a mut [u8],
    }

    impl Asset {
        fn pk(asset_id: u64) -> PkBytes {
            let mut pk = PkBytes::new();
            pk.extend_from_slice(&asset_id.to_be_bytes());
            pk
        }
    }

    impl AssetAccess<'_> {
        fn asset_id(&self) -> u64 {
            let mut bytes = [0u8; 8];
            bytes.copy_from_slice(&self.data[0..8]);
            u64::from_le_bytes(bytes)
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
        const KIND: u8 = 1;
        const RECORD_LEN: usize = 16;
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
                    immutable: false,
                },
            ];
            static PK_FIELDS: [&str; 1] = ["asset_id"];
            static DEF: RecordDefinition = RecordDefinition {
                kind: Asset::KIND,
                name: "Asset",
                is_pk_idx: true,
                support_range_scan: false,
                data_size: 16,
                version: 1,
                pk_encode: Some(Asset::encode_pk_from_bytes),
                fields: &FIELDS,
                reserved_fields: &[],
                pk_fields: &PK_FIELDS,
            };
            &DEF
        }
    }

    impl PkCodec for Asset {
        fn encode_pk_from_bytes(data: &[u8]) -> PkBytes {
            let mut bytes = [0u8; 8];
            bytes.copy_from_slice(&data[0..8]);
            Asset::pk(u64::from_le_bytes(bytes))
        }
    }

    impl GeneratedRecordAccess for Asset {
        const DATA_LEN: usize = 16;
        type Access<'a> = AssetAccess<'a>;
        type NewBuilder<'a> = NewAssetBuilder<'a>;
        type UpdateBuilder<'a> = UpdateAssetBuilder<'a>;

        fn wrap<'a>(buf: &'a [u8]) -> Self::Access<'a> {
            AssetAccess { data: buf }
        }

        fn wrap_new<'a>(buf: &'a mut [u8]) -> Self::NewBuilder<'a> {
            NewAssetBuilder { data: buf }
        }

        fn wrap_update<'a>(buf: &'a mut [u8]) -> Self::UpdateBuilder<'a> {
            UpdateAssetBuilder { data: buf }
        }
    }

    struct AssetCreated;

    struct AssetCreatedAccess<'a> {
        data: &'a [u8],
    }

    impl AssetCreatedAccess<'_> {
        fn asset_id(&self) -> u64 {
            let mut bytes = [0u8; 8];
            bytes.copy_from_slice(&self.data[0..8]);
            u64::from_le_bytes(bytes)
        }
    }

    struct AssetCreatedBuilder {
        asset_id: Option<u64>,
    }

    impl AssetCreatedBuilder {
        fn set_asset_id(mut self, asset_id: u64) -> Self {
            self.asset_id = Some(asset_id);
            self
        }

        fn build(self) -> Vec<u8> {
            self.asset_id
                .expect("asset_id must be set")
                .to_le_bytes()
                .to_vec()
        }
    }

    impl EventSchema for AssetCreated {
        const KIND: u8 = 1;

        fn definition() -> &'static EventDefinition {
            static DEF: EventDefinition = EventDefinition {
                kind: AssetCreated::KIND,
                name: "AssetCreated",
                version: 1,
                fields: &[],
            };
            &DEF
        }
    }

    impl GeneratedEventAccess for AssetCreated {
        type Access<'a> = AssetCreatedAccess<'a>;
        type Builder = AssetCreatedBuilder;

        fn wrap(data: &[u8]) -> Self::Access<'_> {
            AssetCreatedAccess { data }
        }

        fn builder() -> Self::Builder {
            AssetCreatedBuilder { asset_id: None }
        }
    }

    struct AssetCommand;

    struct AssetCommandAccess<'a> {
        _data: &'a [u8],
    }

    impl CommandSchema for AssetCommand {
        const KIND: u8 = 9;

        fn definition() -> &'static CommandDefinition {
            static DEF: CommandDefinition = CommandDefinition {
                kind: AssetCommand::KIND,
                name: "AssetCommand",
                version: 1,
                fields: &[],
            };
            &DEF
        }
    }

    impl GeneratedCommandAccess for AssetCommand {
        type Access<'a> = AssetCommandAccess<'a>;
        type Builder = ();

        fn wrap(data: &[u8]) -> Self::Access<'_> {
            AssetCommandAccess { _data: data }
        }

        fn builder() -> Self::Builder {}
    }

    fn registry() -> SchemaRegistry {
        SchemaRegistry::new(
            Version::new(1, 0),
            &[*Asset::definition()],
            &[],
            &[*AssetCreated::definition()],
            &[],
        )
    }

    #[test]
    fn create_read_update_and_delete_record_by_primary_key() {
        let mut host = TestHost::new(registry());

        let key = host
            .create_typed::<Asset, _>(|asset| {
                asset.init_asset_id(42).set_precision(6);
            })
            .unwrap();
        assert_eq!(key.sys_id, 1);

        let asset = host
            .read_typed_by_pk::<Asset, _, _, _>(Asset::pk(42), |asset| {
                (asset.asset_id(), asset.precision())
            })
            .unwrap();
        assert_eq!(asset, Some((42, 6)));

        let updated = host
            .update_typed_by_pk::<Asset, _, _, _>(Asset::pk(42), |asset| {
                asset.set_precision(8);
                8
            })
            .unwrap();
        assert_eq!(updated, Some(8));
        assert_eq!(
            host.read_typed::<Asset, _, _>(key.sys_id, |asset| asset.precision())
                .unwrap(),
            Some(8)
        );

        assert!(host.delete_by_pk::<Asset, _>(Asset::pk(42)).unwrap());
        assert_eq!(host.record_count(), 0);
    }

    #[test]
    fn duplicate_primary_key_is_rejected() {
        let mut host = TestHost::new(registry());
        host.create_typed::<Asset, _>(|asset| {
            asset.init_asset_id(42).set_precision(6);
        })
        .unwrap();

        let err = host
            .create_typed::<Asset, _>(|asset| {
                asset.init_asset_id(42).set_precision(8);
            })
            .unwrap_err();
        assert!(err.message.contains("duplicate primary key"));
        assert_eq!(host.record_count(), 1);
    }

    #[test]
    fn failed_primary_key_change_does_not_commit_partial_update() {
        let mut host = TestHost::new(registry());
        host.create_typed::<Asset, _>(|asset| {
            asset.init_asset_id(1).set_precision(6);
        })
        .unwrap();
        host.create_typed::<Asset, _>(|asset| {
            asset.init_asset_id(2).set_precision(8);
        })
        .unwrap();

        let err = RuntimeHostContext::update_typed_by_pk_raw(
            &mut host,
            Asset::KIND,
            Asset::pk(1).as_slice(),
            &mut |data| {
                data[0..8].copy_from_slice(&2u64.to_le_bytes());
                data[8] = 9;
            },
        )
        .unwrap_err();

        assert!(err.message.contains("duplicate primary key"));
        let asset = host
            .read_typed_by_pk::<Asset, _, _, _>(Asset::pk(1), |asset| {
                (asset.asset_id(), asset.precision())
            })
            .unwrap();
        assert_eq!(asset, Some((1, 6)));
    }

    #[test]
    fn captures_and_reads_typed_events() {
        let mut host = TestHost::new(registry());
        let payload = AssetCreated::builder().set_asset_id(7).build();

        host.emit_typed_event::<AssetCreated>(payload).unwrap();

        assert_eq!(host.events().len(), 1);
        assert_eq!(
            host.read_event::<AssetCreated, _, _>(0, |event| event.asset_id()),
            Some(7)
        );
    }

    #[test]
    fn invariant_context_can_iterate_and_read_records() {
        let mut host = TestHost::new(registry());
        host.create_typed::<Asset, _>(|asset| {
            asset.init_asset_id(42).set_precision(6);
        })
        .unwrap();

        let mut keys = Vec::new();
        BizInvariantReadContext::for_each_record_key_raw(&host, Asset::KIND, &mut |key| {
            keys.push(key);
        })
        .unwrap();

        assert_eq!(keys.len(), 1);
        let precision = read_invariant_by_pk::<_, Asset, _, _, _>(&host, Asset::pk(42), |asset| {
            asset.precision()
        })
        .unwrap();
        assert_eq!(precision, Some(6));
    }

    struct EchoPlugin;

    impl RuntimePlugin for EchoPlugin {
        fn name(&self) -> &'static str {
            "echo"
        }

        fn schema_registry(&self) -> SchemaRegistry {
            registry()
        }

        fn run_tx(
            &self,
            tx: &mut dyn RuntimeHostContext,
            command: &dyn RuntimeCommandEnvelope,
        ) -> Result<(), RuntimePluginError> {
            tx.debug_log(format!("command_kind={}", command.command_kind()))
                .map_err(|err| RuntimePluginError::new(err.message))?;
            tx.emit_typed_event_raw(AssetCreated::KIND, command.payload())
                .map_err(|err| RuntimePluginError::new(err.message))
        }

        fn validate_biz_invariants(&self, ctx: &dyn BizInvariantReadContext) -> Result<(), String> {
            let mut count = 0;
            ctx.for_each_record_key_raw(Asset::KIND, &mut |_| {
                count += 1;
            })
            .map_err(|err| err.message)?;
            if count == 0 {
                Err("expected at least one asset".to_string())
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn can_run_plugin_command_payload_and_validate_invariants() {
        let mut host = TestHost::for_plugin(EchoPlugin);

        host.create_typed::<Asset, _>(|asset| {
            asset.init_asset_id(42).set_precision(6);
        })
        .unwrap();
        let payload = 42u64.to_le_bytes();

        host.run_with_envelope::<AssetCommand>(10, 11, payload)
            .unwrap();

        assert_eq!(host.debug_logs(), &["command_kind=9".to_string()]);
        assert_eq!(
            host.last_event_of::<AssetCreated>()
                .map(|event| event.read(|event| event.asset_id())),
            Some(42)
        );
        assert!(
            host.expect_event::<AssetCreated, _>(|event| event.asset_id() == 42)
                .payload()
                == payload
        );
        host.validate_invariants().unwrap();
    }
}
