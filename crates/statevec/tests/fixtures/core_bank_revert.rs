//! Loaded R15 qualification: unchanged Bank execution, optional display tags,
//! and counters of actual SDK instance construction/destruction/callbacks.
use statevec::api::plugin::*;
use statevec::prelude::*;
use std::sync::atomic::{AtomicU64, Ordering};

#[path = "../../examples/core_bank/schema.rs"]
mod bank;
#[path = "../../examples/core_bank/business.rs"]
mod business;
#[path = "core_bank_semantic.rs"]
mod semantic;

static COUNTS: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];

// Diagnostic export in this fixture only. It grants no execution authority.
#[unsafe(no_mangle)]
pub extern "C" fn statevec_fixture_count(slot: u32) -> u64 {
    COUNTS.get(slot as usize).map_or(0, |value| value.load(Ordering::Relaxed))
}

struct Module;
struct Configured {
    policy: business::Policy,
    schema: SchemaRegistry,
    revision: String,
}
struct Runtime(business::Runtime);

impl CoreModule for Module {
    type Configured = Configured;
    fn configure(&self, input: CoreConfig<'_>) -> Result<Configured, ConfigureError> {
        let mut config: serde_json::Value =
            serde_json::from_slice(input.bytes).map_err(|_| ConfigureError::Invalid { code: 1 })?;
        let tag = config
            .as_object_mut()
            .ok_or(ConfigureError::Invalid { code: 1 })?
            .remove("semantic");
        let policy: business::Policy =
            serde_json::from_value(config).map_err(|_| ConfigureError::Invalid { code: 1 })?;
        if policy.min_amount == 0 || policy.min_amount > policy.max_balance {
            return Err(ConfigureError::Invalid { code: 2 });
        }
        let mut schema = bank::registry();
        if let Some(tag) = tag {
            let family = tag.as_str().ok_or(ConfigureError::Invalid { code: 3 })?;
            let idl = semantic::tagged(&schema.to_idl_json(), family).ok_or(ConfigureError::Invalid { code: 3 })?;
            // A decoded registry carries no validators; the tag changes no layout.
            schema = SchemaRegistry::from_idl_json(&idl)
                .map_err(|_| ConfigureError::Invalid { code: 3 })?
                .with_command_validator::<bank::Deposit>()
                .with_command_validator::<bank::Transfer>();
        }
        // The optional display tag is outside execution identity. This also
        // permits the same-V test binding in all three metadata controls.
        // Those controls do not qualify a same-V deployment package exchange.
        let revision = format!(
            "core-bank-v1;max={};min={}{}",
            policy.max_balance,
            policy.min_amount,
            if policy.retain_migration { ";migration=retain-v1" } else { "" }
        );
        Ok(Configured { policy, schema, revision })
    }
}
impl ConfiguredCore for Configured {
    type Runtime = Runtime;
    fn metadata(&self) -> CoreMetadata<'_> {
        CoreMetadata::new("core-bank", &self.revision, &self.schema)
    }
    fn create_instance(&self) -> Result<Runtime, CoreCreateError> {
        COUNTS[0].fetch_add(1, Ordering::Relaxed);
        Ok(Runtime(business::Runtime(self.policy)))
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        COUNTS[1].fetch_add(1, Ordering::Relaxed);
    }
}
impl CoreRuntime for Runtime {
    fn initialize_core(&self, tx: &mut PluginTx<'_>, input: &[u8]) -> Result<(), CoreTxError> {
        COUNTS[2].fetch_add(1, Ordering::Relaxed);
        self.0.initialize_core(tx, input)
    }
    fn migrate(&self, tx: &mut MigrationTx<'_>, source: CoreVersion, input: &[u8]) -> Result<(), CoreTxError> {
        COUNTS[3].fetch_add(1, Ordering::Relaxed);
        self.0.migrate(tx, source, input)
    }
    const TX_INVARIANTS: Option<TxInvariantHook<Self>> = Some(|runtime, view| runtime.0.rules(view));
    fn preflight_command(&self, command: &RuntimeCommandRef<'_>) -> Result<(), statevec::model::CommandSchemaError> {
        self.0.preflight_command(command)
    }
    fn run_tx(&self, tx: &mut PluginTx<'_>, command: &RuntimeCommandRef<'_>) -> Result<(), CoreTxError> {
        self.0.run_tx(tx, command)
    }
}
statevec::api::export_core_plugin!(Module);
