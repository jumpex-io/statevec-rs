// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

//! Simplified in-memory execution engine for local business tests.
//!
//! Records, unique keys and emitted events live in ordinary Rust collections.
//! Business handlers can use the typed transaction traits directly, or run
//! through [`statevec_api::RuntimePlugin`]. Transactions restore their prior
//! state on errors and unwind. Production durability, replication, recovery
//! and scheduling are supplied by the platform runtime.
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
//! payloads, and typed business rejections.

use std::collections::{BTreeMap, BTreeSet};
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};

use statevec_api::{
    BizInvariantReadContext, CanonicalIndexCount, InvariantReadContextExt, RecordKey,
    RuntimeCommandRef, RuntimeHostContext, RuntimeHostContextExt, RuntimeHostError, RuntimePlugin,
    RuntimePluginError, TxReadContext, TxSysIdCreateContext, TxUkContext, TxWriteContext,
};
use statevec_model::SchemaRegistry;
use statevec_model::command::GeneratedCommandAccess;
use statevec_model::event::GeneratedEventAccess;
use statevec_model::record::{GeneratedRecordAccess, RecordKind, SysId};

/// Event payload captured by [`TestHost`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestEvent {
    /// Stable event kind from the domain schema.
    pub event_kind: statevec_model::EventKind,
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
        self.next_ext_seq = self.next_ext_seq.checked_add(1)
            .ok_or_else(|| RuntimePluginError::new("test source sequence exhausted"))?;
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
        C::validate_payload(payload.as_ref())
            .map_err(|error| RuntimePluginError::new(format!("invalid command payload: {error:?}")))?;
        let command = RuntimeCommandRef::new(C::KIND, ext_seq, ref_time_us, payload.as_ref());
        self.inner.run_tx(&self.plugin, &command)
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
/// new records and maintain unique-key lookup indexes.
#[derive(Debug, Clone)]
pub struct TestHost {
    registry: SchemaRegistry,
    next_sys_id: SysId,
    records: BTreeMap<RecordKey, Vec<u8>>,
    uk_index: BTreeMap<(RecordKind, u8, Vec<u8>), SysId>,
    allocated_ids: BTreeSet<SysId>,
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
            uk_index: BTreeMap::new(),
            allocated_ids: BTreeSet::new(),
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
        self.transaction(|host| plugin.run_tx(host, command))
    }

    /// Runs one encoded command payload against a plugin.
    pub fn run_command_payload(
        &mut self,
        plugin: &dyn RuntimePlugin,
        command_kind: statevec_model::CommandKind,
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

    /// Reads a generated record by unique-key bytes.
    ///
    /// This high-level helper panics on host implementation errors and returns
    /// `None` only when the record is absent. Prefer this method in business
    /// tests over the `_raw` trait methods.
    pub fn read<R, T>(&self, uk: impl AsRef<[u8]>, f: impl FnOnce(R::Access<'_>) -> T) -> Option<T>
    where
        R: GeneratedRecordAccess,
    {
        self.read_typed_by_uk::<R, _, _, _>(uk, f)
            .expect("test host failed to read record by unique key")
    }

    /// Reads a generated record by unique-key bytes or panics when absent.
    pub fn expect<R, T>(&self, uk: impl AsRef<[u8]>, f: impl FnOnce(R::Access<'_>) -> T) -> T
    where
        R: GeneratedRecordAccess,
    {
        self.read::<R, T>(uk, f)
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

    /// Reads a generated record by unique-key bytes.
    pub fn read_typed_by_uk<R, P, T, F>(&self, uk: P, f: F) -> Result<Option<T>, RuntimeHostError>
    where
        R: GeneratedRecordAccess,
        P: AsRef<[u8]>,
        F: FnOnce(R::Access<'_>) -> T,
    {
        RuntimeHostContextExt::with_read_typed_by_uk::<R, P, T, F>(self, uk, f)
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

    fn encode_unique_keys(
        &self,
        kind: RecordKind,
        data: &[u8],
    ) -> Result<Vec<(u8, Vec<u8>)>, RuntimeHostError> {
        let definition = self.registry.try_get(kind)
            .ok_or_else(|| RuntimeHostError::new(format!("unknown record kind {kind}")))?;
        if data.len() != definition.data_size as usize {
            return Err(RuntimeHostError::new("record length does not match schema"));
        }
        statevec_model::reserved_bytes::validate_reserved_bytes_zero(definition, data)
            .map_err(|error| RuntimeHostError::new(format!("invalid reserved bytes: {error:?}")))?;
        if definition.unique_keys.is_empty() {
            return Ok(Vec::new());
        }
        self.registry.encode_unique_keys(kind, data)
            .map(|keys| keys.into_iter().map(|key| (key.uk_id, key.bytes.to_vec())).collect())
            .ok_or_else(|| RuntimeHostError::new("cannot encode unique keys"))
    }

    fn resolve_uk(&self, kind: RecordKind, uk: &[u8]) -> Option<RecordKey> {
        self.uk_index.get(&(kind, 0, uk.to_vec())).copied()
            .map(|sys_id| RecordKey { kind, sys_id })
    }

    /// Runs a business operation with rollback of records, keys, ids and events
    /// on an error or unwind. No persistence or replication is involved.
    pub fn transaction<T, E>(
        &mut self,
        operation: impl FnOnce(&mut Self) -> Result<T, E>,
    ) -> Result<T, E> {
        let before = self.clone();
        match catch_unwind(AssertUnwindSafe(|| operation(self))) {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(error)) => {
                *self = before;
                Err(error)
            }
            Err(panic) => {
                *self = before;
                resume_unwind(panic)
            }
        }
    }

    /// Reads through a specific unique key, including secondary keys.
    pub fn read_by_uk_id<R, T>(
        &self,
        id: u8,
        uk: impl AsRef<[u8]>,
        read: impl FnOnce(R::Access<'_>) -> T,
    ) -> Option<T>
    where
        R: GeneratedRecordAccess,
    {
        let sys_id = TxUkContext::resolve_uk_id(self, R::KIND, id, uk.as_ref())
            .expect("test key lookup");
        sys_id.and_then(|sys_id| {
            self.read_typed::<R, _, _>(sys_id, read).expect("test record lookup")
        })
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

    fn with_read_typed_by_uk_raw(
        &self,
        record_kind: RecordKind,
        uk: &[u8],
        f: &mut dyn FnMut(&[u8]),
    ) -> Result<bool, RuntimeHostError> {
        let Some(key) = self.resolve_uk(record_kind, uk) else {
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

        TxWriteContext::create_raw(self, record_kind, data)
    }

    fn update_typed_by_uk_raw(
        &mut self,
        record_kind: RecordKind,
        uk: &[u8],
        f: &mut dyn FnMut(&mut [u8]),
    ) -> Result<bool, RuntimeHostError> {
        let Some(key) = self.resolve_uk(record_kind, uk) else {
            return Ok(false);
        };

        TxWriteContext::update_raw(self, key, |data| f(data)).map(|result| result.is_some())
    }

    fn delete_by_uk_raw(
        &mut self,
        record_kind: RecordKind,
        uk: &[u8],
    ) -> Result<bool, RuntimeHostError> {
        let Some(key) = self.resolve_uk(record_kind, uk) else {
            return Ok(false);
        };
        TxWriteContext::delete_raw(self, key)
    }

    fn emit_typed_event_raw(
        &mut self,
        event_kind: statevec_model::EventKind,
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

    fn count_index_prefix_capped_raw(&self, kind: RecordKind, index_id: u8, prefix: &[u8], cap: usize)
        -> Result<CanonicalIndexCount, RuntimeHostError> {
        TxReadContext::count_index_prefix_capped_raw(self, kind, index_id, prefix, cap)
    }

    fn debug_log(&mut self, message: String) -> Result<(), RuntimeHostError> {
        self.debug_logs.push(message);
        Ok(())
    }
}

impl TxReadContext for TestHost {
    type Error = RuntimeHostError;

    fn with_read_raw<T>(
        &self,
        key: RecordKey,
        read: impl FnOnce(&[u8]) -> T,
    ) -> Result<Option<T>, Self::Error> {
        Ok(self.records.get(&key).map(|data| read(data)))
    }

    fn for_each_record_key(
        &self,
        kind: RecordKind,
        visit: &mut dyn FnMut(RecordKey),
    ) -> Result<(), Self::Error> {
        RuntimeHostContext::for_each_record_key_raw(self, kind, visit)
    }

    fn count_index_prefix_capped_raw(
        &self,
        kind: RecordKind,
        index_id: u8,
        prefix: &[u8],
        cap: usize,
    ) -> Result<CanonicalIndexCount, Self::Error> {
        let definition = self.registry.try_get(kind)
            .ok_or_else(|| RuntimeHostError::new(format!("unknown record kind {kind}")))?;
        if !definition.canonical_indexes.iter().any(|index| index.id == index_id) {
            return Err(RuntimeHostError::new("unknown canonical index"));
        }
        if cap == 0 {
            return Ok(CanonicalIndexCount::AtLeast(0));
        }
        let mut count = 0;
        // A small test engine derives index entries from the records rather
        // than maintaining the production engine's ordered index structures.
        for (key, data) in &self.records {
            if key.kind != kind {
                continue;
            }
            let index = self.registry.encode_canonical_index(kind, index_id, data)
                .ok_or_else(|| RuntimeHostError::new("cannot encode canonical index"))?;
            if prefix.len() > index.len() {
                return Err(RuntimeHostError::new("index prefix exceeds key length"));
            }
            if index.starts_with(prefix) {
                count += 1;
                if count == cap {
                    return Ok(CanonicalIndexCount::AtLeast(cap));
                }
            }
        }
        Ok(CanonicalIndexCount::Exact(count))
    }
}

impl TxUkContext for TestHost {
    fn resolve_uk(&self, kind: RecordKind, uk: &[u8]) -> Result<Option<SysId>, Self::Error> {
        self.resolve_uk_id(kind, 0, uk)
    }

    fn resolve_uk_id(
        &self,
        kind: RecordKind,
        id: u8,
        uk: &[u8],
    ) -> Result<Option<SysId>, Self::Error> {
        let definition = self.registry.try_get(kind)
            .ok_or_else(|| RuntimeHostError::new(format!("unknown record kind {kind}")))?;
        if !definition.unique_keys.iter().any(|key| key.id == id) {
            return Err(RuntimeHostError::new("unknown unique-key id"));
        }
        Ok(self.uk_index.get(&(kind, id, uk.to_vec())).copied())
    }
}

impl TxWriteContext for TestHost {
    fn create_raw(&mut self, kind: RecordKind, data: Vec<u8>) -> Result<RecordKey, Self::Error> {
        self.create_with_sys_id_raw(kind, self.next_sys_id, data)
    }

    fn update_raw<T>(
        &mut self,
        key: RecordKey,
        update: impl FnOnce(&mut [u8]) -> T,
    ) -> Result<Option<T>, Self::Error> {
        let Some(before) = self.records.get(&key) else {
            return Ok(None);
        };
        let mut after = before.clone();
        let result = update(&mut after);
        self.encode_unique_keys(key.kind, &after)?;
        let definition = self.registry.try_get(key.kind).expect("registered record");
        for field in definition.fields.iter().filter(|field| field.immutable) {
            let start = field.offset as usize;
            let end = start + field.len as usize;
            if before[start..end] != after[start..end] {
                return Err(RuntimeHostError::new(format!("immutable field {} changed", field.name)));
            }
        }
        self.records.insert(key, after);
        Ok(Some(result))
    }

    fn delete_raw(&mut self, key: RecordKey) -> Result<bool, Self::Error> {
        let Some(data) = self.records.get(&key) else {
            return Ok(false);
        };
        let keys = self.encode_unique_keys(key.kind, data)?;
        for (id, bytes) in keys {
            self.uk_index.remove(&(key.kind, id, bytes));
        }
        self.records.remove(&key);
        Ok(true)
    }

    fn emit_event_raw(&mut self, event_kind: statevec_model::EventKind, payload: Vec<u8>) {
        self.events.push(TestEvent { event_kind, payload });
    }

    fn debug_log(&mut self, message: String) {
        self.debug_logs.push(message);
    }
}

impl TxSysIdCreateContext for TestHost {
    fn create_with_sys_id_raw(
        &mut self,
        kind: RecordKind,
        sys_id: SysId,
        data: Vec<u8>,
    ) -> Result<RecordKey, Self::Error> {
        if sys_id == 0 || self.allocated_ids.contains(&sys_id) {
            return Err(RuntimeHostError::new("system id is zero or already used"));
        }
        let next = sys_id.checked_add(1).ok_or_else(|| RuntimeHostError::new("system id exhausted"))?;
        let keys = self.encode_unique_keys(kind, &data)?;
        for (id, bytes) in &keys {
            if self.uk_index.contains_key(&(kind, *id, bytes.clone())) {
                return Err(RuntimeHostError::new(format!("duplicate unique key {id} for record kind {kind}")));
            }
        }
        let key = RecordKey { kind, sys_id };
        for (id, bytes) in keys {
            self.uk_index.insert((kind, id, bytes), sys_id);
        }
        self.records.insert(key, data);
        self.allocated_ids.insert(sys_id);
        self.next_sys_id = self.next_sys_id.max(next);
        Ok(key)
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

    fn with_read_typed_by_uk_raw(
        &self,
        record_kind: RecordKind,
        uk: &[u8],
        f: &mut dyn FnMut(&[u8]),
    ) -> Result<bool, RuntimeHostError> {
        RuntimeHostContext::with_read_typed_by_uk_raw(self, record_kind, uk, f)
    }

    fn for_each_record_key_raw(
        &self,
        kind: RecordKind,
        f: &mut dyn FnMut(RecordKey),
    ) -> Result<(), RuntimeHostError> {
        RuntimeHostContext::for_each_record_key_raw(self, kind, f)
    }
}

/// Reads a generated record from any invariant context by unique-key bytes.
pub fn read_invariant_by_uk<C, R, P, T, F>(
    ctx: &C,
    uk: P,
    f: F,
) -> Result<Option<T>, RuntimeHostError>
where
    C: BizInvariantReadContext + ?Sized,
    R: GeneratedRecordAccess,
    P: AsRef<[u8]>,
    F: FnOnce(R::Access<'_>) -> T,
{
    InvariantReadContextExt::with_read_typed_by_uk::<R, P, T, F>(ctx, uk, f)
}

#[cfg(test)]
mod tests {
    use super::*;
    use statevec_api::{RuntimeCommandEnvelope, RuntimeHostContextExt};
    use statevec_model::{EventSchema, GeneratedEventAccess, RecordSchema};
    use statevec_macros::{command, event, record, schema_module};

    #[schema_module(version = "1.0")]
    mod schema {
        use super::*;
        #[record(kind = 1, record_len = 64, uk(id = 0, fields = [asset_id]))]
        pub struct Asset {
            #[field(index = 1, immutable)]
            pub asset_id: u64,
            #[field(index = 2)]
            pub precision: u8,
        }
        #[event(kind = 1)]
        pub struct AssetCreated {
            #[field(index = 1)]
            pub asset_id: u64,
        }
        #[command(kind = 9)]
        pub struct AssetCommand {
            #[field(index = 1)]
            pub asset_id: u64,
        }
    }
    use schema::*;

    #[record(
        kind = 2, record_len = 128,
        uk(id = 0, name = "by_item", fields = [item_id]),
        uk(id = 1, name = "by_external", fields = [external_id]),
        index(id = 0, name = "by_owner_status", fields = [owner, status])
    )]
    pub struct Item {
        #[field(index = 1, immutable)]
        pub item_id: u64,
        #[field(index = 2, immutable)]
        pub external_id: u64,
        #[field(index = 3)]
        pub owner: u64,
        #[field(index = 4)]
        pub status: u8,
    }

    fn item_host() -> TestHost {
        TestHost::new(SchemaRegistry::with_records(statevec_model::Version::new(1, 0), &[*Item::definition()]))
    }

    fn create_item(host: &mut TestHost, item_id: u64, external_id: u64, owner: u64) -> RecordKey {
        RuntimeHostContextExt::create_typed::<Item, _>(host, |item| {
            item.init_item_id(item_id).init_external_id(external_id).set_owner(owner).set_status(1);
        }).unwrap()
    }

    #[test]
    fn secondary_unique_key_conflict_preserves_indexes_and_id_allocation() {
        let mut host = item_host();
        let first = create_item(&mut host, 1, 101, 7);

        let error = RuntimeHostContextExt::create_typed::<Item, _>(&mut host, |item| {
            item.init_item_id(2).init_external_id(101).set_owner(8).set_status(1);
        }).unwrap_err();

        assert!(error.message.contains("duplicate unique key 1"), "{error:?}");
        assert_eq!(host.record_count(), 1);
        assert_eq!(host.read::<Item, _>(Item::uk(2), |_| ()), None);
        assert_eq!(host.read_by_uk_id::<Item, _>(1, Item::uk_by_external(101), |item| item.item_id()), Some(1));
        let second = create_item(&mut host, 2, 102, 8);
        assert_eq!(second.sys_id, first.sys_id + 1);

        assert!(TxWriteContext::delete_raw(&mut host, first).unwrap());
        assert_eq!(host.read_by_uk_id::<Item, _>(1, Item::uk_by_external(101), |_| ()), None);
    }

    #[test]
    fn transaction_error_restores_records_secondary_keys_events_and_ids() {
        let mut host = item_host();
        let first = create_item(&mut host, 1, 101, 7);

        let result = host.transaction(|tx| {
            create_item(tx, 2, 102, 8);
            RuntimeHostContext::emit_typed_event_raw(tx, 1, b"provisional")?;
            Err::<(), _>(RuntimeHostError::new("business refused"))
        });

        assert_eq!(result.unwrap_err().message, "business refused");
        assert_eq!(host.record_count(), 1);
        assert_eq!(host.read_by_uk_id::<Item, _>(1, Item::uk_by_external(102), |_| ()), None);
        assert!(host.events().is_empty());
        let next = create_item(&mut host, 3, 102, 9);
        assert_eq!(next.sys_id, first.sys_id + 1);
    }

    #[test]
    fn index_count_observes_provisional_updates_and_transaction_rollback() {
        let mut host = item_host();
        create_item(&mut host, 1, 101, 7);
        let second = create_item(&mut host, 2, 102, 8);
        let prefix = Item::index_by_owner_status_prefix1(7);

        let result = host.transaction(|tx| {
            TxWriteContext::update_raw(tx, second, |data| { Item::wrap_update(data).set_owner(7); })?;
            assert_eq!(TxReadContext::count_index_prefix_capped_raw(tx, Item::KIND, 0, &prefix, 2)?,
                CanonicalIndexCount::AtLeast(2));
            Err::<(), _>(RuntimeHostError::new("discard update"))
        });

        assert!(result.is_err());
        assert_eq!(TxReadContext::count_index_prefix_capped_raw(&host, Item::KIND, 0, &prefix, 2).unwrap(),
            CanonicalIndexCount::Exact(1));
        assert_eq!(host.expect::<Item, _>(Item::uk(2), |item| item.owner()), 8);
    }

    #[test]
    fn transaction_unwind_restores_state_before_propagating_the_panic() {
        let mut host = item_host();
        let panic = catch_unwind(AssertUnwindSafe(|| {
            let _: Result<(), ()> = host.transaction(|tx| {
                create_item(tx, 1, 101, 7);
                panic!("handler failed");
            });
        }));

        assert!(panic.is_err());
        assert_eq!(host.record_count(), 0);
        assert_eq!(host.read_by_uk_id::<Item, _>(1, Item::uk_by_external(101), |_| ()), None);
        assert_eq!(create_item(&mut host, 2, 101, 7).sys_id, 1);
    }

    #[test]
    fn typed_plugin_run_checks_payload_before_invoking_the_plugin() {
        let mut host = TestHost::for_plugin(EchoPlugin);
        let result = host.run_with_envelope::<AssetCommand>(1, 0, [0; 7]);

        assert!(result.is_err());
        assert!(host.events().is_empty());
        assert!(host.debug_logs().is_empty());
    }

    #[test]
    fn create_read_update_and_delete_record_by_unique_key() {
        let mut host = TestHost::new(registry());

        let key = host
            .create_typed::<Asset, _>(|asset| {
                asset.init_asset_id(42).set_precision(6);
            })
            .unwrap();
        assert_eq!(key.sys_id, 1);

        let asset = host
            .read_typed_by_uk::<Asset, _, _, _>(Asset::uk(42), |asset| {
                (asset.asset_id(), asset.precision())
            })
            .unwrap();
        assert_eq!(asset, Some((42, 6)));

        let updated = host
            .update_typed_by_uk::<Asset, _, _, _>(Asset::uk(42), |asset| {
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

        assert!(host.delete_by_uk::<Asset, _>(Asset::uk(42)).unwrap());
        assert_eq!(host.record_count(), 0);
    }

    #[test]
    fn duplicate_unique_key_is_rejected() {
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
        assert!(err.message.contains("duplicate unique key"));
        assert_eq!(host.record_count(), 1);
    }

    #[test]
    fn immutable_key_change_does_not_commit_partial_update() {
        let mut host = TestHost::new(registry());
        host.create_typed::<Asset, _>(|asset| {
            asset.init_asset_id(1).set_precision(6);
        })
        .unwrap();
        host.create_typed::<Asset, _>(|asset| {
            asset.init_asset_id(2).set_precision(8);
        })
        .unwrap();

        let err = RuntimeHostContext::update_typed_by_uk_raw(
            &mut host,
            Asset::KIND,
            Asset::uk(1).as_slice(),
            &mut |data| {
                data[0..8].copy_from_slice(&2u64.to_le_bytes());
                data[8] = 9;
            },
        )
        .unwrap_err();

        assert!(err.message.contains("immutable field"));
        let asset = host
            .read_typed_by_uk::<Asset, _, _, _>(Asset::uk(1), |asset| {
                (asset.asset_id(), asset.precision())
            })
            .unwrap();
        assert_eq!(asset, Some((1, 6)));
    }

    #[test]
    fn captures_and_reads_typed_events() {
        let mut host = TestHost::new(registry());
        let payload = AssetCreated::builder().set_asset_id(7).build().expect("fixed-width payload");

        RuntimeHostContextExt::emit_typed_event::<AssetCreated>(&mut host, payload).unwrap();

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
        let precision = read_invariant_by_uk::<_, Asset, _, _, _>(&host, Asset::uk(42), |asset| {
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
