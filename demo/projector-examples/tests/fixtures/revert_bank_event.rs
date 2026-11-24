//! Decoder bound to the current Bank's display metadata before reactivation.
mod additive_bank;
#[path = "../../../../crates/statevec/tests/fixtures/core_bank_semantic.rs"]
mod semantic;
use additive_bank::Bank;
use statevec_projector_api::plugin::*;
struct Module;
impl ProjectorModule for Module {
    type Configured = additive_bank::Config<false>;
    const SUPPORTED_HISTORY_MODES: SupportedHistoryModes = SupportedHistoryModes::Both;
    const SCHEMA_COMPATIBILITY: SchemaCompatibility = SchemaCompatibility::Additive;
    fn decoder_idl() -> &'static [u8] {
        static IDL: std::sync::OnceLock<String> = std::sync::OnceLock::new();
        IDL.get_or_init(|| {
            semantic::tagged(std::str::from_utf8(Bank::<false, true>::decoder_idl()).unwrap(), "events").unwrap()
        })
        .as_bytes()
    }
    fn configure(&self, input: ProjectorConfig<'_>) -> Result<Self::Configured, ConfigureError> {
        Bank::<false, true>.configure(input)
    }
}
statevec_projector_api::export_projector_plugin!(Module);
