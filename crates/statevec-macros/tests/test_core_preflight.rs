// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

//! The exported Core preflight checks each command against the schema the Core
//! declared before author code runs. These authors accept every command.

#![allow(dead_code)]

use statevec_api::RuntimeCommandRef;
use statevec_api::plugin::__export::{CoreExportTable, configure};
use statevec_api::plugin::ffi::{SvBytes, SvCommand, SvCore, SvDiscovery};
use statevec_api::plugin::*;
use statevec_macros::{EnumU8, schema_module};
use statevec_model::{CommandSchema, CommandSchemaError, GeneratedCommandAccess, SchemaRegistry};
use std::ffi::c_void;
use test_case::test_case;

// The macro recognizes this name and generates Vec<u8> builders.
struct VarBytes;

#[derive(EnumU8)]
#[repr(u8)]
pub enum Side {
    Buy = 1,
    Sell = 2,
}

#[schema_module(version = "1.0")]
mod schema {
    use super::Side;
    use statevec_macros::command;

    #[command(kind = 1)]
    pub struct Place {
        #[field(index = 1, enum_u8)]
        pub side: Side,
        #[field(index = 2)]
        pub note: VarBytes,
        #[field(index = 3)]
        pub amount: u64,
    }

    #[command(kind = 2)]
    pub struct Cancel {
        #[field(index = 1)]
        pub order: u64,
    }
}
use schema::{Cancel, Place};

/// Other modules' enums under the same name: one accepts one more value, the
/// other the same values in another order.
mod wider {
    use statevec_macros::EnumU8;

    #[derive(EnumU8)]
    #[repr(u8)]
    pub enum Side {
        Buy = 1,
        Sell = 2,
        Hold = 3,
    }
}
mod reordered {
    use statevec_macros::EnumU8;

    #[derive(EnumU8)]
    #[repr(u8)]
    pub enum Side {
        Sell = 2,
        Buy = 1,
    }
}

/// `Place`'s payload layout, decoding `wider::Side`.
#[schema_module(version = "1.0")]
mod wider_schema {
    use super::wider::Side;
    use statevec_macros::command;

    #[command(kind = 1)]
    pub struct Place {
        #[field(index = 1, enum_u8)]
        pub side: Side,
        #[field(index = 2)]
        pub note: VarBytes,
        #[field(index = 3)]
        pub amount: u64,
    }
}

/// Two enum fields named `Side`: the first decodes the exported values, the
/// second `wider::Side`.
#[schema_module(version = "1.0")]
mod twin_schema {
    use super::Side;
    use super::wider::Side as WiderSide;
    use statevec_macros::command;

    #[command(kind = 3)]
    pub struct Hedge {
        #[field(index = 1, enum_u8)]
        pub side: Side,
        #[field(index = 2, enum_u8)]
        pub hedge: WiderSide,
    }
}

/// `Place`'s payload layout, decoding `reordered::Side`.
#[schema_module(version = "1.0")]
mod reordered_schema {
    use super::reordered::Side;
    use statevec_macros::command;

    #[command(kind = 1)]
    pub struct Place {
        #[field(index = 1, enum_u8)]
        pub side: Side,
        #[field(index = 2)]
        pub note: VarBytes,
        #[field(index = 3)]
        pub amount: u64,
    }
}

/// The command definitions of `schema`, composed by hand.
fn hand_built() -> SchemaRegistry {
    SchemaRegistry::new(
        schema::SCHEMA_VERSION,
        &[],
        &[*Place::definition(), *Cancel::definition()],
        &[],
        &[*<Side as statevec_model::EnumU8>::DEFINITION],
    )
}

struct Module;
struct Configured(SchemaRegistry);
struct Runtime;
impl CoreModule for Module {
    type Configured = Configured;
    fn configure(&self, config: CoreConfig<'_>) -> Result<Configured, ConfigureError> {
        Ok(Configured(match config.bytes {
            b"generated" => schema::registry(),
            b"hand-built" => hand_built(),
            b"registered" => hand_built()
                .with_command_validator::<Place>()
                .with_command_validator::<Cancel>(),
            _ => return Err(ConfigureError::Invalid { code: 1 }),
        }))
    }
}
impl ConfiguredCore for Configured {
    type Runtime = Runtime;
    fn metadata(&self) -> CoreMetadata<'_> {
        CoreMetadata::new("accept-all", "accept-all-v1", &self.0)
    }
    fn create_instance(&self) -> Result<Runtime, CoreCreateError> {
        Ok(Runtime)
    }
}
impl CoreRuntime for Runtime {
    const TX_INVARIANTS: Option<TxInvariantHook<Self>> = None;
    fn initialize_core(&self, _: &mut PluginTx<'_>, _: &[u8]) -> Result<(), CoreTxError> {
        Ok(())
    }
    fn migrate(&self, _: &mut MigrationTx<'_>, _: CoreVersion, _: &[u8]) -> Result<(), CoreTxError> {
        Ok(())
    }
    // The author forgot schema validation for every kind.
    fn preflight_command(&self, _: &RuntimeCommandRef<'_>) -> Result<(), CommandSchemaError> {
        Ok(())
    }
    fn run_tx(&self, _: &mut PluginTx<'_>, _: &RuntimeCommandRef<'_>) -> Result<(), CoreTxError> {
        panic!("preflight tests never execute a command")
    }
}

