# StateVec

StateVec runs deterministic business logic over schema-defined records. You
define commands, data schemas and handlers; the production cluster replicates
commands and checks their execution.

This repository provides the open-source Rust development libraries, user
guide, examples and a small in-memory engine for local business tests.

## Start here

Read the [user guide](docs/user-guide/README.md), then run the examples:

```sh
cargo nextest run -p bank -p flight-booking
```

The Bank example covers deposits, withdrawals, transfers, events and refusal.
The flight-booking example covers inventory and reservations.

## Libraries

| Crate | Purpose |
| --- | --- |
| `statevec` | Common imports for application code |
| `statevec-model` | Records, commands, events, schemas, keys and schema identity |
| `statevec-macros` | Schema declarations, accessors, builders and checked payloads |
| `statevec-api` | Business transaction interfaces and the plugin ABI |
| `statevec-test` | A small memory engine and helpers for local tests |
| `statevec-ingress-client` | Bounded cluster client with manual, background and async driving |
| `ingress-api` | Request/reply codecs and incremental frame readers |
| `statevec-frame` | Client inputs, intent identity and application-result bindings |
| `statevec-projector-api` | Complete committed events, record changes and projector checkpoint interfaces |

Application crates usually depend on `statevec`, with `statevec-test` as a
development dependency. Use the same StateVec release or Git revision for all
these crates. Cluster client usage is covered in the
[client guide](docs/user-guide/cluster-client.md).
For application read models, see the [projector guide](docs/user-guide/projectors.md).

```rust
use statevec::prelude::*;

#[schema_module(version = "1.0")]
pub mod ledger {
    use super::*;

    #[record(kind = 1, record_len = 64, uk(id = 0, fields = [account_id]))]
    pub struct Account {
        #[field(index = 1, immutable)]
        pub account_id: u64,
        #[field(index = 2)]
        pub balance: u64,
    }

    #[command(kind = 1)]
    pub struct Deposit {
        #[field(index = 1)]
        pub account_id: u64,
        #[field(index = 2)]
        pub amount: u64,
    }
}
```

The current schema uses 16-bit kinds, immutable unique keys, optional ordered
indexes, checked command payloads and fixed-point decimal fields. Payload
builders return `Result`. Older examples using `pk(...)` and unchecked
builders need to be updated.

## Local execution and production

The test engine stores records and keys in Rust collections and captures
events in memory. It can execute typed handlers, discard a failed transaction
and check the resulting records and events. Production uses its own engine,
cluster binding and persistence.

The two engines share the schema and business transaction interfaces. The
local engine helps develop business logic; cluster durability, recovery,
replication and performance are verified by the production platform.

Production deployment uses the platform's node binaries and integration
workflow. This repository currently ships Rust development libraries and local
tests. Single-host serving and replay executables are outside its delivery
scope.

The public API retains plugin ABI version 2 for integration work. Its presence
does not establish that dynamic plugin loading or online upgrades are available
in the production cluster.

## Development

The workspace uses Rust 2024 with a minimum declared Rust version of 1.91.
Development and CI use Rust 1.95.0, pinned in `rust-toolchain.toml`, to keep
compiler diagnostics reproducible for the compile-fail tests.

```sh
cargo check --workspace --all-targets
cargo nextest run --workspace --all-targets
cargo test --workspace --doc
```

The macro suite includes compile-fail cases through `trybuild`.

## License

Apache-2.0. See [LICENSE](LICENSE).
