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
and full record after-images or deletion markers. Event ordinals and transaction
order matter. Insert and update are distinct.

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

`LatestState` is reserved for rebuilding on every startup with no saved offset;
the native delivery-phase integration rejects it until rebuild qualification.
The V1 raw tables/status codecs are available, but a configured dynamic worker
and the default file store are not enabled by this release's groundwork alone.

This SDK contains no projector database, checkpoint file format, network
subscription protocol, or Raft implementation. Local substitutes may implement
the interfaces for business tests; they cannot manufacture the production
runtime's cut or publication types.