unsafe extern "C" fn unused(_: SvBytes, _: *mut *mut c_void) -> SvStatus {
    panic!("these tests configure through the SDK adapter directly")
}
// SAFETY: `unused` never returns.
static EXPORT: CoreExportTable = unsafe { CoreExportTable::new::<Module>(unused) };

fn table() -> &'static SvCore {
    let mut discovery = SvDiscovery { table: std::ptr::null(), table_bytes: 0 };
    // SAFETY: aligned, exact-size local output; EXPORT is static.
    assert_eq!(unsafe { EXPORT.discover(size_of::<SvDiscovery>() as u64, &mut discovery) }, 0);
    // SAFETY: the SDK returned its immutable static table.
    unsafe { &*discovery.table.cast::<SvCore>() }
}

/// A configured Core and one instance, retired on drop.
struct Loaded {
    configured: *mut c_void,
    instance: *mut c_void,
}
impl Loaded {
    fn configure(config: &[u8]) -> Result<Self, SvStatus> {
        let mut configured = std::ptr::null_mut();
        // SAFETY: scoped input and output with the real SDK configure adapter.
        let status = unsafe { configure(&Module, SvBytes::borrowed(config), &mut configured) };
        if !status.is_success() {
            assert!(configured.is_null(), "a refused configuration transfers no handle");
            return Err(status);
        }
        let mut instance = std::ptr::null_mut();
        // SAFETY: live configured handle and a writable instance slot.
        assert!(unsafe { (table().create_instance.unwrap())(configured, &mut instance) }.is_success());
        Ok(Self { configured, instance })
    }
    /// Status and reported ordinal of one exported preflight call.
    fn preflight(&self, commands: &[(u16, &[u8])]) -> (Option<CoreError>, u64) {
        let commands: Vec<_> = commands
            .iter()
            .map(|(kind, payload)| SvCommand {
                kind: *kind,
                reserved16: 0,
                reserved32: 0,
                ext_seq: 1,
                ref_ext_time_us: 5,
                payload: SvBytes::borrowed(payload),
            })
            .collect();
        let mut ordinal = u64::MAX;
        // SAFETY: live instance and a scoped initialized command array.
        let status = unsafe {
            (table().preflight_batch.unwrap())(self.instance, commands.as_ptr(), commands.len() as u64, &mut ordinal)
        };
        (status.decode_for(CoreOperation::PreflightBatch).unwrap(), ordinal)
    }
}
impl Drop for Loaded {
    fn drop(&mut self) {
        // SAFETY: only allocations returned by the SDK and not yet retired.
        unsafe {
            assert!((table().destroy_instance.unwrap())(&mut self.instance).is_success());
            assert!((table().destroy_configured.unwrap())(&mut self.configured).is_success());
        }
    }
}

fn place() -> Vec<u8> {
    Place::builder()
        .set_side(Side::Sell)
        .set_note(vec![7, 8, 9])
        .set_amount(5)
        .build()
        .unwrap()
}
fn cancel() -> Vec<u8> {
    Cancel::builder().set_order(3).build().unwrap()
}
fn schema_refusal(source: CoreSchemaError) -> Option<CoreError> {
    Some(CoreError::Transaction(CoreTxError::Execution(CoreExecutionError::Schema(source))))
}

#[derive(Debug, Clone, Copy)]
enum Malformed {
    ShortPayload,
    TrailingBytes,
    InvalidEnum,
    UnknownKind,
}

