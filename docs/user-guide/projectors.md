# Projectors

A projector builds an application read model from committed transactions. It
can maintain query tables, aggregates or a historical event view. Depend on
`statevec-projector-api` from the same release or Git revision as the other
StateVec libraries.

The production integration supplies a source receiver and runtime control
handle. The SDK defines the consumer interfaces; attachment, process startup
and durable history recovery are supplied by the runtime.

## Consume complete frames

`ProjectionReceiver` returns an owned frame. `CommittedFrame::transactions`
exposes ordered members, each with its actual result and `TransactionProjection`.
An output includes the schema identity, replicated transaction time, every event,
and complete record data without allocator headers: Insert carries `after`,
Update carries `before` and `after`, and Delete carries `before`. Event ordinals
and transaction order matter. Images describe the individual transaction, not
the whole frame. Repeated writes to a row expose one net change; the next
successful transaction sees that result as its before value. Rejected writes
and a create-then-delete within one transaction expose no record change.

For a WebSocket view, compare the typed fields of Update's before/after and
send only changed fields. You do not need a duplicate model merely to recover
the old value. The application still owns subscription baselines, ordering and
resynchronization after reconnect; before/after alone is not a reliable network
delivery protocol. Latest-state rebuild enumerates current records only, then
its incremental frames use the same before/after contract.

Apply each complete frame atomically in your read model before coalescing rows.
An empty frame still advances the source position. A rejected transaction has
empty output. Client replies can omit events and do not replace this stream.

The live/replay context is diagnostic. Use your stored source position to avoid
applying an already covered frame twice. Reference times may be zero or decrease.

```rust
use statevec_projector_api::{CommittedFrame, CommittedTransaction};

fn event_count<F: CommittedFrame>(frame: &F) -> usize {
    frame.transactions().iter().map(|tx| tx.output().events.len()).sum()
}
```

The bounded receiver reports empty and disconnected separately. A full channel
holds up further apply on the attached source. Dequeuing frees capacity but does
not acknowledge a durable projector checkpoint.

## Keep the source cut with your durable model

`frame.checkpoint_cut()` returns the source's cut token. Keep it with the model
version it describes. Application library types can be generic over
`C: CheckpointCut`; the production integration supplies the concrete type.
`cut.position()` exposes identity, source position and actual execution frontier
for inspection. Those readable fields cannot reconstruct a token. For a
continuous-history source, `cut.offset()` returns these four fields:

```rust
use statevec_projector_api::ConsumeOffset;
let offset = ConsumeOffset {
    cluster_id: "ab".repeat(32),
    projector_id: "trading-history".into(),
    raft_term: 7,
    raft_index: 42,
};
let bytes = offset.encode_json()?;
assert_eq!(ConsumeOffset::decode_json(&bytes)?, offset);
# Ok::<(), statevec_projector_api::ConsumeOffsetError>(())
```

`cluster_id` is the source's 64 lowercase hex digits. `projector_id` is your
case-sensitive logical consumer name (1–128 UTF-8 bytes, no C0/DEL/C1 control
characters). It is not a peer ID or an implicit file path. Both coordinates are
nonzero `u64` values for the last completely consumed Raft entry, including an
empty barrier. Do not round them through a floating-point JSON consumer.

Save your effects and offset together in a database transaction when possible.
If you use separate stores, save effects first and make replay idempotent.
After that durability boundary, call `cut.checkpointed()` and offer it through
`ProjectorControl::enqueue_projector_checkpoint`. None of receiving a frame,
dropping it, encoding/saving its offset or obtaining a token automatically
acknowledges it. The native owner validates the offer; it does not persist a
second consumer offset. Native `ReplayOnly` cuts have no durable offset.

An unsuccessful queue offer preserves the unqueued publication in the runtime's
typed `RequestFailure`. Losing an acknowledgement after a successful offer does
not undo acceptance. Re-offer the exact cut if needed; do not
change identity to manufacture a new attempt.

