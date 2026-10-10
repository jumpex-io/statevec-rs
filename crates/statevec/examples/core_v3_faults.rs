//! Core ABI qualification fixture, not an application runtime. Valid commands
//! deliberately unwind in preflight, dispatch or the rule hook. Atomic counts
//! observe physical calls only; they never decide business output or recovery.
use statevec::api::plugin_v3::*;
use statevec::prelude::*;
use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};

static CALLS: [AtomicU64; 5] = [const { AtomicU64::new(0) }; 5];

#[unsafe(no_mangle)]
pub extern "C" fn core_fixture_call_count(slot: u32) -> u64 {
    CALLS.get(slot as usize).map_or(u64::MAX, |count| count.load(Ordering::Relaxed))
}

#[schema_module(version = "1.0")]
mod counters {
    use super::*;
    #[record(kind = 1, record_len = 64, uk(id = 0, fields = [account_id]))]
    pub struct Account {
        #[field(index = 1, immutable = true)]
        pub account_id: u64,
        #[field(index = 2)]
        pub total: u64,
    }
    #[command(kind = 1)]
    pub struct Add {
        #[field(index = 1)]
        pub account_id: u64,
        #[field(index = 2)]
        pub amount: u64,
    }
    #[command(kind = 2)]
    pub struct PanicAfterWrite {
        #[field(index = 1)]
        pub account_id: u64,
        #[field(index = 2)]
        pub amount: u64,
    }
    #[command(kind = 3)]
    pub struct PreflightPanic {
        #[field(index = 1)]
        pub account_id: u64,
        #[field(index = 2)]
        pub amount: u64,
    }
    #[event(kind = 1, inline_response = true)]
    pub struct Changed {
        #[field(index = 1)]
        pub sys_id: u64,
        #[field(index = 2)]
        pub total: u64,
        #[field(index = 3)]
        pub position: u64,
        #[field(index = 4)]
        pub time_ns: u64,
    }
}
use counters::*;

struct Module;
struct Configured(SchemaRegistry);
struct Runtime {
    poisoned: Cell<bool>,
}
impl CoreModule for Module {
    type Configured = Configured;
    fn configure(&self, config: CoreConfig<'_>) -> Result<Configured, ConfigureFailure> {
        if config.bytes != b"{}" {
            return Err(ConfigureFailure::Invalid { code: 1 });
        }
        Ok(Configured(registry()))
    }
}
impl ConfiguredCore for Configured {
    type Runtime = Runtime;
    fn metadata(&self) -> CoreMetadata<'_> {
        CoreMetadata {
            name: "unwind-fixture",
            revision: "core-unwind-fixture-v1",
            schema: &self.0,
        }
    }
    fn create_instance(&self) -> Result<Runtime, CreateFailure> {
        CALLS[0].fetch_add(1, Ordering::Relaxed);
        Ok(Runtime {
            poisoned: Cell::new(false),
        })
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        CALLS[4].fetch_add(1, Ordering::Relaxed);
    }
}
impl Runtime {
    fn unwind(&self) -> ! {
        self.poisoned.set(true);
        panic!("deliberate Core fixture unwind");
    }
    fn rules(&self, view: &dyn CandidateRead) -> Result<(), TxFailure> {
        CALLS[3].fetch_add(1, Ordering::Relaxed);
        view.for_each_after_change(&mut |change| {
            let data = match change {
                CandidateAfterChange::New { after, .. }
                | CandidateAfterChange::Update { after, .. } => after,
                CandidateAfterChange::Delete { .. } => return,
            };
            if Account::wrap(data).account_id() == 99 {
                self.unwind();
            }
        })?;
        Ok(())
    }
}
impl CoreRuntime for Runtime {
    const TX_INVARIANTS: Option<TxInvariantHook<Self>> = Some(Self::rules);
    fn preflight_command(
        &self,
        command: &RuntimeCommandRef<'_>,
    ) -> Result<(), statevec::model::CommandSchemaFailure> {
        CALLS[1].fetch_add(1, Ordering::Relaxed);
        assert!(!self.poisoned.get(), "a poisoned runtime must never be called again");
        match command.command_kind() {
            Add::KIND => Add::validate_payload(command.payload()),
            PanicAfterWrite::KIND => PanicAfterWrite::validate_payload(command.payload()),
            PreflightPanic::KIND => {
                PreflightPanic::validate_payload(command.payload())?;
                self.unwind();
            }
            kind => Err(statevec::model::CommandSchemaFailure::UnsupportedKind { kind }),
        }
    }
    fn run_tx(
        &self,
        tx: &mut PluginTx<'_>,
        command: &RuntimeCommandRef<'_>,
    ) -> Result<(), TxFailure> {
        CALLS[2].fetch_add(1, Ordering::Relaxed);
        assert!(!self.poisoned.get(), "a poisoned runtime must never be called again");
        // All three command layouts are the same two u64s, checked above.
        let input = Add::wrap(command.payload());
        tx.update_or_create_typed_by_uk::<Account, _, _, _, _>(
            Account::uk(input.account_id()),
            |row| {
                row.set_total(row.total() + input.amount());
            },
            |row| {
                row.init_account_id(input.account_id());
                row.set_total(input.amount());
            },
        )?;
        let sys_id = tx
            .resolve_uk(Account::KIND, Account::uk(input.account_id()).as_ref())?
            .unwrap();
        let total = tx
            .with_read_typed::<Account, _, _>(sys_id, |row| row.total())?
            .unwrap();
        tx.emit_typed_event::<Changed>(
            Changed::builder()
                .set_sys_id(sys_id)
                .set_total(total)
                .set_position(tx.tx_seq()?)
                .set_time_ns(tx.ref_tx_time_ns()?)
                .build()
                .expect("fixed fields"),
        );
        if command.command_kind() == PanicAfterWrite::KIND {
            self.unwind();
        }
        Ok(())
    }
}
statevec::api::export_core_plugin_v3!(Module);
