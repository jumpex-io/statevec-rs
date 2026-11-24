//! Shared source Bank semantics used by the additive target without changing
//! the existing command and event definitions.
use statevec::api::plugin::*;
use statevec::prelude::*;
use super::bank::*;

#[derive(Clone, Copy, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Policy {
    pub(crate) max_balance: u64,
    pub(crate) min_amount: u64,
    #[serde(default)]
    pub(crate) retain_migration: bool,
}
pub(crate) struct Runtime(pub(crate) Policy);

// Business codes belong to this example's revision: 1 invalid amount/account,
// 2 insufficient funds, 3 arithmetic overflow, 4 candidate exposure limit,
// 5 unsupported migration source.
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
    pub(crate) fn rules(&self, view: &dyn CandidateRead) -> Result<(), CoreTxError> {
        let mut violation = false;
        view.for_each_after_change(&mut |change| {
            let data = match change {
                CandidateAfterChange::New { key, after } | CandidateAfterChange::Update { key, after }
                    if key.kind == Account::KIND => after,
                _ => return,
            };
            let row = Account::wrap(data);
            violation |= row.account_id() != 0 && row.credit() - row.debit() > self.0.max_balance;
        })?;
        if violation { Err(reject(4)) } else { Ok(()) }
    }
}
impl CoreRuntime for Runtime {
    fn initialize_core(&self, _: &mut PluginTx<'_>, _: &[u8]) -> Result<(), CoreTxError> {
        Ok(())
    }
    fn migrate(&self, tx: &mut MigrationTx<'_>, _: CoreVersion, input: &[u8]) -> Result<(), CoreTxError> {
        if !self.0.retain_migration || input != b"retain-bank-v1" {
            return Err(reject(5));
        }
        // This input explicitly selects the Bank-v1 source layout. The host
        // additionally binds the actual source V and checks untouched layouts.
        // Candidate change visitors intentionally do not enumerate retained
        // rows; a new whole-state policy must inspect them here.
        let (source, _) = tx.records();
        let mut result = Ok(());
        source.for_each_key(Account::KIND, &mut |key| {
            if result.is_err() {
                return;
            }
            result = source
                .with_record(key, |bytes| {
                    if bytes.len() != 104 {
                        return Err(reject(5));
                    }
                    let row = Account::wrap(bytes);
                    let balance = row.credit().checked_sub(row.debit());
                    if row.account_id() != 0 && balance.is_none_or(|balance| balance > self.0.max_balance) {
                        return Err(reject(4));
                    }
                    Ok(())
                })
                .map_err(CoreTxError::from)
                .and_then(|row| row.ok_or_else(|| reject(5))?);
        })?;
        result
    }
    const TX_INVARIANTS: Option<TxInvariantHook<Self>> = Some(Self::rules);
    fn preflight_command(&self, command: &RuntimeCommandRef<'_>) -> Result<(), statevec::model::CommandSchemaError> {
        match command.command_kind() {
            Deposit::KIND => Deposit::validate_payload(command.payload()),
            Transfer::KIND => Transfer::validate_payload(command.payload()),
            kind => Err(statevec::model::CommandSchemaError::UnsupportedKind { kind }),
        }
    }
    fn run_tx(&self, tx: &mut PluginTx<'_>, command: &RuntimeCommandRef<'_>) -> Result<(), CoreTxError> {
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
            kind => Err(CoreTxError::Execution(CoreExecutionError::Schema(CoreSchemaError::UnsupportedKind { kind }))),
        }
    }
}
