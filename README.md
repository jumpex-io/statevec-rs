# StateVec

StateVec is a deterministic operational state engine for business-critical
systems.

This repository contains the schema-first Rust API and plugin contract used to
build deterministic domain models around core business state.

It helps a domain team express stateful business logic as:

```text
Command -> Deterministic Execution -> State Delta + Event -> Durable Truth
```

The project is aimed at systems where ordering, atomic state changes, replay,
auditability, and recovery semantics matter more than generic request handling.
Typical examples include accounts, balances, quotas, inventory, positions,
workflows, settlement state, game entities, and other event-driven business
domains with explicit state transitions.

StateVec is not a web framework and it is not a general-purpose database. It is
the domain model and runtime contract layer for the part of a system that needs
controlled, deterministic state transitions.

## What Is In This Repository

This repository currently contains the open-source Rust crates for defining and
running StateVec domains:

- `statevec`: facade crate that re-exports the stable public API.
- `statevec-model`: core schema, record, command, event, IDL, and registry
  types.
- `statevec-macros`: derive and attribute macros for schema-first domain
  definitions.
- `statevec-api`: typed runtime host APIs and the stable runtime plugin ABI.
- `demo/bank`: a small bank domain plugin with deposit, withdraw, and transfer
  commands. See [`demo/bank/deploy-eval/README.md`](demo/bank/deploy-eval/README.md)
  for the Docker-based single-host evaluation setup.

The current scope is the domain model, generated accessors, runtime plugin
contract, and tests. Single-node runtime binaries, operational tooling, and HA
queue deployment modes are part of the broader StateVec direction, but they are
not presented here as completed runtime products.

`STATEVEC_API_VERSION` is the logical API compatibility string exposed by
`statevec-api`. The runtime plugin ABI version is a numeric value
(`RUNTIME_PLUGIN_ABI_VERSION_V1 = 1`); both are currently `1`, but they are
separate compatibility markers.

## Core Concepts

StateVec domains are built from three primary schema objects.

**Command** describes an external business action, such as `Deposit`,
`Withdraw`, or `Transfer`.

**Record** describes committed business state. Records have stable kinds,
schema versions, generated typed accessors, optional primary keys, and a fixed
layout suitable for deterministic execution.

**Event** describes a committed business fact produced by a transaction. Events
are part of the execution result rather than ad hoc side effects.

A command execution should produce a deterministic result:

```text
TxResult = State Delta + Events + Outcome
```

Business rejects, such as insufficient balance or invalid order state, should be
treated as deterministic outcomes. They are separate from system failures such
as I/O errors, corrupted logs, or closed worker channels.

## Quick Example

Most users should depend on the facade crate and import the prelude:

```rust
use statevec::prelude::*;
```

A minimal schema module looks like this:

```rust
use statevec::prelude::*;

#[schema_module(version = "1.0")]
pub mod v1_0 {
    use super::*;

    #[record(kind = 1, record_len = 64, pk(fields = [account_id]))]
    pub struct Account {
        #[field(index = 1, immutable = true)]
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
```

The macros generate:

- schema registry metadata;
- stable schema fingerprints;
- IDL JSON;
- record builders and typed accessors;
- command and event builders/accessors;
- primary key helpers;
- runtime-facing schema information.

For domain modeling rules and plugin authoring guidance, see
[`docs/designing-statevec-plugins.md`](docs/designing-statevec-plugins.md).

## Runtime Plugin Shape

A domain can implement `RuntimePlugin` and execute commands through the typed
host context APIs. The bank demo shows the intended pattern, including
`command_dispatch!`, typed primary-key updates, event builders, and invariant
validation in [`demo/bank/src/lib.rs`](demo/bank/src/lib.rs).

The runtime host owns durability, replay, publication, and process boundaries.
The plugin owns deterministic business logic and schema-defined state access.

## Bank Demo

The `demo/bank` plugin can be run locally against the single-host evaluation
runtime with Docker-based Redpanda dependencies. The deploy guide covers runtime
binary paths, plugin preparation, startup, status checks, replay, and cleanup:

[`demo/bank/deploy-eval/README.md`](demo/bank/deploy-eval/README.md)

Runtime binaries are distributed separately and should be downloaded from
[`jumpex-io/statevec-runtime`](https://github.com/jumpex-io/statevec-runtime).

## Repository Layout

```text
.
|-- crates/
|   |-- statevec/
|   |-- statevec-api/
|   |-- statevec-macros/
|   `-- statevec-model/
|-- demo/
|   `-- bank/
|       `-- deploy-eval/
|-- Cargo.toml
`-- LICENSE
```

## Development

This workspace uses Rust 2024 and currently declares `rust-version = "1.91"` in
each package.

Run the full test suite:

```bash
cargo test
```

Run tests for a specific crate:

```bash
cargo test -p statevec-model
cargo test -p statevec-macros
cargo test -p statevec-api
```

The macro crate uses `trybuild` for compile-fail tests. If an intentional macro
diagnostic change updates expected spans or messages, refresh snapshots with:

```bash
TRYBUILD=overwrite cargo test -p statevec-macros --test test_macros compile_fail
```

## Design Principles

StateVec is designed around a few constraints:

- Schema comes first. Domain state, commands, and events should have stable
  explicit definitions.
- Execution should be deterministic. Business logic should avoid uncontrolled
  I/O, random numbers, physical time, and other non-replayable effects inside a
  transaction.
- Durable truth is explicit. A transaction is not committed until it crosses the
  runtime's durable boundary.
- Events are facts. They should be derived from committed execution rather than
  emitted as uncontrolled side effects.
- Runtime and domain code have separate responsibilities. Domain plugins should
  depend on stable host capabilities, not engine internals.

## Roadmap Context

The broader StateVec architecture includes:

- single-node deterministic runtime;
- local txlog, checkpoint, replay, and event export;
- Kafka/Redpanda command ingestion and event publication;
- operation status surfaces;
- optional HA queue mode with replication, role transitions, and read-only
  materialization.

Those runtime modes share the same domain model and execution semantics, but
their operational guarantees are different. This repository is currently focused
on the reusable Rust API and domain definition layer.

## License

StateVec is licensed under the Apache License, Version 2.0. See
[`LICENSE`](LICENSE).
