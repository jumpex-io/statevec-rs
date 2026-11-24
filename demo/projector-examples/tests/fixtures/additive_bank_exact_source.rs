mod additive_bank;
use additive_bank::Bank;
use statevec_projector_api::plugin::*;
struct Module;
impl ProjectorModule for Module {
    type Configured = additive_bank::Config<false>;
    const SUPPORTED_HISTORY_MODES: SupportedHistoryModes = SupportedHistoryModes::Both;
    fn decoder_idl() -> &'static [u8] { Bank::<false, false>::decoder_idl() }
    fn configure(&self, input: ProjectorConfig<'_>) -> Result<Self::Configured, ConfigureError> {
        Bank::<false, false>.configure(input)
    }
}
statevec_projector_api::export_projector_plugin!(Module);
