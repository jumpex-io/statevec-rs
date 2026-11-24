//! A stateful Core example with double-entry deposits/transfers and a
//! configured exposure rule. Build: cargo build -p statevec --example core_bank.
//! This has its own schema/revision; it is not an upgrade of another Bank demo.
use statevec::api::plugin::*;
use statevec::prelude::*;

#[path = "core_bank/schema.rs"]
mod bank;
use bank::*;

#[path = "core_bank/business.rs"]
mod business;
use business::{Policy, Runtime};

struct Module;
struct Configured {
    schema: SchemaRegistry,
    policy: Policy,
    revision: String,
}

impl CoreModule for Module {
    type Configured = Configured;
    fn configure(&self, config: CoreConfig<'_>) -> Result<Configured, ConfigureError> {
        let policy: Policy = serde_json::from_slice(config.bytes).map_err(|_| ConfigureError::Invalid { code: 1 })?;
        if policy.min_amount == 0 || policy.min_amount > policy.max_balance {
            return Err(ConfigureError::Invalid { code: 2 });
        }
        Ok(Configured {
            schema: registry(),
            revision: format!(
                "core-bank-v1;max={};min={}{}",
                policy.max_balance,
                policy.min_amount,
                if policy.retain_migration { ";migration=retain-v1" } else { "" }
            ),
            policy,
        })
    }
}
impl ConfiguredCore for Configured {
    type Runtime = Runtime;
    fn metadata(&self) -> CoreMetadata<'_> {
        CoreMetadata::new("core-bank", &self.revision, &self.schema)
    }
    fn create_instance(&self) -> Result<Runtime, CoreCreateError> {
        Ok(Runtime(self.policy))
    }
}

statevec::api::export_core_plugin!(Module);
