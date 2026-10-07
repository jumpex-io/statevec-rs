# Integration, Testing and Change Policy

## Dependencies

Application code depends on `statevec`. Local tests add `statevec-test`:

```toml
[dependencies]
statevec = { git = "https://github.com/jumpex-io/statevec-rs", rev = "<release-commit>" }

[dev-dependencies]
statevec-test = { git = "https://github.com/jumpex-io/statevec-rs", rev = "<release-commit>" }
```

Replace the revision with one supported by your platform release. Keep the two
dependencies at the same revision. Inside this workspace, use the existing
workspace dependencies.

## Local execution

The local engine uses ordinary Rust collections. It supports typed access,
multiple unique keys, index-prefix counts and captured events. A transaction
sees its own writes; an error discards that transaction's records, key changes,
allocated ids and events.

Use `TestHost::transaction` to preserve your application's typed errors.
`TestHost::for_plugin` is a convenience wrapper for the public plugin callback;
that callback returns `RuntimePluginError`, so it does not preserve your
application error enum.

The following test uses the [Bank example](../../../../demo/bank/src/lib.rs):

```rust
use bank::{Account, BankError, BankRuntime, Deposit, Withdraw, registry};
use statevec::{Command, CommandSchema, GeneratedCommandAccess};
use statevec_test::TestHost;

#[test]
fn withdrawal_is_refused_without_changing_state() {
    let runtime = BankRuntime;
    let mut engine = TestHost::new(registry());

    let deposit = Command::new(
        Deposit::KIND, 1, 0,
        Deposit::builder().set_account_id(42).set_amount(100).build().unwrap(),
    );
    engine.transaction(|tx| runtime.dispatch(tx, &deposit)).unwrap();
    let events_before = engine.events().to_vec();

    let withdrawal = Command::new(
        Withdraw::KIND, 2, 0,
        Withdraw::builder().set_account_id(42).set_amount(150).build().unwrap(),
    );
    let error = engine.transaction(|tx| runtime.dispatch(tx, &withdrawal)).unwrap_err();

    assert_eq!(error, BankError::InsufficientFunds);
    assert_eq!(error.rejection_code().unwrap().get(), 1);
    assert_eq!(engine.expect::<Account, _>(Account::uk(42), |r| r.total_credit()), 100);
    assert_eq!(engine.events(), events_before);
}
```

This example is also maintained as
[an executable test](../../../../demo/bank/tests/local_runtime.rs).

### Calling transaction helpers

`statevec::prelude::*` imports both `TypedTxContext` and
`RuntimeHostContextExt`. `TestHost` implements both interfaces, so calling
`engine.create_typed(...)` directly with those imports is ambiguous. Shared
handlers bounded by `Tx: TypedTxContext` select the typed interface, as in the
Bank example above. For fixture setup on a concrete `TestHost`, name the trait:

```rust
use bank::{Account, registry};
use statevec::prelude::*;
use statevec_test::TestHost;

let mut engine = TestHost::new(registry());
engine.transaction(|tx| {
    TypedTxContext::create_typed::<Account, _>(tx, |record| {
        record.init_account_id(42).set_total_credit(100).set_total_debit(0);
    })
}).unwrap();
```

Use the same explicit-trait form for other overlapping helpers, such as
`update_typed_by_uk`. Prefer `TypedTxContext` for code shared with business
handlers; `RuntimeHostContextExt` provides the plugin-host helpers.

Cover success, each business refusal, missing and duplicate records, arithmetic
boundaries and malformed payloads. Compare resulting records and events when
checking that a command sequence is deterministic.

The local engine has no WAL, checkpoints, result-chain evidence, network or
Raft. Its allocation and performance differ from the production engine.
Production fail-stop, recovery and durability require platform tests.

## Commands and time

Use generated builders and handle their `Result`. Then construct a
`statevec::Command` with its kind, external sequence, client-provided reference
time and payload.

`TypedTxContext::ref_tx_time_ns()` returns the platform's replicated transaction
reference as `Result<u64, statevec::api::ReferenceTimeUnavailable>`. Native
execution supplies the exact leader-assigned Unix-epoch nanoseconds through
simulation, validation, retained apply and replay. Zero and decreasing values
are valid; this input is not a monotonic clock or a freshness guarantee.
Command `ref_ext_time_us()` remains separate, client-provided microseconds.

For local tests, call `TestHost::set_ref_tx_time_ns(value)` or construct the
plugin wrapper with `.with_ref_tx_time_ns(value)`. This fixture input persists
until explicitly changed. `.with_ref_time(value)` only sets command external
microseconds. An unconfigured TestHost or legacy V1 host ABI returns
`ReferenceTimeUnavailable`, never a fabricated zero. Preserve this as a typed
execution failure; do not map it to a business refusal or substitute another
clock. TestHost does not establish production assignment or replay behavior.

`TypedTxContext::tx_seq()` returns the executing transaction's replicated
position as `Result<u64, statevec::api::TxPositionUnavailable>`. Every
transaction in an execution lineage, including a deterministic refusal, has a
unique, strictly increasing value that replay reproduces; values need not be
dense. Business code may derive deterministic identities from it instead of
writing a shared counter record. TestHost assigns one position per
`transaction` call, including rolled-back calls, starting at 1 or at
`TestHost::set_next_tx_seq(value)`; outside a transaction, and on the legacy V1
host ABI, it returns `TxPositionUnavailable`.

The flight-booking command helper prints `kind:payload_hex` for inspecting
generated payloads. Submission to a production cluster uses the platform
client and its request format.

## Production integration

Keep schema and handlers in the application library. Shared handlers accept
`TypedTxContext`; the production adapter supplies its transaction context,
error mapping and runtime binding.

The current production implementation uses a fixed native binding. The
platform owns node assembly, execution profile construction, Raft, storage and
engine-worker lifecycle. Application developers do not need to reproduce that
assembly using internal engine or Raft crates for local tests.

Plugin ABI types and export macros are available for integration development.
Dynamic loading and online runtime upgrades require the platform's separate
qualification and release support.

## Schema and behavior changes

Schema kinds, field layouts, keys, enum discriminants and schema versions feed
the schema identity. A different business outcome, refusal code or event needs
a different execution revision even when the schema is unchanged.

Production binds both identities to its execution profile. Existing cluster
roots refuse a different profile. Coordinate changes with the platform's data
migration and deployment process; local testing does not establish upgrade
compatibility.

Business-invariant guidance is deferred until the corresponding API changes
are selected.
