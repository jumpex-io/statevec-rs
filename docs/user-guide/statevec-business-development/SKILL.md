---
name: statevec-business-development
description: Define StateVec data schemas and deterministic business handlers, and test them with the public Rust libraries and local memory engine. Use for application schema, command, event, handler and local-test work.
---

# StateVec Business Development

StateVec replicates commands. Production replicas execute committed commands
against the same schema and business rules. Application code defines the
records, handlers and events; the platform supplies execution and replication.

Application code uses the public `statevec` interfaces. Local tests use
`statevec-test`; production uses a separate platform engine and binding.

## References

- For records, commands, events, enums, keys and indexes, read
  [schema.md](references/schema.md).
- For handlers, errors, rejection codes and deterministic execution, read
  [business-logic.md](references/business-logic.md).
- For local tests, production integration and version changes, read
  [integration-and-testing.md](references/integration-and-testing.md).

## Workflow

1. Define the records and their immutable unique keys.
2. Define commands with all inputs needed by the business decision.
3. Write typed handlers and register them in `command_dispatch!`.
4. Check payloads before dispatch and keep business refusals distinct from host
   or schema failures.
5. Test records, events and refusals with the local memory engine.
6. Supply the schema identity and execution revision to the platform's
   production integration.

## Execution rules

- Handlers must be deterministic. Avoid clocks, randomness, environment, I/O,
  global mutable state and iteration order that can change results.
- Use checked arithmetic and validate input-derived lengths and indexes.
- A refused transaction retains none of its writes or events.
- Represent business refusals with named errors and stable rejection codes in
  `1..=59999`. Keep host and schema failures separate; classify enum variants,
  never rendered error messages.
- Check every command payload before using its generated accessors. Generated
  builders are the preferred way to construct input.
- The production platform may execute a handler during preparation,
  validation or recovery. External effects belong in a consumer of committed
  events.
- Keep schema and execution identities consistent with the deployed cluster.
  Current production integration supports one fixed interpreting version.
  Online upgrades remain platform work.

Business-invariant API changes are deferred. Keep existing callbacks intact
without adding new invariant hooks as part of ordinary application work.
