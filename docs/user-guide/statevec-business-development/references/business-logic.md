# Writing Business Logic

Write handlers against `statevec::TypedTxContext`. The local test engine and
production engine supply their own contexts and execution wrappers.

Keep the schema and handlers together in the application library. A runtime
wrapper chooses dispatch, checks input and supplies error classification; it
does not read clocks, perform I/O or retain mutable business state.

## Dispatch

```rust
use statevec::{command_dispatch, TypedTxContext};

pub struct LedgerRuntime;

command_dispatch! {
    fn try_dispatch_ledger;
    runtime = LedgerRuntime;
    error = LedgerError;
    Deposit => Self::handle_deposit,
    Withdraw => Self::handle_withdraw,
}

impl LedgerRuntime {
    pub const EXECUTION_REVISION: &'static str = "ledger-execution-v1";

    pub fn dispatch<Tx: TypedTxContext + ?Sized>(
        &self,
        tx: &mut Tx,
        command: &dyn statevec::RuntimeCommandEnvelope,
    ) -> Result<(), LedgerError>
    where
        LedgerError: From<Tx::Error>,
    {
        self.preflight_try_dispatch_ledger(command.command_kind(), command.payload())?;
        if self.try_dispatch_ledger(tx, command)? {
            Ok(())
        } else {
            Err(LedgerError::UnsupportedCommand { kind: command.command_kind() })
        }
    }
}
```

The generated `preflight_try_dispatch_ledger` checks the static payload schema,
including lengths, enum values, booleans, fixed-byte padding and trailing bytes.
It returns `CommandSchemaError::UnsupportedKind` for an unregistered kind.
Call it when your own dispatcher accepts an encoded command directly. The
generated dispatch function wraps bytes for its matched type.

`try_dispatch_ledger` invokes your typed handler and returns `Ok(false)` for
an unregistered kind. See `demo/bank/src/lib.rs` for the complete pattern.

### Exported Core dispatch

`export_core_plugin!` checks each command against the configured registry
before the optional `CoreRuntime::preflight_command` runs. Use that hook for
stateless checks that can decide from the command alone. Evaluate decisions
that read or change business state inside `run_tx` or the candidate rule.
If a declared kind lacks a validator, Core configuration fails. Use the
`schema_module!` registry or explicitly attach a generated validator with
`SchemaRegistry::with_command_validator::<C>()`. See the command schema
validation section in `docs/user-guide/core-plugins.md`.

## Errors

Keep expected business refusals and execution failures distinct.

```rust
#[derive(Debug, Clone)]
pub enum LedgerError {
    Host { source: statevec::RuntimeHostError },
    InvalidInput { source: statevec::model::CommandSchemaError },
    UnsupportedCommand { kind: statevec::CommandKind },
    InsufficientFunds,
    UnknownAccount,
}
```

Implement the relevant `From` conversions, `Display` and `Error`. A stable
business-code mapping matches the error variants:

```rust
impl LedgerError {
    pub fn rejection_code(&self) -> Option<statevec::BusinessRejectCode> {
        match self {
            Self::InsufficientFunds => statevec::BusinessRejectCode::new(1),
            Self::UnknownAccount => statevec::BusinessRejectCode::new(2),
            Self::Host { .. } | Self::InvalidInput { .. }
                | Self::UnsupportedCommand { .. } => None,
        }
    }
}
```

Codes `1..=59999` belong to the business runtime; zero means success. Never
classify an error from its rendered message.

In a local `TestHost::transaction`, return the typed error to discard the
transaction's writes and events. In the exported Core, map intentional business
refusals to `CoreTxError::Rejected`. Propagate host and schema errors through
their execution-error paths. Schema-invalid ingress is refused as
`InvalidInput`. A Core execution failure while applying a committed entry
stops the member.

The SDK's `RuntimePlugin` test wrapper returns `RuntimePluginError`. Use
`TestHost::transaction` with your typed dispatcher when asserting the
application error variant and its rejection code.

## Transaction access

### Transaction reference time

Handlers read the execution time supplied by the host:

```rust
use statevec::{ReferenceTimeUnavailable, TypedTxContext};

fn execution_time<Tx: TypedTxContext + ?Sized>(tx: &Tx) -> Result<u64, ReferenceTimeUnavailable> {
    tx.ref_tx_time_ns()
}
```

