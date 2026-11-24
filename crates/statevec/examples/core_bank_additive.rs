//! Product-path qualification target: add fields, keys, kinds and events while
//! retaining the source Bank's command/event wire definitions and behavior.
use statevec::api::plugin::*;
use statevec::prelude::*;
#[path = "core_bank/schema.rs"]
mod bank;
#[path = "core_bank/business.rs"]
mod business;
#[path = "core_bank_additive/schema.rs"]
mod additive;
use additive::{Account, Activated, Receipt, TagAccount};

struct Module;
struct Configured(SchemaRegistry);
struct Runtime(business::Runtime);
fn reject(code: u16) -> CoreTxError {
    CoreTxError::Rejected(BusinessRejectCode::new(code).unwrap())
}
impl CoreModule for Module {
    type Configured = Configured;
    fn configure(&self, config: CoreConfig<'_>) -> Result<Configured, ConfigureError> {
        if config.bytes != b"bank-additive-v2" {
            return Err(ConfigureError::Invalid { code: 1 });
        }
        Ok(Configured(additive::registry()))
    }
}
impl ConfiguredCore for Configured {
    type Runtime = Runtime;
    fn metadata(&self) -> CoreMetadata<'_> {
        CoreMetadata::new("core-bank-additive", "bank-additive-v2-limit100", &self.0)
    }
    fn create_instance(&self) -> Result<Runtime, CoreCreateError> {
        Ok(Runtime(business::Runtime(business::Policy {
            max_balance: 100, min_amount: 1, retain_migration: true,
        })))
    }
}
impl CoreRuntime for Runtime {
    fn initialize_core(&self, tx: &mut PluginTx<'_>, input: &[u8]) -> Result<(), CoreTxError> {
        self.0.initialize_core(tx, input)
    }
    fn migrate(&self, tx: &mut MigrationTx<'_>, source: CoreVersion, input: &[u8]) -> Result<(), CoreTxError> {
        self.0.migrate(tx, source, input)?;
        tx.records().1.emit_event(Activated::KIND, &Activated::builder().set_marker(42).build().unwrap())?;
        Ok(())
    }
    const TX_INVARIANTS: Option<TxInvariantHook<Self>> = Some(|runtime, view| runtime.0.rules(view));
    fn preflight_command(&self, command: &RuntimeCommandRef<'_>) -> Result<(), statevec::model::CommandSchemaError> {
        if command.command_kind() == TagAccount::KIND {
            TagAccount::validate_payload(command.payload())
        } else {
            self.0.preflight_command(command)
        }
    }
    fn run_tx(&self, tx: &mut PluginTx<'_>, command: &RuntimeCommandRef<'_>) -> Result<(), CoreTxError> {
        if command.command_kind() != TagAccount::KIND {
            return self.0.run_tx(tx, command);
        }
        let input = TagAccount::wrap(command.payload());
        tx.update_typed_by_unique_key::<Account, _, _, _>(Account::unique_key(input.account_id()), |row| {
            row.set_tag(input.tag());
        })?.ok_or_else(|| reject(1))?;
        tx.create_typed::<Receipt, _>(|row| { row.init_tag(input.tag()); })?;
        Ok(())
    }
}
statevec::api::export_core_plugin!(Module);
