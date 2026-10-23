# Projectors

A projector builds an application read model from committed transactions. It
can maintain query tables, aggregates or a historical event view. Depend on
`statevec-projector-api` from the same release or Git revision as the other
StateVec libraries.

The [glossary](../glossary.md) defines the data and control terms used below.

Use `v1`: implement `ProjectorModule::configure`,
`ConfiguredProjector::prepare`, and `PreparedProjector::run`. Export with
`export_projector_plugin_v2!`. Run receives a scoped `ProjectorIo` on the SDK
worker, separate from the engine thread. Its source, control and scoped services
remain borrowed until actual joins complete. Attachment, process startup and
durable history recovery stay with the native host.

The separate `native` namespace contains owned-entry/receiver/control traits for
runtime integration. It is not a second step of the plugin API. Raw C tables and
host-side decoding live in `v1::{ffi, host}`.

`v1` is the first safe Rust API namespace in this naming scheme. It does not
mean C ABI 1: `export_projector_plugin_v2!` exports Projector ABI 2, while
`export_core_plugin_v3!` exports Core ABI 3. Neither C ABI changes in SDK 0.5.0.

## Consume complete Raft entries

`ProjectorSource` returns a `RaftEntryDelivery`. Its `view()` exposes ordered transaction
views, each with its actual result, events and record changes.
An output includes the schema identity, replicated transaction time, every event,
and complete record data without allocator headers: Insert carries `after`,
Update carries `before` and `after`, and Delete carries `before`. Event ordinals
and transaction order matter. Images describe the individual transaction, not
the whole entry. Repeated writes to a row expose one net change; the next
successful transaction sees that result as its before value. Rejected writes
and a create-then-delete within one transaction expose no record change.

For a WebSocket view, compare the typed fields of Update's before/after and
send only changed fields. You do not need a duplicate model merely to recover
the old value. The application still owns subscription baselines, ordering and
resynchronization after reconnect; before/after alone is not a reliable network
delivery protocol. Latest-state rebuild enumerates current records only, then
its incremental Raft entries use the same before/after contract.

Apply each complete entry atomically in your read model before coalescing rows.
An empty entry still advances the source position. A rejected transaction has
empty output. Client replies can omit events and do not replace this stream.

The live/replay context is diagnostic. Use your stored source position to avoid
applying an already covered entry twice. Reference times may be zero or decrease.

```rust
use statevec_projector_api::v1::{RaftEntryView, ProjectorError};

fn event_count(entry: &RaftEntryView<'_>) -> Result<usize, ProjectorError> {
    (0..entry.tx_count()).try_fold(0, |count, i| Ok(count + entry.tx(i)?.event_count()))
}
```

The bounded receiver reports empty and disconnected separately. A full channel
holds up further apply on the attached source. Dequeuing frees capacity but does
not acknowledge a durable projector checkpoint.

## Keep the entry token with your durable model

`entry.token()` returns a `RaftEntryToken`. Keep it with the model version it
describes. `entry.view().position()` exposes source metadata for inspection, but
those readable fields cannot reconstruct a token. For a
continuous-history source, `token.offset()` returns `Some(ConsumeOffset)` with
these four fields; latest-state tokens return `None`. A continuous consumer
must handle `None` as a mode mismatch; never replace it with a zero offset or
interpret it as a request to initialize history.

The stored continuous offset is:

```rust
use statevec_projector_api::ConsumeOffset;
let offset = ConsumeOffset {
    cluster_id: "ab".repeat(32),
    projector_id: "trading-history".into(),
    raft_term: 7,
    raft_index: 42,
};
let bytes = offset.encode_properties()?;
assert_eq!(ConsumeOffset::decode_properties(&bytes)?, offset);
# Ok::<(), statevec_projector_api::ConsumeOffsetError>(())
```

`cluster_id` is the source's 64 lowercase hex digits. `projector_id` is your
case-sensitive logical consumer name (1–128 UTF-8 bytes, no C0/DEL/C1 control
characters). It is not a peer ID or an implicit file path. Both coordinates are
nonzero `u64` values for the last completely consumed Raft entry, including an
empty barrier. Custom stores must preserve the entire integer range.

The SDK codec uses bounded UTF-8 properties, not JSON:

```text
cluster_id=abababababababababababababababababababababababababababababababab
projector_id=trading-history
raft_term=7
raft_index=42
```

