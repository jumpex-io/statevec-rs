//! Generic ingress client. The owner decides admission, reconciliation and
//! retirement; adapters and physical ports do not retain parallel policy.

#[statevec_domain_roles::domain_owner_module("ingress_client")]
mod client {
    #[statevec_domain_roles::domain_adapter_module("ingress_client")]
    mod adapter {
        include!("adapter.rs");
    }
    #[statevec_domain_roles::domain_port_module("ingress_client")]
    mod port {
        include!("port.rs");
    }
    include!("owner.rs");
}

pub use client::*;

// Scheduling composition, not another semantic owner or physical port. There
// is exactly one IngressClientOwner behind the presentation's exclusion lock.
mod background;
pub use background::BatchBackground;
