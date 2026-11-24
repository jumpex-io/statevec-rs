use statevec::prelude::*;

#[schema_module(version = "1.0")]
pub mod schema {
    use super::*;

    #[command(kind = 1)]
    pub struct Ping {
        #[field(index = 1)]
        pub value: u64,
    }
}

// Raw adapters also depend on the public names of the callback tables.
// A later SDK can extend a table after its existing fields.
pub mod tables {
    pub use statevec_api::plugin::ffi::{SvCandidate, SvCore, SvMigration, SvTx};
    pub use statevec_projector_api::plugin::ffi::{SvEngineQueryHost, SvProjector, SvProjectorHost, SvProjectorHostQuery};
}