Each line, including the last, ends with LF. The writer emits this field order;
the reader accepts any order but requires each field exactly once. It splits at
the first equals sign and preserves values literally: spaces, equals signs,
quotes, backslashes and Unicode in a valid projector ID are not trimmed or
unescaped. There are no comments, blank lines, continuation lines, CRLF or BOM.
Coordinates use decimal ASCII digits without signs or leading zeroes.
Missing/unknown/duplicate fields, invalid identities, overflow and inputs over
1024 bytes are typed errors that do not echo input values. This is a fixed
four-field format, not the Java Properties grammar. The old JSON codec methods
and JSON-specific error type are removed; application configuration/output JSON
remains application-owned.

Save your effects and offset together in a database transaction when possible.
If you use separate stores, save effects first and make replay idempotent.
After that durability boundary, call `token.declare_durable()` and submit the
`DurableOffsetReport` through `control.report_durable_offset`. None of receiving an entry,
dropping it, encoding/saving its offset or obtaining a token automatically
acknowledges it. The native owner validates the offer; it does not persist a
second consumer offset. Latest-state declaration returns the local
`NotContinuousHistory` error, without a host call or failure latch. The report
is an immutable Copy value of your declaration, not proof of persistence or
exclusive ownership. Only the reply establishes the owner's acceptance.

`Offer::Queued` means queued, not accepted: poll for
`ReplyPoll::Ready(ControlDecision::Accepted)` or an explicit Rejected cause.
An unsuccessful queue offer preserves the caller's report. Losing a reply does
not undo acceptance. Re-offer the exact token if needed; do not
change identity to manufacture a new attempt.

When reopening, use `v1::StartRequest::Continuous { offset }` with metadata
decoded from your actual durable store. Only confirmed absence means `None`;
read/corruption errors must not become a fresh start. The runtime checks source
IDs and joins the actual committed term/index through checkpoint restore or
replay. It does not certify the contents of your business database. A stored
offset is not a live publication token.

The cluster ID is deterministically derived from cluster configuration. Creating
a different cluster with exactly the same configuration can reuse that ID; an
old store's term/index may coincidentally match. Do not reuse a disaster-recovery
offset for unrelated history: the four fields deliberately omit a content hash.

## SDK 0.5.0 Rust vocabulary

All nine SDK crates use 0.5.0. Build the SDK, native host and plugins together
from one reviewed revision. The safe Rust API uses `v1`, `RaftEntryDelivery`,
`RaftEntryToken` and `SnapshotReader`; the C symbols, layouts and numbers stay
unchanged. No old-name aliases are supplied. Configured `HistoryMode` selects
`Continuous` or `LatestState`; `SupportedHistoryModes::Both` advertises both
capabilities but does not run both modes together. Native-only `ReplayOnly`
remains separate in `native::AttachmentRecovery`.

`EngineCheckpointPolicy::min_capture_residence_ms` is the engine's delay after
adopting a selected checkpoint before successor capture, not an application
publication interval. Native configuration keeps its existing spelling.
`Active::engine_checkpoint_blocked_at` observes a verified engine candidate
waiting for durable coverage; it grants no selection authority.

Use `PendingDurableOffset` to retain one exact report through queue pressure and
unknown replies. Each `progress()` polls at most once and offers at most once;
disconnection drops the old reply before retry. Accepted clears the declaration;
Rejected, Closed and call failures remain explicit terminal outcomes and retain
it. Same-index/same-term retry is native-idempotent within the live attachment;
later progress or another term may still reject. The helper does no persistence,
status polling or scheduling. The application decides whether OwnerNotRunning
during its own stop is expected. `PendingSnapshotConfirmation` uses the same
private mechanics for a different fact: EOF plus installed baseline, not durable P.

## Borrowed entry and Snapshot delivery

The `v1` module also provides a scoped borrower for a native V2 host
table. `ProjectorIo::split()` separates `source`, `control`, `services` and
`stop`. `source.try_next()` / `next_timeout()` return a `RaftEntryDelivery`, `SnapshotReader`,
Empty, TimedOut, Stop or Disconnected. `entry.view()` exposes ordered transactions,
borrowed events and record changes without copying their payloads. Views cannot
outlive the entry; releasing the entry does not save or acknowledge an offset.

For continuous history, obtain the `Some(offset)` from
`entry.token().offset()`. After saving your effects and that offset, offer the token's
`declare_durable()` value with `control.report_durable_offset()`. Queue Full, Closed and
the four-live-reply capacity limit are distinct from a queued owner's refusal.
A `ReplyHandle` polls without blocking; dropping a pending reply does not cancel
the submitted operation. Control and reply handles may move to scoped services
created with `services.spawn()`, not to unscoped threads or static storage. All
those services are joined before the enclosing host borrow ends.

