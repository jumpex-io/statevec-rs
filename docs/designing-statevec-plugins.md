# Designing StateVec Plugins

StateVec is a deterministic operational state engine for business-critical
systems. A StateVec plugin describes the business state model and the
deterministic transaction logic that mutates that state.

This guide is written for domain teams and AI coding tools that help generate
or review StateVec plugins. It focuses on the current public `statevec-rs`
schema and plugin API.

## What This Repository Does Not Provide

This repository is the Rust domain API and plugin contract. It does not provide
the serving runtime binary, queue/network integration, local persistence
engine, replay executable, or replication/failover implementation.

Runtime binaries and operator-facing runtime documentation are published
separately through
[`jumpex-io/statevec-runtime`](https://github.com/jumpex-io/statevec-runtime).

## Mental Model

A plugin owns business meaning. The runtime owns operational authority.

The plugin defines:

- records: durable business state;
- commands: requested business actions;
- events: facts produced by committed execution;
- transaction logic: deterministic state transitions;
- schema identity: the schema registry that matches the plugin behavior.

The runtime owns:

- state storage;
- primary-key lookup;
- transaction ordering;
- tx-log durability;
- replay and recovery;
- operation status;
- command ingress and event/result publication.

The core shape is:

```text
Command -> deterministic plugin execution -> State Delta + Events -> Durable Truth
```

Business rejects, such as insufficient balance, invalid order state, or
duplicate business operation, should be deterministic outcomes. Unavailable
files, broken plugin loading, corrupt logs, and host I/O failures are runtime
failures.

## Plugin Boundary

A plugin implements `RuntimePlugin` and is exported with:

```rust
statevec::export_runtime_plugin!(Box::new(MyRuntimeFactory));
```

The runtime loads one plugin instance for one engine process. The plugin should
not own persistence, queue clients, networking, replay, or checkpointing. It
should only execute domain logic through the host-provided transaction context.

The host/plugin ABI entry symbol is:

```text
statevec_runtime_plugin_entry_v1
```

The host validates plugin loading and schema identity before serving. A version
or schema mismatch should fail startup rather than execute against the wrong
state layout.

## Schema Module

Group one compatible schema version under `#[schema_module]`:

```rust
use statevec::prelude::*;

#[schema_module(version = "1.0")]
pub mod v1_0 {
    use super::*;

    // records, commands, events, and EnumU8 types go here
}
```

The schema module generates helpers such as:

- `registry()`;
- `schema_identity()`;
- `SCHEMA_VERSION`;
- static record, command, event, and enum definitions.

Field indexes inside each record, command, and event must be contiguous and
start at `1`.

The macros also add common derives such as `Clone`, `Debug`, `PartialEq`, and
`Eq` to generated record, command, and event structs. Domain code should not
depend on handwritten derives for those traits.

## Records

Records are durable business state. They should represent facts the engine must
recover exactly after restart or replay.

Use records for:

- balances;
- positions;
- account or order state;
- inventory quantities;
- quotas;
- workflow state;
- durable dedupe or sequence state.

Avoid records for:

- request DTOs;
- logs;
- temporary calculations;
- derived views that can be rebuilt from committed state;
- non-deterministic external data.

Example:

```rust
#[record(kind = 1, record_len = 64, pk(fields = [account_id]))]
pub struct Account {
    #[field(index = 1, immutable)]
    pub account_id: u64,

    #[field(index = 2)]
    pub total_credit: u64,

    #[field(index = 3)]
    pub total_debit: u64,
}
```

### Record Kind

`kind` is a stable `u8` identifier for the record type. Valid public values are
`1..=255`; `0` is reserved.

Do not renumber record kinds after state has been written. Treat kind changes
as schema migration events.

### Record Length

`record_len` is the total fixed record size, including the runtime record
header. It must be:

```text
64 * N, where N is a power of two
```

Valid examples:

```text
64, 128, 256, 512, 1024
```

The runtime header is currently `16` bytes. The generated record data area is:

```text
record_data_len = record_len - 16
```

The sum of encoded field sizes must fit inside `record_data_len`.

### Record Field Types

Current record fields support:

| Rust type | Encoded size | PK support |
|---|---:|---|
| `bool` | 1 byte | yes |
| `u8` | 1 byte | yes |
| `u16` | 2 bytes | yes |
| `u32` | 4 bytes | yes |
| `u64` | 8 bytes | yes |
| `i32` | 4 bytes | yes |
| `i64` | 8 bytes | yes |
| `u128` | 16 bytes | no |
| `FixedBytes<N>` | `2 + N` bytes | yes |
| enum with `#[field(enum_u8)]` | 1 byte | yes |

`VarBytes` is not supported in records. Use `VarBytes` only in commands and
events.

`FixedBytes<N>` stores a logical length plus padded fixed storage. For primary
keys, the padded bytes are used so the key has stable width.

### Immutable Fields

Use `#[field(immutable)]` for fields that must be initialized when a record is
created and then never changed by update builders.

Primary-key fields must be immutable.

```rust
#[field(index = 1, immutable)]
pub account_id: u64,
```

Generated creation builders expose `init_<field>()` for immutable fields.
Generated update builders do not expose setters for immutable fields.

### Reserved Fields

Use `#[field(index = N, reserved)]` when preserving layout space for future
schema evolution.

Reserved fields:

- contribute to byte layout;
- are excluded from active field definitions;
- cannot be primary-key fields;
- cannot be immutable;
- cannot use `enum_u8`.

## Primary Keys

Primary keys are host-owned lookup indexes over record state. They let plugin
logic address records by business identity instead of by `sys_id`.

Example:

```rust
#[record(kind = 1, record_len = 64, pk(fields = [account_id]))]
pub struct Account {
    #[field(index = 1, immutable)]
    pub account_id: u64,
}
```

Generated code provides:

```rust
Account::pk(account_id)
```

Plugin code can then use:

```rust
tx.update_typed_by_pk::<Account, _, _, _>(Account::pk(account_id), |account| {
    account.set_total_credit(account.total_credit() + amount);
})
```

PK semantics:

- a PK identifies one logical business record for one record kind;
- PK fields must be immutable;
- PK bytes are derived from current record bytes at create/update boundaries;
- the runtime owns the actual index and lookup behavior;
- the plugin should treat PK as an identity, not as a storage address.

Current PK field types are:

```text
bool, u8, u16, u32, u64, i32, i64, FixedBytes<N>, enum_u8
```

`u128` and `VarBytes` are not supported in PK v1.

## Commands

Commands are requested business actions. They are input to deterministic
execution.

Use commands for:

- `Deposit`;
- `Withdraw`;
- `Transfer`;
- `PlaceOrder`;
- `CancelOrder`;
- `ReserveInventory`;
- `AdvanceWorkflow`.

Commands should carry enough data to decide the transaction deterministically.
They should not depend on wall-clock time, random numbers, network calls, local
files, or mutable global state inside `run_tx`.

Example:

```rust
#[command(kind = 1)]
pub struct Deposit {
    #[field(index = 1)]
    pub account_id: u64,

    #[field(index = 2)]
    pub amount: u64,
}
```

`kind` is a stable `u8` identifier. Valid values are `1..=255`.

## Events

Events are facts produced by committed execution. They are not side effects.
They should describe what became true because a command executed.

Use events for:

- balance changed;
- order accepted;
- order cancelled;
- inventory reserved;
- workflow advanced.

Avoid events for:

- debug logs;
- commands copied back out;
- speculative facts that did not commit;
- external delivery state.

Example:

```rust
#[event(kind = 1)]
pub struct BalanceChanged {
    #[field(index = 1)]
    pub account_id: u64,

    #[field(index = 2)]
    pub new_credit: u64,

    #[field(index = 3)]
    pub new_debit: u64,
}
```

Events are emitted through the transaction context:

```rust
tx.emit_typed_event::<BalanceChanged>(
    BalanceChanged::builder()
        .set_account_id(account_id)
        .set_new_credit(new_credit)
        .set_new_debit(new_debit)
        .build(),
);
```

The runtime records events as part of the deterministic transaction result.
Publication, replay, and recovery derive from committed runtime history.

## Command And Event Field Types

Commands and events support:

| Rust type | Encoded size |
|---|---:|
| `bool` | 1 byte |
| `u8` | 1 byte |
| `u16` | 2 bytes |
| `u32` | 4 bytes |
| `u64` | 8 bytes |
| `i32` | 4 bytes |
| `i64` | 8 bytes |
| `u128` | 16 bytes |
| `FixedBytes<N>` | `2 + N` bytes |
| `VarBytes` | `2 + len` bytes |
| enum with `#[field(enum_u8)]` | 1 byte |

`VarBytes` is represented as `Vec<u8>` in the generated command/event struct
and as `&[u8]` in generated accessors.

Do not use command/event payloads as a general document store. Prefer stable
explicit fields. Use `VarBytes` only when the business payload is truly
opaque or variable length.

## Enum Fields

Use `#[derive(EnumU8)]` with `#[repr(u8)]` for compact enums:

```rust
#[derive(EnumU8)]
#[repr(u8)]
pub enum OrderSide {
    Buy = 1,
    Sell = 2,
}
```

Use `#[field(enum_u8)]` on fields that store the enum:

```rust
#[field(index = 2, enum_u8)]
pub side: OrderSide,
```

Enum discriminants must fit in `u8`. Treat discriminant values as durable
schema values once state or history has been written.

## Deterministic Transaction Logic

Plugin transaction logic should be deterministic for the same starting state
and command payload.

Allowed inside transaction logic:

- reading and updating StateVec records through the host context;
- creating records;
- deleting records;
- emitting events;
- pure calculations;
- deterministic validation.

Avoid inside transaction logic:

- wall-clock time;
- randomness;
- network calls;
- file I/O;
- background tasks;
- reading process-global mutable state;
- non-deterministic iteration over unordered maps.

If a command cannot be applied because of business rules, return a deterministic
plugin error. The runtime can record that as a rejected outcome according to
the runtime mode.

## External Input Conversion

StateVec command payloads are the canonical execution boundary. Business teams
may still expose JSON, Protobuf, REST, GraphQL, or other external request
formats, but those should be converted before entering the StateVec command
path.

Conversion is responsible for:

- parsing external request bytes;
- validating user-facing input shape;
- mapping it to a known `command_kind`;
- building canonical StateVec command payload bytes.

Conversion is not responsible for:

- assigning runtime transaction sequence;
- mutating StateVec state;
- replay correctness;
- durable outcome publication.

Producer-side conversion failure happens before queue acceptance. It has no
runtime transaction and no deterministic StateVec outcome.

Accepted command execution failure is different: once a command is in the
runtime execution path, business rejection should be recorded as a deterministic
outcome rather than silently dropped.

## AI Tooling Checklist

When generating or reviewing a plugin, check:

- every record/command/event kind is stable and in `1..=255`;
- field indexes are contiguous from `1`;
- record lengths are `64 * power_of_two`;
- record fields fit in `record_len - 16`;
- record PK fields are immutable;
- PK field types are supported;
- no `VarBytes` appears in records;
- commands describe requested actions, not durable state;
- events describe committed facts, not debug messages;
- transaction logic avoids time, randomness, I/O, and network calls;
- plugin schema registry matches the runtime behavior;
- business rejects are deterministic.

## Minimal Plugin Shape

```rust
use statevec::prelude::*;

#[schema_module(version = "1.0")]
pub mod v1_0 {
    use super::*;

    #[record(kind = 1, record_len = 64, pk(fields = [account_id]))]
    pub struct Account {
        #[field(index = 1, immutable)]
        pub account_id: u64,
        #[field(index = 2)]
        pub total_credit: u64,
        #[field(index = 3)]
        pub total_debit: u64,
    }

    #[command(kind = 1)]
    pub struct Deposit {
        #[field(index = 1)]
        pub account_id: u64,
        #[field(index = 2)]
        pub amount: u64,
    }

    #[event(kind = 1)]
    pub struct BalanceChanged {
        #[field(index = 1)]
        pub account_id: u64,
        #[field(index = 2)]
        pub new_credit: u64,
        #[field(index = 3)]
        pub new_debit: u64,
    }
}

pub struct MyRuntime;

impl RuntimePlugin for MyRuntime {
    fn name(&self) -> &'static str {
        "my-runtime"
    }

    fn schema_registry(&self) -> SchemaRegistry {
        v1_0::registry()
    }

    fn run_tx(
        &self,
        tx: &mut dyn RuntimeHostContext,
        command: &dyn RuntimeCommandEnvelope,
    ) -> Result<(), RuntimePluginError> {
        match command.command_kind() {
            v1_0::Deposit::KIND => {
                let deposit = v1_0::Deposit::wrap(command.payload());
                let account_id = deposit.account_id();
                let amount = deposit.amount();

                tx.update_or_create_typed_by_pk::<v1_0::Account, _, _, _, _>(
                    v1_0::Account::pk(account_id),
                    |account| {
                        let next = account.total_credit() + amount;
                        account.set_total_credit(next);
                        next
                    },
                    |account| {
                        account.init_account_id(account_id);
                        account.set_total_credit(amount);
                        account.set_total_debit(0);
                        amount
                    },
                )
                .map_err(|e| RuntimePluginError::new(e.to_string()))?;

                Ok(())
            }
            other => Err(RuntimePluginError::new(format!(
                "unknown command kind {other}"
            ))),
        }
    }
}
```
