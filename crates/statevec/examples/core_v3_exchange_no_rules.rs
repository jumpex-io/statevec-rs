//! The same order/reservation workload with no optional transaction hook.
#[path = "support/core_exchange.rs"]
mod exchange;
use statevec::api::plugin_v3::{CoreModule, CoreConfig, ConfigureFailure};
struct Module;
impl CoreModule for Module {
    type Configured = exchange::Configured<false>;
    fn configure(&self, config: CoreConfig<'_>) -> Result<Self::Configured, ConfigureFailure> {
        exchange::Module::<false>.configure(config)
    }
}
statevec::api::export_core_plugin_v3!(Module);
