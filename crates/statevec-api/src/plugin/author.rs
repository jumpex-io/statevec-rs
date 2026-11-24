//! Safe authoring surface for a configured Core.
use super::tx::PluginTx;
use super::{ConfigureError, CoreCreateError, CoreExecutionError, CoreTxError, CoreVersion, MigrationTx};
use crate::{ReferenceTimeUnavailable, RuntimeCommandRef};
use statevec_model::{CommandSchemaError, RecordKey, SchemaRegistry};

#[non_exhaustive]
pub struct CoreConfig<'a> {
    pub bytes: &'a [u8],
}

impl<'a> CoreConfig<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }
}

#[non_exhaustive]
pub struct CoreMetadata<'a> {
    pub name: &'a str,
    /// Commits all execution-relevant configuration, independent of its spelling.
    pub revision: &'a str,
    pub schema: &'a SchemaRegistry,
}

impl<'a> CoreMetadata<'a> {
    pub fn new(name: &'a str, revision: &'a str, schema: &'a SchemaRegistry) -> Self {
        Self { name, revision, schema }
    }
}

pub trait CoreModule: Send + Sync + 'static {
    type Configured: ConfiguredCore;
    fn configure(&self, config: CoreConfig<'_>) -> Result<Self::Configured, ConfigureError>;
}

/// Immutable configuration, parsed once before a profile/root is selected.
/// Multiple live/candidate instances may coexist. They must not share mutable
/// business state, use side files or acquire an uncommitted execution setting.
pub trait ConfiguredCore: Send + Sync + 'static {
    type Runtime: CoreRuntime;
    fn metadata(&self) -> CoreMetadata<'_>;
    fn create_instance(&self) -> Result<Self::Runtime, CoreCreateError>;
}

pub type TxInvariantHook<R> = fn(&R, &dyn CandidateRead) -> Result<(), CoreTxError>;

/// One engine-thread instance. The SDK catches unwinds at every C boundary;
/// catching an unwind does not authorize reuse of this instance.
pub trait CoreRuntime: Sized + 'static {
    const TX_INVARIANTS: Option<TxInvariantHook<Self>>;
    /// Deterministic initialization of the empty business state. This can run
    /// more than once (proposal, validation, apply or replay). It must have no
    /// external side effects; use only input and the deterministic host views,
    /// never wall-clock time, randomness, process state or external I/O.
    fn initialize_core(&self, tx: &mut PluginTx<'_>, input: &[u8]) -> Result<(), CoreTxError>;
    /// Validate retained source records under the target rules and emit events.
    /// InPlace preserves record IDs, bytes, revisions and addresses, with no
    /// record deltas. Incompatible layouts refuse before this callback (system
    /// code 5, reason 9). Record create/preserve/delete attempts return
    /// MigrationWriteForbidden and latch reason 10 even if ignored.
    /// Trials are discarded and committed apply recomputes this callback; the
    /// same deterministic/no-side-effects contract as initialization applies.
    /// The host owns version, history, schema metadata and position.
    fn migrate(&self, tx: &mut MigrationTx<'_>, source: CoreVersion, input: &[u8]) -> Result<(), CoreTxError>;
    /// Optional business preflight. The exported SDK preflight has already
    /// checked this command against its declared schema: an undeclared kind or
    /// an invalid payload never reaches this method or `run_tx`.
    fn preflight_command(&self, command: &RuntimeCommandRef<'_>) -> Result<(), CommandSchemaError> {
        let _ = command;
        Ok(())
    }
    fn run_tx(&self, tx: &mut PluginTx<'_>, command: &RuntimeCommandRef<'_>) -> Result<(), CoreTxError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateChange<'a> {
    New { key: RecordKey, after: &'a [u8] },
    Update { key: RecordKey, before: &'a [u8], after: &'a [u8] },
    Delete { key: RecordKey, before: &'a [u8] },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateAfterChange<'a> {
    New { key: RecordKey, after: &'a [u8] },
    Update { key: RecordKey, after: &'a [u8] },
    Delete { key: RecordKey },
}

/// Views of the same native candidate, including records it did not change.
/// Callback data is borrowed only for that callback. Reads may reenter other
/// reads. Errors preserve access vs host/ABI/panic causes, never business codes.
pub trait CandidateRead {
    fn ref_tx_time_ns(&self) -> Result<u64, ReferenceTimeUnavailable>;
    fn read_before(&self, key: RecordKey, visit: &mut dyn FnMut(&[u8])) -> Result<bool, CoreExecutionError>;
    fn read_after(&self, key: RecordKey, visit: &mut dyn FnMut(&[u8])) -> Result<bool, CoreExecutionError>;
    fn resolve_unique_key_before(
        &self,
        kind: u16,
        unique_key_id: u8,
        key: &[u8],
    ) -> Result<Option<u64>, CoreExecutionError>;
    fn resolve_unique_key_after(
        &self,
        kind: u16,
        unique_key_id: u8,
        key: &[u8],
    ) -> Result<Option<u64>, CoreExecutionError>;
    fn for_each_change(&self, visit: &mut dyn FnMut(CandidateChange<'_>)) -> Result<(), CoreExecutionError>;
    fn for_each_after_change(&self, visit: &mut dyn FnMut(CandidateAfterChange<'_>)) -> Result<(), CoreExecutionError>;
}
