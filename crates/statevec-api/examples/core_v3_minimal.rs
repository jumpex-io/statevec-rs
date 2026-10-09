//! Minimal rule-free plugin. Build with:
//! cargo build -p statevec-api --example core_v3_minimal
//! This demonstrates packaging, not a durable host or a business application.
use statevec_api::{RuntimeCommandRef, TypedTxContext, plugin_v3::*};
use statevec_model::{CommandSchemaFailure, SchemaRegistry, Version};

struct Module;
struct Configured {
    schema: SchemaRegistry,
}
struct Runtime;
impl CoreModule for Module {
    type Configured = Configured;
    fn configure(&self, config: CoreConfig<'_>) -> Result<Configured, ConfigureFailure> {
        if config.bytes != b"{}" {
            return Err(ConfigureFailure::Invalid { code: 1 });
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
    fn create_instance(&self) -> Result<Runtime, CreateFailure> {
        Ok(Runtime)
    }
}
impl CoreRuntime for Runtime {
    const TX_INVARIANTS: Option<TxInvariantHook<Self>> = None;
    fn preflight_command(&self, _: &RuntimeCommandRef<'_>) -> Result<(), CommandSchemaFailure> {
        Ok(())
    }
    fn run_tx(&self, tx: &mut PluginTx<'_>, _: &RuntimeCommandRef<'_>) -> Result<(), TxFailure> {
        let _time = tx.ref_tx_time_ns()?;
        let _position = tx.tx_seq()?;
        Ok(())
    }
}
statevec_api::export_core_plugin_v3!(Module);