The unsafe `v1::host::with_host` entry is for an integration that already owns a validated
host table, startup binding and callback resources that remain alive through all
actual joins. Application authors normally use the generated exporter instead.

### Engine queries

A host can also answer lookups against the engine. It then passes the
`SvProjectorHostV2Query` table and provides `ENGINE_QUERY_CAPABILITY`; otherwise
`control.engine_query_supported()` is false and `offer_engine_query()` returns
`Unsupported`. A query is a batch of up to `MAX_ENGINE_QUERY_KEYS` keys, each an
`EngineQueryKey::UniqueKey { kind, uk_id, key }` or `EngineQueryKey::SysId { kind, sys_id }`.
`key` is the encoded unique key, as `Registry::encode_unique_keys` produces it from
a record with those fields set. A system-id lookup matches only a live record of
that kind.

The handle polls like a control reply. `Ready` gives the engine's Raft position and
frontier and one optional `EngineRecordView` per key, in request order; the record
bytes stay borrowed until the handle is dropped. The whole batch is read from one
engine state between applied entries. That state may be ahead of the entries this
projector has consumed, so compare `position()` with your own model before
combining the two. `Rejected` refuses the whole batch (unknown kind or unique key,
an oversized reply, or no usable engine); `Disconnected` means the host dropped it.

For latest-state initialization, the first source delivery is a `SnapshotReader`.
`info()` gives the exact frozen baseline and live-record count. `next_record()`
lends one record, returns `Progress` for a bounded scan quantum, or returns `End`
only after the real complete scan. Neither an error nor `Stop` is EOF. Borrowed
bytes must end before another read; the snapshot reader cannot move between threads.
After `End`, `finish()` releases the native snapshot reader and yields a `SnapshotToken`.
Install the complete in-memory model before `control.offer_snapshot_confirmation(&token)`;
Accepted permits ordered delivery from S+1. No offset file or database is read
or written by this path. Dropping an unfinished snapshot reader cannot grant readiness;
an explicit model/read/service failure retains precedence over abandonment.

## Export a configured continuous-history worker

Implement `ProjectorModule` to check configuration, `ConfiguredProjector` to
prepare application resources, and `PreparedProjector` to report the actual
recovery offset and consume Raft entries in `run`. Export the module with
`statevec_projector_api::export_projector_plugin_v2!`. The generated V2 table
owns configure/prepare/start, failure observation, explicit stop, actual worker
and scoped-service joins, and explicit destruction. A failed thread spawn leaves
the prepared value with its caller; a successful spawn transfers it once.

The `projector_v2_delivery` cdylib example exports such a module using only public
SDK crates, alongside a low-level transport conformance probe. Its temporary JSON
report proves loaded delivery; it is not a durable business database or an offset
store. The configured example never offers a checkpoint and reports no recovery
offset; only the separate transport probe sends a synthetic fixture acknowledgement.
Your application must persist effects and the complete offset at its own durability
boundary before offering a checkpoint, and load that offset on restart. The
official Node adapter now wires this exporter into product configuration and the
shell lifecycle: it prepares a continuous-history learner attachment, polls the
worker, explicitly stops and joins it, and retires configured material only after
confirmed cleanup. Configured worker lifecycle is qualified within the paired
local x86_64 Linux scope.

## Persist an offset in a file

On Unix, `FileOffsetStore::initialize(directory)` explicitly creates one directory
under an existing parent and syncs both. `FileOffsetStore::open(path, cluster_id,
projector_id)` opens that directory, acquires a nonblocking exclusive writer lock
on the stable `<filename>.lock` sidecar, and validates any existing offset. Before
returning an existing selection, it syncs that same file and its opened directory;
either failure refuses open, releases writer ownership and preserves the bytes.
This confirmation must precede handing an offset to native recovery: native may
accept the restored prefix without waiting for a new entry or report. Leave
the lock file in place. Its lifetime is independent of atomic offset replacement.
The helper refuses competing writers, symlinks/non-regular selected files,
corruption and namespace mismatch; only positive absence returns `None`.

`load()` reads the actual selected bytes. `persist_offset(&offset)` writes the
bounded version-2 envelope, syncs the file, atomically replaces it and syncs the
directory before success. Exact duplicate saves are idempotent; a backwards
index or conflicting term at the same index refuses. A failed save requires
closing/reopening and inspecting the actual selection; it may contain the old
or new offset. Successful reopen re-establishes file/directory durability, not
business effects or atomicity across stores. No file/host operation runs implicitly
from Drop.

