//! Static gaps, explicit reserved bytes and tail slack through real Core tables.
//! This qualification fixture deliberately accepts raw payloads for host checks.
use statevec::RuntimeCommandEnvelope;
use statevec::api::plugin::*;
use statevec::api::{RuntimeCommandRef, TxWriteContext};
use statevec::model::{
    CommandSchema, FieldDefinition, FieldType, RecordDefinition, RecordKey, SchemaRegistry, UniqueKeyDefinition,
    Version,
};
use statevec::prelude::command;

/// A whole 40-byte raw row, in five words: any bytes are schema-valid, so the
/// host's own write checks decide its gaps and tail.
macro_rules! raw_row_command {
    ($name:ident, $kind:literal) => {
        #[command(kind = $kind)]
        pub struct $name {
            #[field(index = 1)]
            pub w0: u64,
            #[field(index = 2)]
            pub w1: u64,
            #[field(index = 3)]
            pub w2: u64,
            #[field(index = 4)]
            pub w3: u64,
            #[field(index = 5)]
            pub w4: u64,
        }
    };
}
raw_row_command!(CreateRow, 1);
raw_row_command!(UpdateRow, 2);

const FIELD: FieldDefinition = FieldDefinition {
    name: "id",
    field_index: 1,
    offset: 0,
    ty: FieldType::U64,
    len: 8,
    rust_type_name: "u64",
    enum_type_name: None,
    decimal_scale: None,
    semantic: None,
    immutable: false,
};
static ROW: RecordDefinition = RecordDefinition {
    kind: 91,
    name: "GapProfile",
    data_size: 40,
    version: 1,
    fields: &[
        FIELD,
        FieldDefinition { name: "value", field_index: 2, offset: 16, ..FIELD },
        FieldDefinition {
            name: "tag",
            field_index: 3,
            offset: 32,
            ty: FieldType::U8,
            len: 1,
            rust_type_name: "u8",
            ..FIELD
        },
    ],
    reserved_fields: &[FieldDefinition { name: "reserved", field_index: 4, offset: 24, ..FIELD }],
    unique_keys: &[UniqueKeyDefinition { id: 0, name: "id", encode: None, fields: &["id"] }],
    canonical_indexes: &[],
};
struct Module;
struct Configured {
    schema: SchemaRegistry,
    revision: &'static str,
}
struct Runtime;
impl CoreModule for Module {
    type Configured = Configured;
    fn configure(&self, config: CoreConfig<'_>) -> Result<Configured, ConfigureError> {
        let revision = match config.bytes {
            b"{}" => "gap-profile-v1",
            b"next" => "gap-profile-v2",
            _ => return Err(ConfigureError::Invalid { code: 1 }),
        };
        let schema = SchemaRegistry::new(
            Version::new(1, 0),
            &[ROW],
            &[*CreateRow::definition(), *UpdateRow::definition()],
            &[],
            &[],
        )
        .with_command_validator::<CreateRow>()
        .with_command_validator::<UpdateRow>();
        Ok(Configured { schema, revision })
    }
}
impl ConfiguredCore for Configured {
    type Runtime = Runtime;
    fn metadata(&self) -> CoreMetadata<'_> {
        CoreMetadata::new("gap-profile", self.revision, &self.schema)
    }
    fn create_instance(&self) -> Result<Runtime, CoreCreateError> {
        Ok(Runtime)
    }
}
impl CoreRuntime for Runtime {
    const TX_INVARIANTS: Option<TxInvariantHook<Self>> = None;
    fn initialize_core(&self, tx: &mut PluginTx<'_>, input: &[u8]) -> Result<(), CoreTxError> {
        if !input.is_empty() {
            tx.create_raw(91, input.to_vec())?;
        }
        Ok(())
    }
    fn migrate(&self, tx: &mut MigrationTx<'_>, _: CoreVersion, input: &[u8]) -> Result<(), CoreTxError> {
        if !input.is_empty() {
            tx.records().1.create_record(91, input)?;
        }
        Ok(())
    }
    fn run_tx(&self, tx: &mut PluginTx<'_>, command: &RuntimeCommandRef<'_>) -> Result<(), CoreTxError> {
        let bytes = command.payload();
        if command.command_kind() == CreateRow::KIND {
            tx.create_raw(91, bytes.to_vec())?;
        } else {
            let sys_id = u64::from_le_bytes(bytes[..8].try_into().unwrap());
            tx.update_raw(RecordKey { kind: 91, sys_id }, |row| row.copy_from_slice(bytes))?;
        }
        Ok(())
    }
}
statevec::api::export_core_plugin!(Module);
