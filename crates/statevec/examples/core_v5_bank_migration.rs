//! Bank-v1 to Bank-v2 qualification pair. Explicitly interprets the v1 layout,
//! changes record layout/indexes, and continues ordinary deposits after restore.
use statevec::api::v1::*;
use statevec::prelude::*;

#[schema_module(version = "2.0")]
mod bank {
    use super::*;
    #[record(kind = 1, record_len = 256, unique_key(id = 0, fields = [account_id]),
        index(id = 0, name = "by_credit", fields = [credit]))]
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
        #[field(index = 6)]
        pub limit: u64,
    }
    #[command(kind = 1)]
    pub struct Deposit {
        #[field(index = 1)]
        pub account_id: u64,
        #[field(index = 2)]
        pub amount: u64,
    }
}
use bank::*;

struct Module;
struct Configured(SchemaRegistry);
struct Runtime;
fn reject(code: u16) -> CoreTxError {
    CoreTxError::Rejected(BusinessRejectCode::new(code).unwrap())
}
impl CoreModule for Module {
    type Configured = Configured;
    fn configure(&self, config: CoreConfig<'_>) -> Result<Configured, ConfigureError> {
        if config.bytes != b"bank-layout-v2" { return Err(ConfigureError::Invalid { code: 1 }) }
        Ok(Configured(registry()))
    }
}
impl ConfiguredCore for Configured {
    type Runtime = Runtime;
    fn metadata(&self) -> CoreMetadata<'_> {
        CoreMetadata { name: "core-bank-migration", revision: "bank-layout-v2-limit100", schema: &self.0 }
    }
    fn create_instance(&self) -> Result<Runtime, CoreCreateError> { Ok(Runtime) }
}
impl CoreRuntime for Runtime {
    fn initialize_core(&self, _: &mut PluginTx<'_>, _: &[u8]) -> Result<(), CoreTxError> { Ok(()) }
    fn migrate(&self, tx: &mut MigrationTx<'_>, _: CoreVersion, input: &[u8]) -> Result<(), CoreTxError> {
        if !matches!(input, b"layout" | b"omit" | b"reject" | b"replace-delete-create") { return Err(reject(5)) }
        if input == b"omit" { return Ok(()) }
        let (source, mut target) = tx.records();
        let mut result = Ok(());
        source.for_each_key(Account::KIND, &mut |key| {
            if result.is_err() { return }
            result = source.with_record(key, |bytes| -> Result<(), CoreTxError> {
                if bytes.len() != 104 || bytes[40..].iter().any(|byte| *byte != 0) { return Err(reject(5)) }
                let word = |offset| u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap());
                let mut data = vec![0; Account::DATA_LEN];
                {
                    let mut row = Account::wrap_new(&mut data);
                    row.init_account_id(if input == b"replace-delete-create" && word(0) == 2 { 99 } else { word(0) });
                    row.set_credit(word(8));
                    row.set_debit(word(16));
                    row.set_time_ns(word(24));
                    row.set_position(word(32));
                    row.set_limit(100);
                }
                if input == b"replace-delete-create" && word(0) == 2 {
                    target.delete_record(key)?;
                    target.create_record(Account::KIND, &data)?;
                } else {
                    target.preserve_record(key, Account::KIND, &data)?;
                }
                Ok(())
            }).map_err(CoreTxError::from).and_then(|row| row.ok_or_else(|| reject(5))?);
        })?;
        result?;
        if input == b"reject" { Err(reject(42)) } else { Ok(()) }
    }
    const TX_INVARIANTS: Option<TxInvariantHook<Self>> = Some(|_, view| {
        let mut invalid = false;
        view.for_each_after_change(&mut |change| {
            let bytes = match change {
                CandidateAfterChange::New { after, .. } | CandidateAfterChange::Update { after, .. } => after,
                CandidateAfterChange::Delete { .. } => return,
            };
            let row = Account::wrap(bytes);
            invalid |= row.account_id() != 0
                && row.credit().checked_sub(row.debit()).is_none_or(|value| value > row.limit());
        })?;
        if invalid { Err(reject(4)) } else { Ok(()) }
    });
    fn preflight_command(&self, command: &RuntimeCommandRef<'_>) -> Result<(), statevec::model::CommandSchemaError> {
        match command.command_kind() {
            Deposit::KIND => Deposit::validate_payload(command.payload()),
            kind => Err(statevec::model::CommandSchemaError::UnsupportedKind { kind }),
        }
    }
    fn run_tx(&self, tx: &mut PluginTx<'_>, command: &RuntimeCommandRef<'_>) -> Result<(), CoreTxError> {
        let input = Deposit::wrap(command.payload());
        let credit = tx.with_read_typed_by_unique_key::<Account, _, _, _>(Account::unique_key(input.account_id()), |row| row.credit())?
            .ok_or_else(|| reject(1))?;
        // This exercises the newly built canonical index before any write.
        let count = TxReadContext::count_index_prefix_capped_raw(tx, Account::KIND, 0, &credit.to_be_bytes(), 10)?;
        if count != statevec::api::CanonicalIndexCount::Exact(1) { return Err(reject(6)) }
        tx.update_typed_by_unique_key::<Account, _, _, _>(Account::unique_key(input.account_id()), |row| {
            let credit = row.credit().checked_add(input.amount()).ok_or_else(|| reject(3))?;
            row.set_credit(credit);
            Ok::<_, CoreTxError>(())
        })?.ok_or_else(|| reject(1))??;
        tx.update_typed_by_unique_key::<Account, _, _, _>(Account::unique_key(0), |row| {
            row.set_debit(row.debit().checked_add(input.amount()).ok_or_else(|| reject(3))?);
            Ok::<_, CoreTxError>(())
        })?.ok_or_else(|| reject(1))??;
        Ok(())
    }
}
statevec::api::export_core_plugin_v5!(Module);
