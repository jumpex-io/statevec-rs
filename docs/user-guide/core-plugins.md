# Core plugin authoring (experimental)

Core V3 supplies a safe Rust authoring API and a generated C export. The matching
host is still undergoing qualification; building a library does not yet provide
a supported way to run a production node.

The [minimal example](../../crates/statevec-api/examples/core_v3_minimal.rs) builds
without any private StateVec crate:

```sh
cargo build -p statevec-api --example core_v3_minimal
```

For your own library, set `crate-type = ["cdylib"]` under `[lib]` and implement
these interfaces from `statevec_api::v1`:

- `CoreModule`: parse bounded configuration once. Use a unit struct as the module.
- `ConfiguredCore`: expose schema and revision, and create independent runtime
  instances from that immutable configuration.
- `CoreRuntime`: validate command payloads, run handlers, and declare either
  `TX_INVARIANTS = None` or a candidate-rule function. The declaration is required.

Export the unit module with `statevec_api::export_core_plugin_v3!(MyModule)`.
The `v1` module names the safe Rust API. The macro exports Core C ABI 3;
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
