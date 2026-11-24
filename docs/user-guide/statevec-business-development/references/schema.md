# Defining a Schema

A schema module declares every record, command, event and enum of one
business runtime. The macros generate typed accessors, builders, key helpers
and a `SchemaRegistry`. The registry's identity contributes to the initial
and subsequent Core versions.

## Module layout

```rust
use statevec::{command, event, record, schema_module, EnumU8};

#[schema_module(version = "1.0")]
pub mod v1_0 {
    use super::*;
    use statevec::record::Decimal;

    #[derive(EnumU8)]
    #[repr(u8)]
    pub enum OrderStatus {
        Open = 1,
        Filled = 2,
        Cancelled = 3,
    }

    #[record(kind = 1, record_len = 64, unique_key(id = 0, fields = [account_id]))]
    pub struct Account {
        #[field(index = 1, immutable)]
        pub account_id: u64,
        #[field(index = 2)]
        pub balance: Decimal<6>,
    }

    #[command(kind = 1)]
    pub struct Deposit {
        #[field(index = 1)]
        pub account_id: u64,
        #[field(index = 2)]
        pub amount: Decimal<6>,
    }

    #[event(kind = 1, inline_response = true)]
    pub struct BalanceChanged {
        #[field(index = 1)]
        pub account_id: u64,
        #[field(index = 2)]
        pub balance: Decimal<6>,
    }
}

pub use v1_0::*;
```

Declare the facade dependency under its crate name, `statevec`, so generated
code can resolve the SDK in any module.

The module generates `registry()`, `schema_identity()` and `idl_json()`.
The schema version, names, kinds, layouts, keys and enum discriminants feed
the portable schema identity. IDL also carries field semantics and event reply
policy. Their compatibility and revision requirements are described below.

## Field types

| Rust type | Encoded bytes | Record | Command / event | UK / index key |
| --- | --- | --- | --- | --- |
| `bool`, `u8` | 1 | yes | yes | yes |
| `u16` | 2 | yes | yes | yes |
| `u32`, `i32` | 4 | yes | yes | yes |
| `u64`, `i64` | 8 | yes | yes | yes |
| `u128` | 16 | yes | yes | no |
| `Decimal<S>` (`S <= 38`) | 16 (i128 mantissa) | yes | yes | yes |
| `FixedBytes<N>` | 2 + N | yes | yes | yes |
| enum with `#[field(enum_u8)]` | 1 | yes | yes | yes |
| `VarBytes` | variable | no | yes | no |

There are no floating-point or string types. Use `Decimal<S>` for amounts,
`FixedBytes<N>` for short identifiers and text, and an `EnumU8` enum for closed
state sets. Optional semantic tags (`#[field(semantic = "...")]`, for example
`text`, `timestampMicros`, `uuid`) are consumer metadata; they change neither
bytes nor schema identity. InPlace upgrades compare record-field tags and
Additive Projectors compare all of them, so keep an existing field's tag.

## Records

```rust
#[record(
    kind = 11,
    record_len = 128,
    unique_key(id = 0, name = "by_account_client", fields = [account_id, client_id]),
    unique_key(id = 1, name = "by_external_id", fields = [external_id]),
    index(id = 0, name = "by_account_status", fields = [account_id, status])
)]
pub struct Order { /* fields */ }
```

Do not leave a trailing comma after the last attribute argument; the schema
parser rejects it.

- **`kind`** is in `1..=61439`. Kind 0 and `61440..` are reserved. Kinds are
  unique per registry.
- **`record_len`** is a multiple of 16 from 32 to 32768 bytes, including the
  24-byte header: 32, 48, 64, 80, 96, and so on. The 24-byte header plus all
  fields must fit, or the macro reports the computed layout. Each record
  occupies exactly `record_len` bytes, so a record with 56 bytes of fields can
  use 80 rather than 128. Fields are packed in index order and read by byte
  copy; `record_len` aligns the slot, not individual fields. An existing kind
  keeps its `record_len` across InPlace upgrades.
- **Field indexes** start at 1 and are contiguous. Use one `#[field(...)]` per
  field. Generics, tuple structs and `#[repr]` are not allowed.
- **`immutable`** fields are written once, at creation, through `init_<field>`.
  Mutable fields use `set_<field>`.
