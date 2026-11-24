# Core plugin authoring

The Core SDK supplies a safe Rust authoring API and generates the C entry point
and callback table for the host.

The [minimal example](../../crates/statevec-api/examples/core_minimal.rs) builds
without any private StateVec crate:

```sh
cargo build -p statevec-api --example core_minimal
```

For your own library, set `crate-type = ["cdylib"]` under `[lib]` and implement
these interfaces from `statevec_api::plugin`:

- `CoreModule`: parse bounded configuration once. Use a unit struct as the module.
- `ConfiguredCore`: expose schema and revision, and create independent runtime
  instances from that immutable configuration.
- `CoreRuntime`: initialize, migrate, run handlers, and declare either
  `TX_INVARIANTS = None` or a candidate-rule function. All lifecycle
  callbacks and the rule declaration are required. `preflight_command` is
  optional business preflight; the SDK validates the command schema
  ([Command schema validation](#command-schema-validation)).

Return the configured identity with
`CoreMetadata::new(name, revision, &schema)`. If the Core declares its schema
with SDK macros, add the facade to Cargo.toml under the name `statevec` and
import the macros from it.

Export the unit module with `statevec_api::export_core_plugin!(MyModule)`.
The host looks up `statevec_api::plugin::ffi::CORE_ENTRY_SYMBOL` and reads the
binary ABI major from the table header. The RC1 authoring API targets Rust 1.91.
Use the shared [glossary](../glossary.md) for the public terms.
Invoke this macro in the final `cdylib` crate, even if the implementation lives
in another library crate. The macro checks the panic strategy of the crate where
it expands; exporting through an intermediate `rlib` cannot check the final
plugin's build settings.
The SDK implements the C tables, borrowed arguments, typed status conversion,
panic boundaries and allocation-matched destructors. You do not write C glue.

Build the plugin with `panic = "unwind"` (Rust's default). The export macro
rejects `panic = "abort"`: aborting would terminate the node before the host
could retire and replace the failed instance. This does not contain process
abort, allocation abort or native memory corruption; plugins remain trusted
in-process code.

The configured revision must commit every execution-relevant setting, after
normalization. Event `inline_response` (`inlineResponse` in IDL) is part of this
commitment: changing whether an event appears in client replies requires a new
revision, even when its fields are unchanged. Schema identity excludes this
delivery policy, and the host does not hash it separately into the execution
profile. Exporting the full IDL preserves the policy but does not bind it to an
existing root.

Do not read side files or mutable global settings during execution. Live and
snapshot-candidate runtime instances can coexist; an engine can create and
discard instances repeatedly.

## Command schema validation

The exported preflight checks every command against the configured schema
before it calls `preflight_command`. It refuses a kind the schema does not
declare with `UnsupportedKind`, and a payload that fails its generated
validation with that schema error and the failing command's ordinal. The
validation checks field lengths, enum and boolean values, `FixedBytes`
lengths and padding, and that the payload is consumed exactly. The host
calls this preflight before a command first runs: when the leader prepares
a batch to propose, and when a member applies or replays an entry it did
not prepare. A refused command never reaches a handler. A refused proposal
reaches the client as `InvalidInput`, in every execution mode.

A `schema_module!` registry carries each command's generated validator. A
registry built by hand, with `SchemaRegistry::new` or decoded from IDL,
carries none: attach one for each command with
`SchemaRegistry::with_command_validator::<C>()`, where `C` is the generated
command type. It panics, as for a different payload layout, when an enum
field of `C` accepts other values than the enum the registry exports under
that name.

Configuration refuses a registry that lacks a validator for a declared
command with `ConfigureError::missing_command_validator(kind)`, which is
`Invalid` with code `ConfigureError::SDK_CODES_FROM | kind`. `Invalid` codes
from `SDK_CODES_FROM` (`0xFFFF_0000`) are reserved for the SDK; keep your
own codes below it.

`preflight_command` defaults to accepting. Implement it only for business
preflight. Its command is already schema-valid.

### Core version after a rebuild

The check also runs when a member applies or replays a committed entry. A
Core rebuilt with this SDK can refuse a committed input that its earlier
build accepted, and that member stops. A rebuild alone does not change the
derived V: V depends on the schema, the execution revision and the rule
dispatch, not on the SDK. Choose the version by the history:

- **Same V.** Keep it only when the Core's own preflight already refused
  every input the SDK check refuses, as the Bank example does. Then no
  history is processed differently.
- **New V.** Otherwise declare a new execution revision, which gives a new
  V. Upgrade InPlace as usual, and keep the earlier V's package so that
  older entries replay under it.

A package built with an earlier SDK keeps relying on its own preflight,
also under a newer host.

## Build and pack a Core package

The Linux `statevec-package` crate provides package authoring. Rust applications
that depend on this package import it as `statevec_package`. Build your
`cdylib`, then run:

```sh
cargo build --release -p my-core
cargo run --locked -p statevec-package --bin sv-package -- pack declaration.json configuration.bin core.svtar x86_64-unknown-linux-gnu target/release/libmy_core.so
```

From this SDK checkout, `cargo install --locked --path crates/statevec-package`
installs the same `sv-package pack` command. The SDK executable contains
`pack` only. The server distribution supplies the extended tool with
`inspect`, `verify`, `compare`, `provision`, `bind` and `list`; its `pack` command
forwards to this implementation. Choose the appropriate distribution for an
installation prefix; they intentionally use the same executable name and pack
arguments, not two archive encoders.

The declaration is JSON with no extra fields:

```json
{
  "format": "statevec.package-declaration",
  "version": 1,
  "schema_identity": {
    "version": "0100",
    "types": "00000000000000000000000000000000",
    "records": "00000000000000000000000000000000",
    "commands": "00000000000000000000000000000000",
    "events": "00000000000000000000000000000000"
  },
  "execution_revision": "example-rules-1",
  "has_tx_invariants": true
}
```

Keep `format` and `version` as shown. Supply your actual `SchemaRegistry::identity()`
(`Version::to_bytes()` and the four fingerprints, lowercase hex), configured
semantic revision and runtime rule-dispatch setting. The declaration does not
contain a supplied V or byte digests: the shared `CoreVersion::derive` and
packer compute them. Configuration is copied as exact bytes, not normalized
JSON. Its execution-relevant meaning must already be bound by the revision.

An optional second `TARGET ARTIFACT` adds `aarch64-unknown-linux-gnu`; in a
dual-target archive x86_64 must come first. Either target may be the only one.
The tool checks bounded input, ELF headers/dependencies and canonical tar bytes,
but never executes a plugin. `execution: "not_checked"` is deliberate; use the
matched server's `verify` before deployment to check actual ABI/configured V.
The format and semantic-V hash are the same as server-authored packages.
Manifest/configuration/artifact limits are 16 KiB / 1 MiB / 256 MiB per target;
the complete archive is bounded by 514 MiB. Output is create-only. A failed
pack may leave a partial archive to inspect or remove. Store publication is a
separate `provision` step. The tool exits with 0 on success, 2 for invalid
arguments, 3 for I/O errors and 4 for a refused Core package (`package_refused`).
It prints one bounded JSON object to stdout without configuration or artifact
contents.

## Initialization and migration

The SDK exports `initialize_core` and `migrate` in its Core table. The host
checks the declared ABI major before configuration.

`initialize_core` writes the initial business state through `PluginTx` on the
current V0 runtime; it does not acquire or replace a same-version instance.
`migrate` receives source semantic `CoreVersion`, explicit input and a distinct
`MigrationTx`. `tx.records()` returns `SourceRecords` (old-layout raw reads) and
`TargetRecords` (events and retained record-operation slots). The
`UpgradeMode::InPlace` callback may read source rows and
emit events. `preserve_record`, `create_record` and `delete_record` return
`MigrationWriteForbidden` (access cause 23); ignoring the error still refuses
activation with system code 5, reason 10 (`InPlaceWriteForbidden`). Clone,
record conversion and a copy fallback are not available.

`UpgradeMode::RevertPrevious` reactivates only the preceding successful Core
with an equal complete schema identity. It keeps current records and calls
neither `initialize_core` nor `migrate`. The target rules see an empty change
set and can read known-key records; they cannot enumerate the whole state.
Core authors and operators must check whether the old logic accepts data
written by the newer version. Each successful revert appends a new activation;
it does not undo transactions or restore old data.

InPlace permits rule/index changes, fields added within checked-zero gaps or
tail, retiring fields, and new empty kinds. Added fields must have a valid
all-zero value. `reserved` follows Protobuf's retirement semantics: retain the
old field's name and index so neither can be reused. Because StateVec records
have fixed storage, also retain its offset and length. Old bytes may be nonzero;
they remain opaque and cannot be used for new fields. Move the field to
`reserved_fields` (or keep its declaration with `#[field(index = N, reserved)]`)
and remove unique keys and canonical indexes referencing it. Registry
construction also checks UK metadata in loaded-plugin IDL: every key needs a
nonempty list of active fields supported by the host's generic encoder, with
valid encoding widths. Retired or missing fields, U128/VarBytes components and
empty key lists make the schema invalid for a Core package used at genesis or
during an upgrade.
Native custom encoders may omit the list, but IDL cannot carry those functions;
their exported metadata must independently satisfy the host requirements.
An obsolete enum may use an equal-width
scalar placeholder; its old enum domain is no longer required. Generated
updates preserve these bytes; raw full-row writers must preserve them explicitly.
Existing reserved entries cannot be removed, moved or reactivated.

Existing kinds, record sizes and retained active field facts,
including names, offsets, types, semantics and immutability, must remain
compatible; enum value sets cannot change and the schema version cannot
decrease. Diagnostic `rust_type_name` spelling does not affect compatibility.
Incompatibility returns system code 5, reason 9 (`InPlaceRecordIncompatible`)
before the callback and does not add the target to rejected-callback history.

Record pages, bytes, IDs, revisions and existing row schema headers survive
activation. No record delta is emitted. A later ordinary update stamps that
row with the active global schema version; new rows also use that version.
The host reuses unchanged host-generic indexes and rebuilds changed indexes;
native function encoders are conservatively rebuilt. New unique-key conflicts
refuse activation before target invariants run.
Adding or rebuilding an index reads existing records and can delay
transactions even when `migrate` does no work. Before deployment, test
the target Core with production row counts and representative keys. Measure
transaction latency while the upgrade runs.

The declared target invariant callback runs once per execution attempt over
the unchanged records using target indexes. Its change traversal is empty, so
a new whole-state policy must explicitly read the relevant rows. Installation
does not run the public full-state `validate_invariants()` rebuild.
Schema-bound Projectors need a matching target schema and the separately
documented rebuild procedure; an empty delta does not authorize an old decoder.

Both callbacks must be deterministic and repeatable. Proposal, follower
validation, committed apply and replay may call them independently. They must
not perform external I/O, consult a wall clock/random generator or change
external/global state. The supplied transaction time and position are
replicated inputs, not local clocks. Revision declarations must include changes
to initialization and migration behavior as well as ordinary handlers/rules.
Rejection discards prepared indexes and events, but is still a committed transaction when
applied. The callback cannot edit activation history, the chain or active V.

`PluginTx` implements the existing typed/raw record, unique-key, index-count,
supplied-id and event interfaces. Its error is `CoreExecutionError`, preserving
`Access(CoreAccessError)` separately from host, panic and ABI failures. Use `?` to
convert it into `CoreTxError`; only `CoreTxError::Rejected` carries a business code.
Read the cluster-assigned time in nanoseconds with `ref_tx_time_ns()`.
Successive committed batches can have the same time, including across leader
changes. Use `tx_seq()` for the transaction's position. Shared read callbacks
may perform other reads. Mutable visitors
cannot reenter that transaction or retain record bytes after the call.

Candidate rules use `CandidateRead` to inspect before/after views, including
unchanged objects by record key or unique key. After-only traversal does not
fetch before values. Errors are execution failures, not absent records.

The SDK preserves callback failures even when a handler returns success. A
later panic cannot be downgraded into an ordinary callback error. The native
host remains responsible for rollback, failed-instance retirement and deciding
whether reconstruction is permitted. Catching a panic does not certify reuse.

Build the Core for the node that will load it. Use `aarch64-unknown-linux-gnu`
for Linux ARM64 or `x86_64-unknown-linux-gnu` for Linux x86-64. Use the SDK
revision supplied with that node release. The [user guide](README.md) lists
the supported Linux distributions.

## Replica determinism

Every member and every replay must produce the same record changes, events
and results for the same replicated inputs. Use a defined iteration order
when the order affects these values. Hash maps and hash sets can enumerate
keys differently in separate processes. Specify portable numeric behavior
when floating-point rounding, NaNs, fused operations or platform math
functions could change a result.

Use stable record keys and explicit encodings. Pointer addresses vary
between processes. Uninitialized bytes and struct padding cannot supply
a defined value to a comparison, serialization or hash. Use the supplied
transaction time. Keep wall-clock readings, random generators, external
I/O and mutable process-global data out of transaction decisions. Change
the declared execution revision when a change affects business behavior.

Run RC1 design-partner trials in **CrossCheck** (`cross_check`). A
follower that cannot reproduce the leader's declared result withholds its
ACK and stops. A consistent quorum can continue. In **CmdOnly**
(`cmd_only`), replicas execute the committed commands independently.
Divergent transaction results and direct reads can go unnoticed. The
operator can compare the actual chain hashes at the same applied position.
[Compare CmdOnly replicas](administration.md#compare-cmdonly-replicas)

## When the Core fails

A deterministic Core failure repeats when the same input runs with the
same Core. Its effect depends on the execution mode.

- **CrossCheck** (`"cluster_mode": "cross_check"`). The leader runs each batch before it
  proposes it. A failure there commits nothing: the client receives
  `InternalFailure` and keeps the batch. When the leader isolates the
  failed instance and replaces it, the cluster keeps serving; when
  creating the replacement or retiring the failed instance fails too, the
  leader exits, without the batch in its WAL. Each retry fails again, on
  any leader, until the Core changes. Fix the Core, have the operator run an
  InPlace Core upgrade, and then recover the held batch with `try_import`.
- **A committed entry.** A member whose Core fails while applying a
  committed entry stops. A deterministic failure repeats on replay. In
  CrossCheck the leader has already run that entry, so only
  a member whose Core gives a different result fails: a nondeterministic
  Core ([Replica determinism](#replica-determinism)) or a local resource.
  In CmdOnly the leader runs only the Core's preflight before it proposes;
  the transaction first runs after commit. Each member that applies the
  entry hits the same failure. The SDK's schema check refuses a
  schema-invalid command before it is proposed
  ([Command schema validation](#command-schema-validation)); a failure on
  a schema-valid command remains. A voter can stay up at an earlier commit
  once the cluster has lost its majority. A Core upgrade is a later entry
  that no member reaches, and this release has no supported repair. Use
  CrossCheck while the Core is new.
- **Memory.** This release does not limit how much memory one transaction
  uses. The host holds the records a transaction writes and the events it
  emits until its batch completes. Size every node for the largest batch
  any of its leaders can accept. In RC1, `whole_batch_max_commands`
  defaults to 1,024 and can be set lower on individual nodes. A follower
  still applies batches accepted by a leader with a higher limit.
  When an allocation fails, the member process aborts at once,
  without the `primary.committed_core` diagnostic. The node's journal shows
  `memory allocation of N bytes failed`, or the kernel log shows that the
  process was killed for memory.
  - In CrossCheck the leader aborts before it proposes the
    batch. Sending the same batch again can stop the next leader the same
    way.
  - In CmdOnly each member that applies the committed entry aborts, and
    replay repeats it, with no supported repair.

  Bound every loop in the Core by its input, and keep each transaction's
  writes and events proportional to its command. The operator must
  monitor and alert on each node's resident memory (RSS), as "Resident
  memory" in the host repository's `docs/cluster-next/alerts.md` requires.
  The alert warns of growth and helps attribute an abort; it cannot stop
  one runaway transaction.
- **Retirement.** A Core that fails while it is destroyed, at shutdown or
  in a replacement, stops its member. A destructor that fails on every
  member stops each member that shuts down, so a stop on every member does
  not by itself name a committed entry.

For a callback that failed while applying a committed entry, the node's
terminal diagnostic includes `primary.committed_core` with the Raft index
and term, Core version and loaded package digest. Compare this evidence
across starts in the same cluster. A panic location on its own can refer
to many different inputs.

The Core's own panic handler prints the location to the node's standard
error, which the node's service unit sends to the journal:

```text
thread '<unnamed>' (1065883) panicked at src/rules.rs:132:9:
```

The thread is `<unnamed>` because the Core has its own copy of the Rust
standard library. The path is relative to the Core's build. When the
operator sets `RUST_BACKTRACE=1` for the node, the handler also prints the
stack. Function names in it come from the Core library's symbol table, so
strip only its debug sections, not its symbols. Keep the debug information
of each released Core, indexed by its ELF build ID, to read source lines
from a core dump.
