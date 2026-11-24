use sdk_source_consumer::schema::{Ping, registry};
use statevec::model::CommandSchemaError;
use statevec::{CommandSchema, GeneratedCommandAccess, RuntimeCommandEnvelope, SchemaRegistry, TypedTxContext};
use statevec_api::RuntimeCommandRef;
use statevec_api::plugin::{
    ConfigureError, ConfiguredCore, CoreConfig, CoreCreateError, CoreMetadata, CoreModule, CoreRuntime, CoreTxError,
    CoreVersion, MigrationTx, PluginTx, TxInvariantHook,
};

struct PingModule;
struct Configured(SchemaRegistry);

impl CoreModule for PingModule {
    type Configured = Configured;

    fn configure(&self, _: CoreConfig<'_>) -> Result<Configured, ConfigureError> {
        Ok(Configured(registry()))
    }
}

impl ConfiguredCore for Configured {
    type Runtime = PingModule;

    fn metadata(&self) -> CoreMetadata<'_> {
        CoreMetadata::new("ping", "ping-1", &self.0)
    }

    fn create_instance(&self) -> Result<PingModule, CoreCreateError> {
        Ok(PingModule)
    }
}

impl CoreRuntime for PingModule {
    const TX_INVARIANTS: Option<TxInvariantHook<Self>> = None;

    fn initialize_core(&self, _: &mut PluginTx<'_>, _: &[u8]) -> Result<(), CoreTxError> {
        Ok(())
    }

    fn migrate(&self, _: &mut MigrationTx<'_>, _: CoreVersion, _: &[u8]) -> Result<(), CoreTxError> {
        Ok(())
    }

    fn preflight_command(&self, command: &RuntimeCommandRef<'_>) -> Result<(), CommandSchemaError> {
        match command.command_kind() {
            Ping::KIND => Ping::validate_payload(command.payload()),
            kind => Err(CommandSchemaError::UnsupportedKind { kind }),
        }
    }

    fn run_tx(&self, tx: &mut PluginTx<'_>, _: &RuntimeCommandRef<'_>) -> Result<(), CoreTxError> {
        tx.ref_tx_time_ns()?;
        Ok(())
    }
}

statevec_api::export_core_plugin!(PingModule);
