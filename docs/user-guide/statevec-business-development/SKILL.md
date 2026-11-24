---
name: statevec-business-development
description: Develop StateVec application schemas, deterministic command handlers, events and Core candidate rules in Rust. Use for local business tests, Core plugin authoring, and application changes that evolve a deployed schema or its behavior.
---

# StateVec Business Development

Application code defines schemas and deterministic business logic with the public
`statevec` facade. Share typed handlers between the `statevec-test` memory
engine and the production Core's `PluginTx`. The host replicates commands and
supplies the transaction context.

## References

- For records, commands, events, enums, keys and indexes, read
  [schema.md](references/schema.md).
- For dispatch, rejection codes, determinism and candidate rules, read
  [business-logic.md](references/business-logic.md).
- For local tests, Core exports, version changes and cluster integration, read
  [integration-and-testing.md](references/integration-and-testing.md).

Paths to public guides and runnable examples in these references are relative
to the SDK checkout at the application's pinned release or revision.

## Workflow

1. Check the application's StateVec dependency versions. Read the schema or
   Core it already uses before changing a deployed record or command.
2. Define records, keys, command inputs and committed events with SDK macros.
3. Implement the handlers against `TypedTxContext`. Use generated builders
   and validate raw commands before calling generated accessors in direct or
   local dispatch. The Core exporter runs the schema check before the optional
   `preflight_command` hook.
4. Use `TestHost` to assert records, events and typed refusals. Exercise the
   exported Core on a matching Node when the change affects candidate rules,
   initialization, migration or cluster integration.
5. Declare the execution revision and package the Core. For an existing cluster,
   check retained data and attached Projector decoders before planning activation.

## Execution rules

- Use command inputs, the configured policy and the supplied transaction
  context to make deterministic decisions. Read transaction time with
  `ref_tx_time_ns()` and position with `tx_seq()`. Keep process clocks,
  randomness, external I/O and mutable global state out of handlers and rules.
- Use checked arithmetic and validate input-derived lengths and indexes.
- A refused transaction retains none of its writes or events.
- Represent business refusals with named errors and stable rejection codes in
  `1..=59999`. Keep host and schema failures separate; classify enum variants,
  never rendered error messages.
- A handler can run during preparation, validation, apply or replay. Perform
  external effects from a committed-event consumer using durable progress
  tracking and idempotent delivery.
- Advance the Core's execution revision when a change alters execution,
  including configuration, initialization, migration, rules or client-visible
  event delivery.
