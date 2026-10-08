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
for validation and storage. Those readable fields cannot reconstruct a token.

After checkpoint data and its atomically selected manifest are synced, including
their directories, call `cut.publication(generation, manifest_digest)`. Offer the
result through `ProjectorControl::enqueue_projector_checkpoint`. Generation must
be nonzero and monotonically advance according to the runtime contract. The
runtime checks the observation before accepting coverage; a successful store
write alone does not select an engine checkpoint.

An unsuccessful queue offer preserves the unqueued publication in the runtime's
typed `RequestFailure`. Losing an acknowledgement after a successful offer does
not undo acceptance. Re-offer the exact generation and cut if needed; do not
change identity to manufacture a new attempt.

When reopening, provide `ProjectorCheckpointRestoreObservation` decoded from
your actual durable store. It is untrusted input: the runtime must join the
restored position with its own checkpoint or replay before allowing new
checkpoint coverage. A stored position is not a live publication token.

This SDK contains no projector database, checkpoint file format, network
subscription protocol, or Raft implementation. Local substitutes may implement
the interfaces for business tests; they cannot manufacture the production
runtime's cut or publication types.
