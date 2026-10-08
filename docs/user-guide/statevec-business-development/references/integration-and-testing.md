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

External command time is in microseconds. The host separately supplies the
transaction reference time in nanoseconds, read through `tx.ref_tx_time_ns()`.
In local tests, pass that input explicitly:

```rust
use statevec::prelude::*;
use statevec_test::TestHost;

let mut engine = TestHost::new(SchemaRegistry::with_records(Version::new(1, 0), &[]));
assert_eq!(engine.ref_tx_time_ns(), Err(ReferenceTimeUnavailable));
assert_eq!(engine.transaction_at(0, |tx| tx.ref_tx_time_ns()), Ok(0));
assert_eq!(engine.transaction_at(90, |tx| tx.ref_tx_time_ns()), Ok(90));
assert_eq!(engine.transaction_at(5, |tx| tx.ref_tx_time_ns()), Ok(5));
assert_eq!(engine.ref_tx_time_ns(), Err(ReferenceTimeUnavailable));
```

The [executable context tests](../../../../crates/statevec-test/tests/reference_time.rs)
also drive generated handlers and check rollback. `transaction_at(time_ns, f)`
retains your typed error. `PluginTestHost::run_at::<CommandType>(time_ns, payload)`
sets the same input while calling the bound plugin. Configure the independent
external time with `with_ref_ext_time_us(time_us)`.

Plain `transaction` and `run` provide no transaction time. This also applies to
an untimed transaction nested inside a timed one. A nested timed call uses its
own supplied value. Every call restores the outer value on success, error or
panic; it never leaves a time for the next call to inherit. The production
platform binds time into replicated input; business code must not read a local
clock.

The flight-booking command helper prints `kind:payload_hex` for inspecting
generated payloads. Submission to a production cluster uses the platform
client and its request format.

## Production integration

### Transaction position

`TypedTxContext::tx_seq()` exposes the executing transaction's replicated
position. It increases within one execution lineage, including deterministic
refusals, and replay reproduces it. Positions need not be dense and are not
globally unique across independent clusters.

`TestHost` assigns a position to each `transaction` or `transaction_at` call,
starting at 1; `set_next_tx_seq` changes the next fixture position between calls.
Rollback or panic preserves consumed positions. Nested calls restore the outer
position and time, and later calls do not reuse a nested call's position.
Outside a transaction the accessor returns `TxPositionUnavailable`.

Custom raw hosts may provide `tx_seq_raw`; its default reports unavailability.
Direct typed hosts use `tx_seq`. The V1 plugin ABI does not supply positions.

### Runtime binding

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

## Updating from SDK 0.2.0 to 0.2.1

This development update adds required trait methods and therefore needs source
changes in custom context implementations, despite the patch version:

- Implement `ref_tx_time_ns_raw(&self) -> Result<u64, ReferenceTimeUnavailable>`
  for `TxReadContext` and `RuntimeHostContext`.
- A direct `TypedTxContext` implementation must implement `ref_tx_time_ns` with
  the same return type. Existing blanket bridges forward to the raw method.
- Return the call's explicit input, or `Err(ReferenceTimeUnavailable)` if the
  context cannot supply it. The method has no default implementation.
- Replace `PluginTestHost::with_ref_time` with `with_ref_ext_time_us`. Use
  `run_at`/`transaction_at` when a handler also requires transaction time.
- Direct `TypedTxContext` implementations also implement `resolve_typed_uk`,
  `update_typed` and `delete_typed` for record handles. Blanket raw-context
  implementations forward to the existing key and record operations. The V1
  plugin-host adapter refuses these operations explicitly.

The [custom host example](../../../../crates/statevec-api/tests/test_runtime_context.rs)
implements explicit unavailability. The plugin ABI version and layout are
unchanged. The package version alone does not authorize a production execution
revision or an existing-data upgrade.

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
