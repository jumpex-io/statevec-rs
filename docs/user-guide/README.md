# StateVec User Guide

This guide covers data schemas, deterministic business handlers, commands,
events and local testing with the open-source StateVec Rust libraries.

| Read | Purpose |
| --- | --- |
| [Business development](statevec-business-development/SKILL.md) | Concepts, workflow and rules |
| [Schema](statevec-business-development/references/schema.md) | Records, payloads, keys and indexes |
| [Business logic](statevec-business-development/references/business-logic.md) | Handlers, typed errors, rejection codes and determinism |
| [Integration and testing](statevec-business-development/references/integration-and-testing.md) | Local execution, production integration and version changes |
| [Cluster client](cluster-client.md) | Submission, outcomes and recovery with the public client SDK |
| [Projectors](projectors.md) | Complete committed output, read models and durable checkpoint coordination |

For working examples, see [Bank](../../demo/bank/src/lib.rs) and
[flight booking](../../demo/flight-booking/src/lib.rs).

Local tests use the small `statevec-test` memory engine. Production uses the
platform engine and cluster binding. Both execute business handlers through the
shared transaction interfaces; local tests make no durability or replication
claim.

Business-invariant guidance will be revisited with the corresponding API work.
The existing optional plugin callback remains available.

The business development guide is also an agent skill. Copy the complete
`statevec-business-development` directory into `.agents/skills/` or
`.claude/skills/` in your application repository. Use documentation from the
same revision as your StateVec dependencies.