The envelope is ASCII `SVPOFF02`, a little-endian u32 payload length, at most
1024 bytes from `encode_properties()`, and SHA-256 of the header plus payload.
There are no trailing bytes. Old `SVPOFF01` JSON files are explicitly rejected
with `OffsetStoreError::Format(OffsetFileError::Header)` and left unchanged;
there is no automatic conversion or fresh fallback. Preserve old stores and
use the matching old SDK until an explicit application migration is arranged;
deleting the offset is not a safe migration. Database users persisting the four
fields directly do not change their stored representation.

An abrupt process exit before replacement can leave `.sv-offset-<pid>-<n>`
temporaries. They are never selected or adopted on reopen. The helper removes
its own unselected temporary on ordinary failure, but does not scan/delete
orphans: another offset writer may share the directory. Operators may remove
confirmed leftovers only while all writers using that directory are stopped;
leave selected offset files and stable `.lock` sidecars intact.

Persist business effects first, then call `persist_offset`, then submit the real
entry's `declare_durable()` value with `report_durable_offset`. Reporting only queues
host validation and requires its reply; it does not write your offset. DB users
may store the four fields directly in the transaction containing their effects.
The file helper stores no business model and makes no cross-store atomicity claim.

## Durable history reference consumer

Build `cargo build -p statevec-projector-api --example projector_v2_history`.
Its configuration object accepts `database_path` and optional `offset_path` and
`status_path`, with relative paths resolved against the configuration directory.
With no offset_path it commits events, current record images and the four
ConsumeOffset fields in one SQLite transaction (`WAL`, `synchronous=FULL`). With
an offset_path it commits idempotent DB effects first, then calls the default
file store. One stable DB writer lock covers the attachment; the application
owns its table/key organization. Inspect parses paths but opens no DB/store.

Recovery loads the actual selected offset; replay verifies exact previously
stored entries without repeating effects. New Update/Delete images check the
existing model against `before`. Whole-entry saves include empty barriers.
The consumer reports only after commit/save, retains the exact publication
through queue pressure/lost replies, and polls control/status during finite
idle waits. Optional status_path is an observation stream of `ready <host-prefix>
<model-prefix>`; it has no authority over either prefix. A DB commit without an
offset-file save can leave the model ahead; it becomes ready only after native
replay catches up and confirms that model prefix. All u64 coordinates use
eight-byte big-endian DB blobs to avoid SQLite's signed-integer limitation.

This example defines one local business schema, not a generic distributed DB
lease, fencing or exactly-once protocol. Use a stopped, consistent business
backup and an offset that retained native history can still validate.
Custom DB/Redis recovery must establish its backend's durability before returning
an offset, not just read a visible value after an ambiguous I/O error. SQLite WAL
recovery and sync-error/crash combinations belong to the application's SQLite
storage contract; the file helper's sync tests do not qualify those combinations.

## Latest-state reference consumer

Build `cargo build -p statevec-projector-api --example projector_v2_latest`.
Its strict configuration object is `{"report_path":"updates.jsonl"}`. Relative
paths resolve against the configuration directory. This latest-only consumer
starts with an empty private model on every invocation, enumerates the actual
state source through EOF, emits a complete `reset`, then confirms the source's
rebuild token. Only after confirmation does it consume and emit ordered `delta`
rows, checking each Update/Delete against the model's `before` image. Record
IDs, revisions and Raft indices are decimal strings to preserve all u64 values.

The append-only report is subscriber output, never a recovery store: it is
never read, has no offset/manifest, and is not fsynced as durability evidence.
Each invocation emits a newline separator before its new reset, without reading
old output. Output readers ignore empty lines, discard any prior partial row and
resynchronize at reset, replacing the previous model. `ready` marks confirmation of the reset's native baseline,
not persistence. There is no continuous-history fallback or cache selection.
For WebSocket use, replace the report writer with your subscription transport
and encode schema-specific field deltas from before/after. Client reset, delivery
and reconnect semantics remain the application's responsibility; this example
does not implement a network protocol or promise exactly-once delivery.

The paired official Node now selects this path with
`"recovery_mode":"latest-state-only"`. Its inspect command checks the selected
mode's capability without opening the report or preparing a model. The learner,
External retention and enabled checkpoint-policy requirements still apply.
This is the phase's main implementation, not completed exit qualification;
continuous-history export and explicit Unix file offset storage remain separate
paths. Build host and SDK from the paired revisions.

This SDK contains no projector database, network
subscription protocol, or Raft implementation. Local substitutes may implement
the interfaces for business tests; they cannot manufacture the production
runtime's token or publication types.
