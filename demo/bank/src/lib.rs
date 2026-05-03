// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

//! Simple bank demo: deposit, withdraw, and in-bank transfer with double-entry bookkeeping.

use statevec::command_dispatch;
use statevec::event::GeneratedEventAccess;
use statevec::record::RecordSchema;
use statevec::{
    BizInvariantReadContext, InvariantReadContextExt, RuntimeCommandEnvelope, RuntimeHostContext,
    RuntimePlugin, RuntimePluginError, RuntimePluginFactory, RuntimePluginLoadError,
    RuntimePluginUnloadError, TypedTxContext,
};
#[allow(unused_imports)]
use statevec::{statevec_api, statevec_model};

mod schema;
pub use schema::*;

const PLATFORM_ACCOUNT_ID: u64 = 0;
const ENTRY_TYPE_DEPOSIT: u8 = 1;
const ENTRY_TYPE_WITHDRAW: u8 = 2;
const ENTRY_TYPE_TRANSFER: u8 = 3;

#[derive(Debug)]
pub enum BankError {
    Host(String),
    Message(String),
    UnknownCommand(u8),
}

impl From<statevec::RuntimeHostError> for BankError {
    fn from(value: statevec::RuntimeHostError) -> Self {
        Self::Host(value.to_string())
    }
}

impl std::fmt::Display for BankError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Host(err) => write!(f, "{err}"),
            Self::Message(msg) => write!(f, "{msg}"),
            Self::UnknownCommand(kind) => write!(f, "unknown command kind {kind}"),
        }
    }
}

impl std::error::Error for BankError {}

pub(crate) struct BankRuntime;

impl BankRuntime {
    fn bad(msg: impl Into<String>) -> BankError {
        BankError::Message(msg.into())
    }

    fn adjust_account<Tx: TypedTxContext<Error: Into<BankError>> + ?Sized>(
        account_id: u64,
        tx: &mut Tx,
        credit_delta: u64,
        debit_delta: u64,
        check_nonneg: bool,
    ) -> Result<(u64, u64), BankError> {
        tx.update_or_create_typed_by_pk::<Account, _, _, _, _>(
            Account::pk(account_id),
            |r| -> Result<(u64, u64), BankError> {
                let new_credit = r
                    .total_credit()
                    .checked_add(credit_delta)
                    .ok_or_else(|| Self::bad("credit overflow"))?;
                let new_debit = r
                    .total_debit()
                    .checked_add(debit_delta)
                    .ok_or_else(|| Self::bad("debit overflow"))?;
                if check_nonneg && new_credit < new_debit {
                    return Err(Self::bad("insufficient balance"));
                }
                r.set_total_credit(new_credit);
                r.set_total_debit(new_debit);
                Ok((new_credit, new_debit))
            },
            |r| -> Result<(u64, u64), BankError> {
                if check_nonneg && credit_delta < debit_delta {
                    return Err(Self::bad("insufficient balance"));
                }
                r.init_account_id(account_id);
                r.set_total_credit(credit_delta);
                r.set_total_debit(debit_delta);
                Ok((credit_delta, debit_delta))
            },
        )
        .map_err(Into::into)?
    }

    fn emit_balance_changed<Tx: TypedTxContext + ?Sized>(
        tx: &mut Tx,
        account_id: u64,
        new_credit: u64,
        new_debit: u64,
        entry_type: u8,
    ) {
        tx.emit_typed_event::<BalanceChanged>(
            BalanceChanged::builder()
                .set_account_id(account_id)
                .set_new_credit(new_credit)
                .set_new_debit(new_debit)
                .set_entry_type(entry_type)
                .build(),
        );
    }

    fn handle_deposit<Tx: TypedTxContext<Error: Into<BankError>> + ?Sized>(
        &self,
        tx: &mut Tx,
        command: DepositAccess<'_>,
    ) -> Result<(), BankError> {
        let account_id = command.account_id();
        let amount = command.amount();
        if account_id == PLATFORM_ACCOUNT_ID {
            return Err(Self::bad("cannot deposit to platform account"));
        }
        if amount == 0 {
            return Err(Self::bad("amount must be positive"));
        }

        let (uc, ud) = Self::adjust_account(account_id, tx, amount, 0, true)?;
        let (pc, pd) = Self::adjust_account(PLATFORM_ACCOUNT_ID, tx, 0, amount, false)?;
        Self::emit_balance_changed(tx, account_id, uc, ud, ENTRY_TYPE_DEPOSIT);
        Self::emit_balance_changed(tx, PLATFORM_ACCOUNT_ID, pc, pd, ENTRY_TYPE_DEPOSIT);
        Ok(())
    }