When reopening, use `ProjectorRecovery::Continuous { offset }` with metadata
decoded from your actual durable store. Only confirmed absence means `None`;
read/corruption errors must not become a fresh start. The runtime checks source
IDs and joins the actual committed term/index through checkpoint restore or
replay. It does not certify the contents of your business database. A stored
offset is not a live publication token.

The cluster ID is deterministically derived from cluster configuration. Creating
a different cluster with exactly the same configuration can reuse that ID; an
old store's term/index may coincidentally match. Do not reuse a disaster-recovery
offset for unrelated history: the four fields deliberately omit a content hash.

## Migrating from 0.2.1 to 0.3.0

This is a source-breaking consumer change. Replace Fresh/Restore recovery with
Continuous and its optional `ConsumeOffset`. Replace the old generation/manifest
publication constructor with `cut.checkpointed()`; use `offset()` instead of
`restore_observation()`. Replace generation/cut rejection matches with offset
regression/conflict, and allow for future variants of the non-exhaustive public
enums. Rich positions now contain `ProjectionAppliedIdentity` from the complete
native AppliedBatch, not an encoded-entry digest. Core V3 and execution identity
are unchanged. Persisted stores need an application-reviewed migration: use the
correct consumer namespace and a truthful complete offset, not made-up defaults.
Record-change matches must also accept Update's `before` and Delete's `before`.
The local V1 draft has two borrowed image spans (64-byte change DTO, operations
0/1/2); rebuild host and plugin together. No extra version/capability negotiation
is introduced by this local amendment.

## Borrowed V1 delivery

The `plugin_v1` module also provides a scoped borrower for a native V1 host
table. `ProjectorIo::split()` separates `source`, `control`, `services` and
`stop`. `source.try_next()` / `next_timeout()` return a `FrameLoan`, Empty,
TimedOut, Stop or Disconnected. `loan.view()` exposes ordered transactions,
borrowed events and record changes without copying their payloads. Views cannot
outlive the loan; releasing the loan does not save or acknowledge an offset.

After saving your effects and `loan.checkpoint_cut().offset()`, offer the cut's
`checkpointed()` value with `control.offer_checkpoint()`. Queue Full, Closed and
the four-live-reply capacity limit are distinct from a queued owner's refusal.
A `ReplyHandle` polls without blocking; dropping a pending reply does not cancel
the submitted operation. Control and reply handles may move to scoped services
created with `services.spawn()`, not to unscoped threads or static storage. All
those services are joined before the enclosing host borrow ends.

The unsafe `with_host` entry is for an integration that already owns a validated
host table, startup binding and callback resources that remain alive through all
actual joins. Application authors normally use the generated exporter instead.

## Export a configured continuous-history worker

Implement `ProjectorModule` to check configuration, `ConfiguredProjector` to
prepare application resources, and `PreparedProjector` to report the actual
recovery offset and consume frames in `run`. Export the module with
`statevec_projector_api::export_projector_plugin_v1!`. The generated V1 table
owns configure/prepare/start, failure observation, explicit stop, actual worker
and scoped-service joins, and explicit destruction. A failed thread spawn leaves
the prepared value with its caller; a successful spawn transfers it once.

The `projector_v1_delivery` cdylib example exports such a module using only public
SDK crates, alongside a low-level transport conformance probe. Its temporary JSON
report proves loaded delivery; it is not a durable business database or an offset
store. Your application must still persist effects and the complete offset at
its own durability boundary. The official Node adapter that wires this exporter
into product configuration and shell lifecycle is a later integration phase.

`LatestState` is reserved for rebuilding on every startup with no saved offset;
the native delivery-phase integration rejects it until rebuild qualification.
The V1 raw tables, scoped delivery, status codecs and configured continuous-history
exporter are available. Latest-state workers and the default file offset store
remain pending; their planned APIs are not implementation evidence.

This SDK contains no projector database, checkpoint file format, network
subscription protocol, or Raft implementation. Local substitutes may implement
the interfaces for business tests; they cannot manufacture the production
runtime's cut or publication types.