The value is in nanoseconds. Production assigns it before simulation and
retains it through replication and replay. The cluster keeps the value
non-decreasing in committed order. Zero is valid. The command's client-supplied
`ref_ext_time_us` is a separate value in microseconds.

Propagate `ReferenceTimeUnavailable` as an execution failure. In a custom
runtime error enum, retain the typed cause and implement its `From`
conversion. Local tests set the time with `transaction_at` or `run_at`, as
shown in [integration and testing](integration-and-testing.md#commands-and-time).

In a Core plugin, `PluginTx` supplies the host's value. Candidate rules read
it and candidate state through `CandidateRead`.

### Records and events

| Call | Use |
| --- | --- |
| `with_read_typed_by_unique_key::<R, _, _, _>(R::unique_key(..), read)` | Read a record by UK 0 |
| `with_read_typed_by_unique_key_id::<R, _, _, _>(id, uk, read)` | Read by another unique key |
| `update_typed_by_unique_key::<R, _, _, _>(uk, update)` | Update an existing record |
| `update_or_create_typed_by_unique_key::<R, _, _, _, _>(uk, update, create)` | Update or create |
| `create_typed::<R, _>(create)` | Create a record with initialized immutable fields |
| `delete_by_unique_key::<R, _>(uk)` | Delete by unique key |
| `resolve_typed_unique_key::<R, _>(uk)` | Resolve UK 0 to the record's system id |
| `with_read_typed::<R, _, _>(sys_id, read)` | Read by system id |
| `update_typed::<R, _, _>(sys_id, update)` | Update by system id |
| `delete_typed::<R>(sys_id)` | Delete by system id |
| `emit_typed_event::<E>(payload)` | Emit a generated event |
| `count_index_prefix_capped::<R, _>(id, prefix, cap)` | Count up to a threshold |

Every by-key call resolves its unique key again. When a handler reads a record
and later updates or deletes it in the same command, resolve the key once and
use the system id, which names that record for its whole life; after deletion
the id names no record. Core plugins (`PluginTx`) and `TestHost` support these
handles.

Read and write calls return typed host errors. Propagate them into your error
enum. Check an expected duplicate or missing record and return a business
refusal explicitly. Creating a duplicate unique key without that check returns
a host error, not a business rejection code.

A record's unique-key fields are immutable. Update callbacks may change other
fields; the host rejects an attempted immutable-field change.

Closures can return an application `Result`. Handle the outer host error and
the inner business error separately. Earlier provisional writes are discarded
if the enclosing transaction is refused.

Build an event with `E::builder()` and handle its `build()` result before
emitting it. A failed transaction publishes none of its provisional events.

## Candidate rules

Declare `CoreRuntime::TX_INVARIANTS` as `None` or
`Some(rule)`. The rule receives the runtime and a read-only `CandidateRead`
view of the transaction's proposed changes. Use `for_each_after_change` when
the rule needs only change keys and after-images. Use `for_each_change` when
the rule needs before-images, including the prior contents of deleted rows.
Check the kind before decoding row bytes. Borrow each view within its callback.

Resolve related records by known keys, including unchanged rows. Return a
stable business rejection when the candidate violates policy and propagate
access or view failures. For example, the loadable Bank Core validates the
maximum account balance in `crates/statevec/examples/core_bank/business.rs`.
An ordinary `TestHost` transaction exercises the handler. Verify the rule
dispatch on the exported Core and assert that rejection preserves both records
and events.

InPlace activation preserves rows, so the target rule's change traversal is
empty. If the new policy must validate existing rows, read them through
`MigrationTx`'s source view in `migrate`. Review this explicitly even for a
rules-only change. See `docs/user-guide/core-plugins.md`.

## Determinism

Use checked arithmetic for input-derived values. Bound variable-length data
before interpreting it. Avoid panics, unchecked indexing and hash-dependent
ordering of observable results. V1 does not limit the memory one transaction
uses; keep each transaction's writes and events proportional to its command.

Carry time and other external inputs in the command or obtain them through the
qualified platform context. A local test supplies its values explicitly.
Reading a process clock inside a handler makes its behavior depend on where
it runs.

Production may invoke handlers during preparation, validation, apply and
replay. Drive external effects from committed events in a separate consumer.

## Version changes

Change the execution revision when the same input can produce a different
outcome, refusal code or event. Include normalized policy settings and
lifecycle or candidate-rule behavior in that revision. Coordinate the
revision and schema with the migration workflow in
[integration-and-testing.md](integration-and-testing.md).
