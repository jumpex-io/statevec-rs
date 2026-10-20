//! Order/reservation Core example with an optional notional invariant hook.
#[path = "support/core_exchange.rs"]
mod exchange;
use statevec::api::v1::{CoreModule, CoreConfig, ConfigureError};
struct Module;
impl CoreModule for Module {
    type Configured = exchange::Configured<true>;
    fn configure(&self, config: CoreConfig<'_>) -> Result<Self::Configured, ConfigureError> {
        exchange::Module::<true>.configure(config)
    }
}
statevec::api::export_core_plugin_v5!(Module);
