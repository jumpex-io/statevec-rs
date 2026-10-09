//! Safe authoring surface for the fixed-version Core V3 host.
use super::{ConfigureFailure, CreateFailure, ExecutionFailure, TxFailure, tx::PluginTx};
use crate::{ReferenceTimeUnavailable, RuntimeCommandRef};
use statevec_model::{CommandSchemaFailure, RecordKey, SchemaRegistry};

pub struct CoreConfig<'a> {
    pub bytes: &'a [u8],
}
pub struct CoreMetadata<'a> {
    pub name: &'a str,
    /// Commits all execution-relevant configuration, independent of its spelling.
    pub revision: &'a str,
    pub schema: &'a SchemaRegistry,
}

pub trait CoreModule: Send + Sync + 'static {
    type Configured: ConfiguredCore;
    fn configure(&self, config: CoreConfig<'_>) -> Result<Self::Configured, ConfigureFailure>;
}

/// Immutable configuration, parsed once before a profile/root is selected.
/// Multiple live/candidate instances may coexist. They must not share mutable
/// business state, use side files or acquire an uncommitted execution setting.
pub trait ConfiguredCore: Send + Sync + 'static {
    type Runtime: CoreRuntime;
    fn metadata(&self) -> CoreMetadata<'_>;
    fn create_instance(&self) -> Result<Self::Runtime, CreateFailure>;
}

pub type TxInvariantHook<R> = fn(&R, &dyn CandidateRead) -> Result<(), TxFailure>;

/// One engine-thread instance. The SDK catches unwinds at every C boundary;
/// catching an unwind does not authorize reuse of this instance.
pub trait CoreRuntime: Sized + 'static {
    const TX_INVARIANTS: Option<TxInvariantHook<Self>>;
    fn preflight_command(
        &self,
        command: &RuntimeCommandRef<'_>,
    ) -> Result<(), CommandSchemaFailure>;
    fn run_tx(
        &self,
        tx: &mut PluginTx<'_>,
        command: &RuntimeCommandRef<'_>,
    ) -> Result<(), TxFailure>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateChange<'a> {
    New {
        key: RecordKey,
        after: &'a [u8],
    },
    Update {
        key: RecordKey,
        before: &'a [u8],
        after: &'a [u8],
    },
    Delete {
        key: RecordKey,
        before: &'a [u8],
    },
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
    fn read_before(
        &self,
        key: RecordKey,
        visit: &mut dyn FnMut(&[u8]),
    ) -> Result<bool, ExecutionFailure>;
    fn read_after(
        &self,
        key: RecordKey,
        visit: &mut dyn FnMut(&[u8]),
    ) -> Result<bool, ExecutionFailure>;
    fn resolve_uk_before(
        &self,
        kind: u16,
        uk_id: u8,
        key: &[u8],
    ) -> Result<Option<u64>, ExecutionFailure>;
    fn resolve_uk_after(
        &self,
        kind: u16,
        uk_id: u8,
        key: &[u8],
    ) -> Result<Option<u64>, ExecutionFailure>;
    fn for_each_change(
        &self,
        visit: &mut dyn FnMut(CandidateChange<'_>),
    ) -> Result<(), ExecutionFailure>;
    fn for_each_after_change(
        &self,
        visit: &mut dyn FnMut(CandidateAfterChange<'_>),
    ) -> Result<(), ExecutionFailure>;
}
