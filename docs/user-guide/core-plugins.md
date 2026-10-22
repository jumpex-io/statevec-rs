# Core plugin authoring (experimental)

Core V5 supplies a safe Rust authoring API and a generated C export. The matching
host is still undergoing qualification; building a library does not yet provide
a supported way to run a production node.

The [minimal example](../../crates/statevec-api/examples/core_v5_minimal.rs) builds
without any private StateVec crate:

```sh
cargo build -p statevec-api --example core_v5_minimal
```

For your own library, set `crate-type = ["cdylib"]` under `[lib]` and implement
these interfaces from `statevec_api::v1`:

- `CoreModule`: parse bounded configuration once. Use a unit struct as the module.
- `ConfiguredCore`: expose schema and revision, and create independent runtime
  instances from that immutable configuration.
- `CoreRuntime`: initialize, migrate, validate command payloads, run handlers,
  and declare either `TX_INVARIANTS = None` or a candidate-rule function.
  All lifecycle callbacks and the rule declaration are required.

Export the unit module with `statevec_api::export_core_plugin_v5!(MyModule)`.
The `v1` module names the safe Rust API. The macro exports Core C ABI 5;
the Rust module version and Cargo package version do not change the C ABI.
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

## Build and pack a Core package

The Linux `statevec-material` crate provides package authoring without a server
checkout or runtime-engine dependency. Build your `cdylib`, then run:

```sh
cargo build --release -p my-core
cargo run --locked -p statevec-material -- pack declaration.json configuration.bin core.svtar x86_64-unknown-linux-gnu target/release/libmy_core.so
```

From this SDK checkout, `cargo install --locked --path crates/statevec-material`
installs the same `statevec-material pack` command. The SDK executable contains
`pack` only. The server distribution supplies the extended tool with
`inspect`, `verify`, `compare`, `provision`, `bind` and `list`; its `pack` command
forwards to this implementation. Choose the appropriate distribution for an
installation prefix; they intentionally use the same executable name and pack
arguments, not two archive encoders.

The declaration is JSON with no extra fields:

```json
{
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

Replace the example values with your actual `SchemaRegistry::identity()`
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
the complete archive is bounded by 514 MiB. Output is create-only. Failure may
leave partial output for explicit inspection/removal; it is not a store
publication. Success is exit 0, invalid arguments 2, I/O 3 and material refusal
4; stdout is one bounded JSON object, never configuration/artifact contents.

## Initialization and migration

The development 0.6.0 SDK adds `initialize_core` and `migrate`; pair it with a
Core ABI 5 host. Older Core material, including ABI 4, is refused before configuration. This is
an in-progress lifecycle capability, not a claim that Node upgrade/recovery is
ready for deployment.

`initialize_core` writes the initial business state through `PluginTx` on the
current V0 runtime; it does not acquire or replace a same-version instance.
`migrate` receives source semantic `CoreVersion`, explicit input and a distinct
`MigrationTx`. `tx.records()` returns `SourceRecords` (old-layout raw reads) and
`TargetRecords` (a separate candidate based on the source).
`preserve_record` carries a source ID into a chosen target kind with revision
incremented by one, even for identical bytes; `create_record` allocates a new
ID with revision 1. `delete_record` explicitly removes a source row and checks
its successor revision. Replacement/deletion can touch each source ID only once.
Untouched compatible rows keep their bytes, ID and revision, with no row delta.
Compatibility compares canonical fields (including names), referenced types
and reserved spans, not just byte length. Indexes and rules can change.
The conservative comparison includes each field's `semantic`,
`rust_type_name` and `immutable` metadata. Changing only a consumer semantic
tag or diagnostic Rust type name still requires explicit replacement of
existing rows; byte-compatible aliases are not inferred.
Untouched incompatible or removed-kind rows reject the whole migration with
system code 5, reason 8 (`UnprocessedSourceRecord`): the plugin omitted required
handling. The same package/input/source state will refuse again.

The host validates all target values and rebuilds indexes before one candidate
invariant callback. Untouched rows are not included in change traversal; an
author's new whole-state policy must explicitly read the relevant rows. Pages
and indexes are still copied/rebuilt: zero delta is not zero CPU or memory.
Row schema metadata follows the target's global schema even for untouched rows.
This does not provide a cross-schema Projector delivery contract.

Both callbacks must be deterministic and repeatable. Proposal, follower
validation, committed apply and replay may call them independently. They must
not perform external I/O, consult a wall clock/random generator or change
external/global state. The supplied transaction time and position are
replicated inputs, not local clocks. Revision declarations must include changes
to initialization and migration behavior as well as ordinary handlers/rules.
Rejection discards the candidate, but is still a committed transaction when
applied. The callback cannot edit activation history, the chain or active V.

`PluginTx` implements the existing typed/raw record, unique-key, index-count,
supplied-id and event interfaces. Its error is `CoreExecutionError`, preserving
`Access(CoreAccessError)` separately from host, panic and ABI failures. Use `?` to
convert it into `CoreTxError`; only `CoreTxError::Rejected` carries a business code.
Time is nanoseconds from `ref_tx_time_ns()`, independent of external command
time; `tx_seq()` supplies the executing transaction's position. Neither uses a
local clock. Shared read callbacks may perform other reads. Mutable visitors
cannot reenter that transaction or retain record bytes after the call.

Candidate rules use `CandidateRead` to inspect before/after views, including
unchanged objects by record key or unique key. After-only traversal does not
fetch before values. Errors are execution failures, not absent records.

The SDK preserves callback failures even when a handler returns success. A
later panic cannot be downgraded into an ordinary callback error. The native
host remains responsible for rollback, failed-instance retirement and deciding
whether reconstruction is permitted. Catching a panic does not certify reuse.

Initial host qualification targets 64-bit little-endian Linux GNU on x86-64 and
ARM64; current loaded-library checks cover local x86-64 only. Use the exact SDK
revision paired with the host release. No V1 compatibility shim is provided.
