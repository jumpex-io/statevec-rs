# Writing Business Logic

Write handlers against `statevec::TypedTxContext`. The local test engine and
production engine supply their own contexts and execution wrappers.

Keep the schema and handlers together in the application library. A runtime
wrapper chooses dispatch, checks input and supplies error classification; it
does not read clocks, perform I/O or retain mutable business state.

## Dispatch

```rust
use statevec::{command_dispatch, TypedTxContext};
#[allow(unused_imports)]
use statevec::{statevec_api, statevec_model};

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
Call it before the generated accessors consume external bytes.

`try_dispatch_ledger` invokes your typed handler and returns `Ok(false)` for
an unregistered kind. The [Bank implementation](../../../../demo/bank/src/lib.rs)
contains a complete version of this pattern.

## Errors

Keep expected business refusals and execution failures distinct.

```rust
#[derive(Debug, Clone)]
pub enum LedgerError {
    Host { source: statevec::RuntimeHostError },
    InvalidInput { source: statevec::model::CommandSchemaFailure },
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

In local tests, a typed error escapes the transaction and its writes and events
are discarded. In production, the platform adapter records a business refusal
as an execution result. A host failure, unsupported input or panic has separate
failure handling; after commitment an execution failure stops the node.

The optional plugin callback returns `RuntimePluginError`. It is useful for
plugin integration tests, but that error does not carry your typed rejection
enum. Use the direct typed handler path when a test needs to assert rejection
classification.

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
retains it through replication and replay. Zero is valid, and successive values
may decrease. It is separate from the command's client-supplied
`ref_ext_time_us`, which is in microseconds.

Propagate `ReferenceTimeUnavailable` as an execution failure, not a business
refusal. For example, add a `ReferenceTimeUnavailable { source:
statevec::ReferenceTimeUnavailable }` variant and its `From` conversion to your
runtime error. Never replace an unavailable value with zero, external time or a
local clock. Local tests supply it with `transaction_at` or `run_at`, described
in [integration and testing](integration-and-testing.md#commands-and-time).

The existing plugin ABI has no transaction-time capability. Its Rust host
adapter returns `ReferenceTimeUnavailable`; using the new SDK does not add a
field to that ABI. Native transaction candidate before/after queries are
currently platform-internal and are not exposed through this SDK.

### Records and events

| Call | Use |
| --- | --- |
| `with_read_typed_by_uk::<R, _, _, _>(R::uk(..), read)` | Read a record by UK 0 |
| `with_read_typed_by_uk_id::<R, _, _, _>(id, uk, read)` | Read by another unique key |
| `update_typed_by_uk::<R, _, _, _>(uk, update)` | Update an existing record |
| `update_or_create_typed_by_uk::<R, _, _, _, _>(uk, update, create)` | Update or create |
| `create_typed::<R, _>(create)` | Create a record with initialized immutable fields |
| `delete_by_uk::<R, _>(uk)` | Delete by unique key |
| `emit_typed_event::<E>(payload)` | Emit a generated event |
| `count_index_prefix_capped::<R, _>(id, prefix, cap)` | Count up to a threshold |

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

## Determinism

Use checked arithmetic for input-derived values. Bound variable-length data
before interpreting it. Avoid panics, unchecked indexing and hash-dependent
ordering of observable results.

Carry time and other external inputs in the command or obtain them through the
qualified platform context. A local test supplies its values explicitly.
Reading a process clock inside a handler makes its behavior depend on where
it runs.

Production may invoke handlers during preparation, validation and recovery.
Publish external effects from committed events through a separate consumer.

## Version changes

Change the execution revision when the same input can produce a different
outcome, refusal code or event. Schema changes also change the schema identity.
Coordinate both with the platform release and migration workflow described in
[integration-and-testing.md](integration-and-testing.md).
