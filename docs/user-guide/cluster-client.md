# Cluster client

`statevec-ingress-client` connects an application to a StateVec cluster. It uses
the same intent and request/reply codecs as the production nodes. Node storage,
Raft replication and checkpoint formats stay in the production repository.

Use the same release or Git revision for all StateVec dependencies:

```toml
[dependencies]
statevec = { git = "https://github.com/jumpex-io/statevec-rs.git", branch = "dev" }
statevec-ingress-client = { git = "https://github.com/jumpex-io/statevec-rs.git", branch = "dev" }
statevec-frame = { git = "https://github.com/jumpex-io/statevec-rs.git", branch = "dev" }
```

Keep `Cargo.lock` under version control to pin the reviewed revision.

## Prepare and submit

Encode commands using your application's schema. `ClientStreamBatch::try_new`
takes the stream's next sequence and the ordered payloads. A batch contains one
contiguous stream range. Construction validates the complete batch; a refusal
returns the original input. The caller provides the service's frame and command
limits, which are not negotiated over the connection.

Construct `BatchClient::bounded` with the cluster, genesis and execution-profile
identities from the deployment configuration, a fresh session incarnation, the
stream identity, endpoints and time budgets. A client holds one batch at a time.
An application must exclusively own its stream and track its idle boundary.

Submit at a known idle boundary with `try_submit`. Drive socket I/O with
`BatchDriverWorker<BatchTcpWorker>`, or use `BatchBackground` for background and
async delivery. The client owns retry, reconciliation and endpoint rotation.
Driver diagnostics report consumed faults; keep driving the same client rather
than turning every diagnostic into an application stop.

## Outcomes and recovery

Progress events leave the batch in client custody. Consuming a terminal event
returns the result or the complete original input and releases the local slot.
`Applied` carries complete application evidence. `NotAdmitted` proves the batch
was not admitted under the client's proof rules. `OutcomeUnknown` cannot prove
that execution did not happen.

For an uncertain original, preserve its payloads and any known commitment, then
use `try_import` in a fresh session. Do not invent a new sequence or submit a
different command to replace an uncertain original. A clock failure stops the
session and returns retained work as unknown; ordinary connection failures are
handled by the client's recovery policy.

The client retains state in memory. Applications needing recovery after their
own process crashes must durably record the original intent and recovery
evidence before losing them. That application boundary is not provided by the
local test engine.

## Library tests

The public workspace runs codec boundaries, client ownership and byte-driver
schedules without a production engine dependency. Real multi-node, WAL,
checkpoint and Bank composition tests remain in the production repository.
