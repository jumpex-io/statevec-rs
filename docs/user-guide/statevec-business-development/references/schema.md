# Defining a Schema

A schema module declares every record, command, event and enum of one
business runtime. The macros generate typed accessors, builders, key helpers
and a `SchemaRegistry` whose identity is bound into the cluster at genesis.

## Module layout

```rust
// Generated code names these crates; keep them in scope where the macros expand.
#[allow(unused_imports)]
use statevec::{statevec_api, statevec_model};
use statevec::{command, event, record, schema_module, EnumU8};

#[schema_module(version = "1.0")]
pub mod v1_0 {
    use super::*;
    use statevec::record::Decimal;
    use statevec::FixedBytes;

    #[derive(EnumU8)]
    #[repr(u8)]
    pub enum OrderStatus {
        Open = 1,
        Filled = 2,
        Cancelled = 3,
    }

    #[record(kind = 1, record_len = 64, uk(id = 0, fields = [account_id]))]
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

If the schema lives in a nested module, start it with `use super::*;` so the
crate-level `statevec_api`/`statevec_model` imports are visible; the same
applies to the module that invokes `command_dispatch!`.

The module generates `registry()`, `schema_identity()` and `idl_json()`. The
`version` string, names, kinds, field order, types, keys and enum discriminants
all feed the schema identity. Rust module paths do not.

## Field types

| Rust type | Encoded bytes | Record | Command / event | UK / index key |
| --- | --- | --- | --- | --- |
| `bool`, `u8` | 1 | yes | yes | yes |
| `u16` | 2 | yes | yes | yes |
| `u32`, `i32` | 4 | yes | yes | yes |
| `u64`, `i64` | 8 | yes | yes | yes |
| `u128` | 16 | yes | yes | no |
| `Decimal<S>` (`S <= 38`) | 16 (i128 mantissa) | yes | yes | no |
| `FixedBytes<N>` | 2 + N | yes | yes | yes |
| enum with `#[field(enum_u8)]` | 1 | yes | yes | yes |
| `VarBytes` | variable | no | yes | no |

There are no floating-point or string types. Use `Decimal<S>` for amounts,
`FixedBytes<N>` for short identifiers and text, and an `EnumU8` enum for closed
state sets. Optional semantic tags (`#[field(semantic = "...")]`, for example
`text`, `timestampMicros`, `uuid`) are consumer metadata only; they change
neither bytes nor schema identity.

## Records

```rust
#[record(
    kind = 11,
    record_len = 128,
    uk(id = 0, name = "by_account_client", fields = [account_id, client_id]),
    uk(id = 1, name = "by_external_id", fields = [external_id]),
    index(id = 0, name = "by_account_status", fields = [account_id, status])
)]
pub struct Order { /* fields */ }
```

Do not leave a trailing comma after the last attribute argument; the schema
parser rejects it.

- **`kind`** is in `1..=61439`. Kind 0 and `61440..` are reserved. Kinds are
  unique per registry.
- **`record_len`** is a power-of-two multiple of 64: 64, 128, 256, and so on,
  up to 32768 bytes including the 24-byte header. The 24-byte header plus all
  fields must fit, or the macro reports the computed layout.
- **Field indexes** start at 1 and are contiguous. Use one `#[field(...)]` per
  field. Generics, tuple structs and `#[repr]` are not allowed.
- **`immutable`** fields are written once, at creation, through `init_<field>`.
  Mutable fields use `set_<field>`.
- **Reserved space**: `#[field(index = N, reserved)] pub _reserved: u32` keeps
  bytes allocated and zero. Reserved fields cannot be keys. Reserving space does
  not make a later layout change free; see the change policy in
  [integration-and-testing.md](integration-and-testing.md#schema-and-behavior-changes).

### Unique keys

- At most 3 per record, with ids contiguous from 0. Each needs `id = N`; with
  more than one, each also needs `name = "..."`.
- Every UK field must be `immutable`. A key cannot change after creation.
- Generated helpers: `Order::uk(..)` encodes UK 0, `Order::uk_<name>(..)`
  encodes a named UK, and `Order::<NAME>_UK_ID` is its id. For
  `name = "by_external_id"`, use `Order::uk_by_external_id(..)`.
- A UK value identifies at most one live record of that kind. Choose fields the
  business already treats as an identity, such as an account number or an
  external order id.

### Canonical indexes

- At most 2 per record, ids contiguous from 0, each with `name` and 1 to 3
  fields. The encoded key must be at most 64 bytes.
- Indexes may use mutable fields. `u128`, `Decimal` and reserved fields cannot
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
- **`VarBytes`** carries variable data such as a list of ids or a memo. Your
  handler must parse and bound it itself and reject malformed content with a
  business code.
- **Payload validation** checks the complete encoded layout, boolean and enum
  values, `FixedBytes` lengths and zero padding, and rejects trailing bytes.
  Validate before reading through generated accessors. The generated dispatch
  preflight checks every command field, including fields the handler ignores.
- **`inline_response = true`** marks an event that may be returned to the client
  in its bounded response. Use it for small facts the caller needs, not for
  large payloads.
- **Generated items:** `DepositAccess<'_>` with one getter per field (passed to
  your handler), `Deposit::builder()` with `set_<field>(value)` (by value,
  including `FixedBytes`) and `build() -> Result<Vec<u8>, PayloadBuildError>`.
  `builder()` needs `statevec::GeneratedCommandAccess` or
  `statevec::GeneratedEventAccess` in scope; `Deposit::KIND` needs
  `statevec::CommandSchema` (`EventSchema` for events, `RecordSchema` for
  records).

## Enums

`#[derive(EnumU8)]` requires `#[repr(u8)]` and an explicit discriminant on every
variant. Discriminants are stored bytes: never renumber or reuse one. In a
field, mark the enum with `#[field(enum_u8)]`.

## Design checklist

- **Identity:** each record has one immutable natural key (UK 0), and no handler
  needs to change it.
- **Sizing:** fixed-width fields cover the real business range, including
  `Decimal` scale and `FixedBytes` length, with headroom. `record_len` leaves
  room for planned fields, and any reserved span is zero-filled.
- **Command completeness:** a command carries every input its handler decides
  on (amounts, ids, client-supplied times, idempotency references). Nothing
  comes from the environment.
- **Events:** each event describes a committed fact and is emitted only on
  success. A rejected command emits nothing.
- **Stable numbering:** kinds, field indexes, UK/index ids and enum
  discriminants are final for the life of the cluster.
