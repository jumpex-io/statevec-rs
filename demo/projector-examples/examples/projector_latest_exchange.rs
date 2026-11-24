//! The latest-state consumer with the Exchange decoder, not a host-selected IDL.
#[path = "projector_latest/plugin.rs"]
mod plugin;
#[allow(dead_code, unused_imports)]
#[path = "../../../crates/statevec/examples/support/core_exchange_schema.rs"]
mod exchange;
use statevec_projector_api::plugin::*;
struct LatestExchange;
impl ProjectorModule for LatestExchange {
    type Configured = plugin::Config;
    const SUPPORTED_HISTORY_MODES: SupportedHistoryModes = SupportedHistoryModes::LatestState;
    fn decoder_idl() -> &'static [u8] { exchange::decoder_idl() }
    fn configure(&self, input: ProjectorConfig<'_>) -> Result<Self::Configured, ConfigureError> {
        plugin::Latest.configure(input)
    }
}
statevec_projector_api::export_projector_plugin!(LatestExchange);
