//! Minimal rule-free plugin. Build with:
//! cargo build -p statevec-api --example core_v4_minimal
//! This demonstrates packaging, not a durable host or a business application.
use statevec_api::{RuntimeCommandRef, TypedTxContext, v1::*};
use statevec_model::{CommandSchemaError, SchemaRegistry, Version};

struct Module;
struct Configured {
    schema: SchemaRegistry,
}
struct Runtime;
impl CoreModule for Module {
    type Configured = Configured;
    fn configure(&self, config: CoreConfig<'_>) -> Result<Configured, ConfigureError> {
        if config.bytes != b"{}" {
            return Err(ConfigureError::Invalid { code: 1 });
        }
        Ok(Configured {
            schema: SchemaRegistry::with_records(Version::new(1, 0), &[]),
        })
    }
}
impl ConfiguredCore for Configured {
    type Runtime = Runtime;
    fn metadata(&self) -> CoreMetadata<'_> {
        CoreMetadata {
            name: "minimal",
            revision: "inline-zero-effect-v1",
            schema: &self.schema,
        }
    }
    fn create_instance(&self) -> Result<Runtime, CoreCreateError> {
        Ok(Runtime)
    }
}
impl CoreRuntime for Runtime {
    const TX_INVARIANTS: Option<TxInvariantHook<Self>> = None;
    fn initialize_core(&self, _: &mut PluginTx<'_>, _: &[u8]) -> Result<(), CoreTxError> { Ok(()) }
    // This deliberately empty-schema example produces an empty target.
    fn migrate(&self, _: &mut MigrationTx<'_>, _: CoreVersion, _: &[u8]) -> Result<(), CoreTxError> { Ok(()) }
    fn preflight_command(&self, _: &RuntimeCommandRef<'_>) -> Result<(), CommandSchemaError> {
        Ok(())
    }
    fn run_tx(&self, tx: &mut PluginTx<'_>, _: &RuntimeCommandRef<'_>) -> Result<(), CoreTxError> {
        let _time = tx.ref_tx_time_ns()?;
        let _position = tx.tx_seq()?;
        Ok(())
    }
}
statevec_api::export_core_plugin_v4!(Module);
