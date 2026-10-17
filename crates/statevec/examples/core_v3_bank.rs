//! A stateful Core V3 example with double-entry deposits/transfers and a
//! configured exposure rule. Build: cargo build -p statevec --example core_v3_bank.
//! This has its own schema/revision; it is not an upgrade of another Bank demo.
use statevec::api::v1::*;
use statevec::prelude::*;

#[schema_module(version = "1.0")]
mod bank {
    use super::*;
    #[record(kind = 1, record_len = 128, unique_key(id = 0, fields = [account_id]))]
    pub struct Account {
        #[field(index = 1, immutable = true)]
        pub account_id: u64,
        #[field(index = 2)]
        pub credit: u64,
        #[field(index = 3)]
        pub debit: u64,
        #[field(index = 4)]
        pub time_ns: u64,
        #[field(index = 5)]
        pub position: u64,
    }
    #[command(kind = 1)]
    pub struct Deposit {
        #[field(index = 1)]
        pub account_id: u64,
        #[field(index = 2)]
        pub amount: u64,
    }
    #[command(kind = 2)]
    pub struct Transfer {
        #[field(index = 1)]
        pub from: u64,
        #[field(index = 2)]
        pub to: u64,
        #[field(index = 3)]
        pub amount: u64,
    }
    #[event(kind = 1, inline_response = true)]
    pub struct Balance {
        #[field(index = 1)]
        pub account_id: u64,
        #[field(index = 2)]
        pub credit: u64,
        #[field(index = 3)]
        pub debit: u64,
        #[field(index = 4)]
        pub time_ns: u64,
        #[field(index = 5)]
        pub position: u64,
    }
}
use bank::*;

#[derive(Clone, Copy, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Policy {
    max_balance: u64,
    min_amount: u64,
}
struct Module;
struct Configured {
    schema: SchemaRegistry,
    policy: Policy,
    revision: String,
}
struct Runtime(Policy);

impl CoreModule for Module {
    type Configured = Configured;
    fn configure(&self, config: CoreConfig<'_>) -> Result<Configured, ConfigureError> {
        let policy: Policy =
            serde_json::from_slice(config.bytes).map_err(|_| ConfigureError::Invalid { code: 1 })?;
        if policy.min_amount == 0 || policy.min_amount > policy.max_balance {
            return Err(ConfigureError::Invalid { code: 2 });
        }
        Ok(Configured {
            schema: registry(),
            revision: format!("core-bank-v1;max={};min={}", policy.max_balance, policy.min_amount),
            policy,
        })
    }
}
impl ConfiguredCore for Configured {
    type Runtime = Runtime;
    fn metadata(&self) -> CoreMetadata<'_> {
        CoreMetadata {
            name: "core-bank",
            revision: &self.revision,
            schema: &self.schema,
        }
    }
    fn create_instance(&self) -> Result<Runtime, CoreCreateError> {
        Ok(Runtime(self.policy))
    }
}

// Business codes belong to this example's revision: 1 invalid amount/account,
// 2 insufficient funds, 3 arithmetic overflow, 4 candidate exposure limit.
fn reject(code: u16) -> CoreTxError {
    CoreTxError::Rejected(BusinessRejectCode::new(code).expect("example rejection code"))
}
impl Runtime {
    fn adjust(tx: &mut PluginTx<'_>, id: u64, credit: u64, debit: u64) -> Result<(), CoreTxError> {
        let time = tx.ref_tx_time_ns()?;
        let position = tx.tx_seq()?;
        let balances = tx.update_or_create_typed_by_unique_key::<Account, _, _, _, _>(
            Account::unique_key(id),
            |row| -> Result<_, CoreTxError> {
                let credit = row.credit().checked_add(credit).ok_or_else(|| reject(3))?;
                let debit = row.debit().checked_add(debit).ok_or_else(|| reject(3))?;
                if id != 0 && credit < debit {
                    return Err(reject(2));
                }
                row.set_credit(credit);
                row.set_debit(debit);
                row.set_time_ns(time);
                row.set_position(position);
                Ok((credit, debit))
            },
            |row| -> Result<_, CoreTxError> {
                if id != 0 && credit < debit {
                    return Err(reject(2));
                }
                row.init_account_id(id);
                row.set_credit(credit);
                row.set_debit(debit);
                row.set_time_ns(time);
                row.set_position(position);
                Ok((credit, debit))
            },
        )??;
        tx.emit_typed_event::<Balance>(
            Balance::builder()
                .set_account_id(id)
                .set_credit(balances.0)
                .set_debit(balances.1)
                .set_time_ns(time)
                .set_position(position)
                .build()
                .expect("fixed fields"),
        );
        Ok(())
    }
    fn rules(&self, view: &dyn CandidateRead) -> Result<(), CoreTxError> {
        let mut violation = false;
        view.for_each_after_change(&mut |change| {
            let data = match change {
                CandidateAfterChange::New { after, .. }
                | CandidateAfterChange::Update { after, .. } => after,
                CandidateAfterChange::Delete { .. } => return,
            };
            let row = Account::wrap(data);
            violation |= row.account_id() != 0 && row.credit() - row.debit() > self.0.max_balance;
        })?;
        if violation {
            Err(reject(4))
        } else {
            Ok(())
        }
    }
}
impl CoreRuntime for Runtime {
    const TX_INVARIANTS: Option<TxInvariantHook<Self>> = Some(Self::rules);
    fn preflight_command(
        &self,
        command: &RuntimeCommandRef<'_>,
    ) -> Result<(), statevec::model::CommandSchemaError> {
        match command.command_kind() {
            Deposit::KIND => Deposit::validate_payload(command.payload()),
            Transfer::KIND => Transfer::validate_payload(command.payload()),
            kind => Err(statevec::model::CommandSchemaError::UnsupportedKind { kind }),
        }
    }
    fn run_tx(
        &self,
        tx: &mut PluginTx<'_>,
        command: &RuntimeCommandRef<'_>,
    ) -> Result<(), CoreTxError> {
        match command.command_kind() {
            Deposit::KIND => {
                let input = Deposit::wrap(command.payload());
                if input.account_id() == 0 || input.amount() < self.0.min_amount {
                    return Err(reject(1));
                }
                Self::adjust(tx, input.account_id(), input.amount(), 0)?;
                Self::adjust(tx, 0, 0, input.amount())
            }
            Transfer::KIND => {
                let input = Transfer::wrap(command.payload());
                if input.from() == 0
                    || input.to() == 0
                    || input.from() == input.to()
                    || input.amount() < self.0.min_amount
                {
                    return Err(reject(1));
                }
                Self::adjust(tx, input.from(), 0, input.amount())?;
                Self::adjust(tx, input.to(), input.amount(), 0)
            }
            kind => Err(CoreTxError::Execution(CoreExecutionError::Schema(
                CoreSchemaError::UnsupportedKind { kind },
            ))),
        }
    }
}
statevec::api::export_core_plugin_v3!(Module);
