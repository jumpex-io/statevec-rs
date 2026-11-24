# StateVec User Guide

This guide covers data schemas, deterministic business handlers, commands,
events and local testing with the open-source StateVec Rust libraries.

StateVec nodes and their plugins run on Linux ARM64 (`aarch64`) and
x86-64 (`x86_64`). Supported distributions are Ubuntu 24.04 or later,
Debian 13 or later, and CentOS Stream 10 or later. Choose the release
bundle for the node's CPU architecture.

| Read | Purpose |
| --- | --- |
| [Business development](statevec-business-development/SKILL.md) | Concepts, workflow and rules |
| [Schema](statevec-business-development/references/schema.md) | Records, payloads, keys and indexes |
| [Business logic](statevec-business-development/references/business-logic.md) | Handlers, typed errors, rejection codes and determinism |
| [Integration and testing](statevec-business-development/references/integration-and-testing.md) | Local execution, production integration and version changes |
| [Cluster client](cluster-client.md) | Submission, outcomes and recovery with the public client SDK |
| [Administration](administration.md) | Official admin CLI, versioned JSON, uncertain outcomes, saved intents and collections |
| [Projectors](projectors.md) | Complete committed output, read models and durable checkpoint coordination |
| [Core plugins](core-plugins.md) | Core authoring, initialization, migration and dynamic-library exports |

The loadable [Bank](../../crates/statevec/examples/core_bank.rs) and
[Exchange](../../crates/statevec/examples/core_exchange.rs) Cores use the
public plugin ABI. The [Bank](../../demo/bank/src/lib.rs) and
[flight booking](../../demo/flight-booking/src/lib.rs) libraries exercise
business handlers in the local memory engine.

Local tests use the small `statevec-test` memory engine. Production uses the
platform engine and cluster binding. Both execute business handlers through the
shared transaction interfaces; local tests make no durability or replication
claim.

The business development guide is also an agent skill. Copy the complete
`statevec-business-development` directory into `.agents/skills/` or
`.claude/skills/` in your application repository. Use documentation from the
same revision as your StateVec dependencies.