    fn handle_withdraw<Tx: TypedTxContext<Error: Into<BankError>> + ?Sized>(
        &self,
        tx: &mut Tx,
        command: WithdrawAccess<'_>,
    ) -> Result<(), BankError> {
        let account_id = command.account_id();
        let amount = command.amount();
        if account_id == PLATFORM_ACCOUNT_ID {
            return Err(Self::bad("cannot withdraw from platform account"));
        }
        if amount == 0 {
            return Err(Self::bad("amount must be positive"));
        }

        let (uc, ud) = Self::adjust_account(account_id, tx, 0, amount, true)?;
        let (pc, pd) = Self::adjust_account(PLATFORM_ACCOUNT_ID, tx, amount, 0, false)?;
        Self::emit_balance_changed(tx, account_id, uc, ud, ENTRY_TYPE_WITHDRAW);
        Self::emit_balance_changed(tx, PLATFORM_ACCOUNT_ID, pc, pd, ENTRY_TYPE_WITHDRAW);
        Ok(())
    }

    fn handle_transfer<Tx: TypedTxContext<Error: Into<BankError>> + ?Sized>(
        &self,
        tx: &mut Tx,
        command: TransferAccess<'_>,
    ) -> Result<(), BankError> {
        let from = command.from_account_id();
        let to = command.to_account_id();
        let amount = command.amount();
        if from == PLATFORM_ACCOUNT_ID || to == PLATFORM_ACCOUNT_ID {
            return Err(Self::bad(
                "platform account cannot participate in transfers",
            ));
        }
        if from == to {
            return Err(Self::bad("cannot transfer to self"));
        }
        if amount == 0 {
            return Err(Self::bad("amount must be positive"));
        }

        let (fc, fd) = Self::adjust_account(from, tx, 0, amount, true)?;
        let (tc, td) = Self::adjust_account(to, tx, amount, 0, true)?;
        Self::emit_balance_changed(tx, from, fc, fd, ENTRY_TYPE_TRANSFER);
        Self::emit_balance_changed(tx, to, tc, td, ENTRY_TYPE_TRANSFER);
        Ok(())
    }
}

command_dispatch! {
    fn try_dispatch_bank;
    runtime = BankRuntime;
    error = BankError;
    Deposit => Self::handle_deposit,
    Withdraw => Self::handle_withdraw,
    Transfer => Self::handle_transfer,
}

impl RuntimePlugin for BankRuntime {
    fn name(&self) -> &'static str {
        "bank"
    }

    fn schema_registry(&self) -> statevec::SchemaRegistry {
        registry()
    }

    fn run_tx(
        &self,
        tx: &mut dyn RuntimeHostContext,
        command: &dyn RuntimeCommandEnvelope,
    ) -> Result<(), RuntimePluginError> {
        if self
            .try_dispatch_bank(tx, command)
            .map_err(|e| RuntimePluginError::new(e.to_string()))?
        {
            return Ok(());
        }
        Err(RuntimePluginError::new(format!(
            "unknown command kind {}",
            command.command_kind()
        )))
    }

    fn validate_biz_invariants(&self, ctx: &dyn BizInvariantReadContext) -> Result<(), String> {
        let mut keys = Vec::new();
        ctx.for_each_record_key_raw(Account::KIND, &mut |key| keys.push(key))
            .map_err(|e| e.to_string())?;

        let mut total_balance: i128 = 0;
        for key in keys {
            let Some((account_id, credit, debit)) = ctx
                .with_read_typed::<Account, _, _>(key.sys_id, |acc| {
                    (acc.account_id(), acc.total_credit(), acc.total_debit())
                })
                .map_err(|e| e.to_string())?
            else {
                continue;
            };

            if account_id != PLATFORM_ACCOUNT_ID && credit < debit {
                return Err(format!(
                    "account {account_id} has negative balance: credit={credit} debit={debit}"
                ));
            }
            total_balance += i128::from(credit) - i128::from(debit);
        }

        if total_balance != 0 {
            return Err(format!(
                "total balance across all accounts is not zero: {total_balance}"
            ));
        }
        Ok(())
    }

    fn on_unload(&mut self) -> Result<(), RuntimePluginUnloadError> {
        Ok(())
    }
}

struct BankRuntimeFactory;

impl RuntimePluginFactory for BankRuntimeFactory {
    fn plugin_name(&self) -> &'static str {
        "bank"
    }

    fn schema_registry(&self) -> statevec::SchemaRegistry {
        registry()
    }

    fn create(
        &self,
        _plugin_config_text: &str,
    ) -> Result<Box<dyn RuntimePlugin>, RuntimePluginLoadError> {
        Ok(Box::new(BankRuntime))
    }
}

statevec::export_runtime_plugin!(Box::new(BankRuntimeFactory));