- **Retired fields** keep their name, index, offset and length. Use
  `#[field(index = N, reserved)]` on the existing field or retain its reserved
  slot in the exported schema. Remove any key or index referencing it.
  Historical bytes can be nonzero. New fields use checked-zero gaps or a
  zero tail. See the change policy in
  [integration-and-testing.md](integration-and-testing.md#schema-and-behavior-changes).

### Unique keys

- At most 3 per record, with ids contiguous from 0. Each needs `id = N`; with
  more than one, each also needs `name = "..."`.
- Use at least one supported active field in each key. The host builds its key
  encoder from exported IDL for loaded Cores. The IDL carries field definitions
  and key order, while Rust function pointers stay in the native registry.
- Every UK field must be `immutable`. A key cannot change after creation.
- Decimal keys encode the fixed-scale i128 mantissa as 16 big-endian bytes
  with its sign bit flipped. Byte order matches signed value order within that
  field's schema scale; the scale is preserved in IDL and schema identity.
  Generated helpers and IDL-imported generic encoders produce the same bytes.
- Generated helpers: `Order::unique_key(..)` encodes UK 0, `Order::unique_key_<name>(..)`
  encodes a named unique key, and `Order::<NAME>_UNIQUE_KEY_ID` is its id. For
  `name = "by_external_id"`, use `Order::unique_key_by_external_id(..)`.
- A UK value identifies at most one live record of that kind. Choose fields the
  business already treats as an identity, such as an account number or an
  external order id.

### Canonical indexes

- At most 2 per record, ids contiguous from 0, each with `name` and 1 to 3
  fields. The encoded key must be at most 64 bytes.
- Indexes may use mutable fields. `u128` and reserved fields cannot
  be indexed.
- Generated helpers: `Order::index_<name>(..)`, prefix helpers
  `index_<name>_prefix1(..)` and `_prefix2(..)`, and `Order::<NAME>_INDEX_ID`.
  For `name = "by_account_status"`, use `Order::index_by_account_status(..)`.
- Inside a transaction, indexes support bounded prefix counts
  (`count_index_prefix_capped`). They are not a general query API.

### Generated record API

| Item | Use |
| --- | --- |
| `Order::KIND`, `Order::definition()` | kind constant and static definition |
| `OrderAccess` | read view: `OrderAccess::new(&bytes)`, getters `order.amount()` |
| `OrderAccessMut`, `NewOrderBuilder`, `UpdateOrderBuilder` | write views used by the transaction API closures |
| `init_<field>` / `set_<field>` | write an immutable field at creation / a mutable field; `FixedBytes` values are passed by reference |

You normally touch the write views only inside the transaction closures
described in [business-logic.md](business-logic.md).

## Commands and events

```rust
#[command(kind = 4)]
pub struct BatchTransfer {
    #[field(index = 1)]
    pub from_account_id: u64,
    #[field(index = 2)]
    pub to_account_ids: VarBytes,
    #[field(index = 3)]
    pub amount: Decimal<6>,
}
```

- **Kinds** are in `1..=61439`, unique per message family. Named-field structs
  only. Field indexes follow the same contiguous rule as records. `immutable`
  does not apply to payloads.
- **`VarBytes`** carries variable data such as a list of ids or a memo. Bound
  and parse its application-defined contents in your handler. Schema
  validation covers the byte field's encoded framing.
- **Payload validation** checks the complete encoded layout, boolean and enum
  values, `FixedBytes` lengths and zero padding, and rejects trailing bytes.
  The generated dispatch preflight and exported Core schema check both cover
  fields the handler ignores. A `schema_module!` registry carries the
  validators automatically.
- **`inline_response = true`** marks an event that may be returned to the client
  in its bounded response. Use it for small facts the caller needs, not for
  large payloads. Schema identity excludes this flag; changing it requires a
  new execution revision.
- **Generated items:** `DepositAccess<'_>` with one getter per field (passed to
  your handler), `Deposit::builder()` with `set_<field>(value)` (by value,
  including `FixedBytes`) and `build() -> Result<Vec<u8>, PayloadBuildError>`.
  `builder()` needs `statevec::GeneratedCommandAccess` or
  `statevec::GeneratedEventAccess` in scope; `Deposit::KIND` needs
  `statevec::CommandSchema` (`EventSchema` for events, `RecordSchema` for
  records).

## Enums

`#[derive(EnumU8)]` requires `#[repr(u8)]` and an explicit discriminant on every
variant. Discriminants are stored bytes: never renumber or reuse one. Keep an
existing enum's variant set unchanged across upgrades: InPlace upgrades refuse
a change to an enum used by a record field, and Additive Projectors refuse any
change. In a field, mark the enum with `#[field(enum_u8)]`.

## Design checklist

- **Identity:** choose an immutable natural key as UK 0 when the handler looks
  up the record by its business identity.
- **Sizing:** fixed-width fields cover the real business range, including
  `Decimal` scale and `FixedBytes` length, with headroom. `record_len` leaves
  unused zero tail for planned fields. Keep retired slots opaque.
- **Command completeness:** a command carries every input its handler decides
  on (amounts, ids, client-supplied times, idempotency references). Nothing
  comes from the environment.
- **Events:** publish only events from a successful transaction. The host
  discards provisional events if a handler or candidate rule refuses it.
- **Stable numbering:** kinds, field indexes, UK/index ids and enum
  discriminants are final for the life of the cluster.