#[test_case(Malformed::ShortPayload; "short payload")]
#[test_case(Malformed::TrailingBytes; "trailing bytes")]
#[test_case(Malformed::InvalidEnum; "invalid enum")]
#[test_case(Malformed::UnknownKind; "unknown kind")]
fn preflight_refuses_a_schema_invalid_command_the_author_accepts(case: Malformed) {
    let valid = place();
    let (kind, payload, expected) = match case {
        Malformed::ShortPayload => (
            Place::KIND,
            valid[..valid.len() - 1].to_vec(),
            CoreSchemaError::FieldAccess { field: 3, required: valid.len(), actual: valid.len() - 1 },
        ),
        Malformed::TrailingBytes => (
            Place::KIND,
            [valid.as_slice(), &[0]].concat(),
            CoreSchemaError::TrailingBytes { consumed: valid.len(), actual: valid.len() + 1 },
        ),
        Malformed::InvalidEnum => {
            let mut payload = valid.clone();
            assert_eq!(payload[0], Side::Sell as u8, "the enum is the first encoded byte");
            payload[0] = 9;
            (Place::KIND, payload, CoreSchemaError::Enum { field: 1, raw: 9 })
        }
        Malformed::UnknownKind => (7, valid.clone(), CoreSchemaError::UnsupportedKind { kind: 7 }),
    };
    assert_eq!(Place::validate_payload(&valid), Ok(()));
    let loaded = Loaded::configure(b"generated").unwrap();
    let cancel = cancel();
    assert_eq!(loaded.preflight(&[(Cancel::KIND, &cancel), (Place::KIND, &valid)]), (None, 0));
    assert_eq!(
        loaded.preflight(&[(Cancel::KIND, &cancel), (kind, &payload)]),
        (schema_refusal(expected), 1),
        "refused at the failing ordinal before the author's preflight accepts it"
    );
}

#[test_case(b"generated", true; "schema module registry")]
#[test_case(b"hand-built", false; "hand-built registry without validators")]
#[test_case(b"registered", true; "hand-built registry with registered validators")]
fn configuration_requires_a_validator_for_every_registered_command(config: &[u8], accepted: bool) {
    match Loaded::configure(config) {
        Ok(loaded) => {
            assert!(accepted, "a registry without validators must not configure");
            let payload = place();
            assert_eq!(loaded.preflight(&[(Place::KIND, &payload)]), (None, 0));
        }
        Err(status) => {
            assert!(!accepted, "a complete registry must configure: {status:?}");
            assert_eq!(
                status.decode_for(CoreOperation::Configure),
                Ok(Some(CoreError::Configuration(ConfigureError::missing_command_validator(Place::KIND)))),
                "the first command without a validator is refused with the SDK's configure code"
            );
        }
    }
}

/// `schema`'s registry with `Place` replaced, keeping `Cancel`.
fn with_place(place: statevec_model::CommandDefinition) -> SchemaRegistry {
    SchemaRegistry::new(
        schema::SCHEMA_VERSION,
        &[],
        &[place, *Cancel::definition()],
        &[],
        &[*<Side as statevec_model::EnumU8>::DEFINITION],
    )
}

#[test]
fn a_validator_attaches_by_payload_layout_not_display_metadata() {
    // A display tag, as an IDL-decoded registry can carry, leaves the layout.
    let mut fields = Place::definition().fields.to_vec();
    fields[2].semantic = Some(statevec_model::SemanticTag::TimestampMicros);
    let tagged =
        statevec_model::CommandDefinition { fields: Box::leak(fields.into_boxed_slice()), ..*Place::definition() };
    let registry = with_place(tagged).with_command_validator::<Place>();
    let validator = registry.command_payload_validator(Place::KIND).unwrap();
    assert_eq!(validator(&place()), Ok(()));
    assert!(registry.command_payload_validator(Cancel::KIND).is_none());

    // Another layout under Place's kind, or an unregistered kind, is a
    // programmer error, like an invalid static definition.
    let other = statevec_model::CommandDefinition { kind: Place::KIND, ..*Cancel::definition() };
    let mismatch = std::panic::catch_unwind(|| with_place(other).with_command_validator::<Place>());
    assert!(mismatch.is_err(), "a validator must match the registered payload layout");
    let unregistered = SchemaRegistry::new(schema::SCHEMA_VERSION, &[], &[*Cancel::definition()], &[], &[]);
    assert!(std::panic::catch_unwind(|| unregistered.with_command_validator::<Place>()).is_err());
}

#[test]
fn a_validator_must_accept_the_enum_values_the_registry_exports() {
    // The same layout and enum name, but the validator would accept `Hold`
    // while the exported IDL declares only Buy and Sell.
    let wider = std::panic::catch_unwind(|| hand_built().with_command_validator::<wider_schema::Place>());
    assert!(wider.is_err(), "a validator must accept only the enum values the schema exports");
    // The same values in another declaration order decode alike.
    let reordered = hand_built().with_command_validator::<reordered_schema::Place>();
    assert!(reordered.command_payload_validator(Place::KIND).is_some());
    // Each enum field is bound to the enum it decodes, not the first one of
    // its name: here only the second field accepts `Hold`.
    let twin = || {
        let hedge = *twin_schema::Hedge::definition();
        SchemaRegistry::new(
            schema::SCHEMA_VERSION,
            &[],
            &[hedge],
            &[],
            &[*<Side as statevec_model::EnumU8>::DEFINITION],
        )
        .with_command_validator::<twin_schema::Hedge>()
    };
    assert!(std::panic::catch_unwind(twin).is_err(), "every enum field must accept only the exported values");
}
