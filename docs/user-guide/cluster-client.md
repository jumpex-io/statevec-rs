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

With no pending batch, consecutive connection failures use capped exponential
backoff starting at `retry_millis`, doubling up to `max(retry_millis, 5000)`.
An immediate EOF after a successful TCP handshake does not reset it; a valid,
fully correlated batch reply does. Connection establishment remains eager,
because local batch admission requires a connected session. Pending batches
retain the configured retry interval and their original lifetime; this idle
backoff adds no retry authority, random source or new application timeout.

The default TCP worker enables kernel keepalive (5 seconds idle, 1 second
between probes, 3 probes) and, on Linux, a 5-second `TCP_USER_TIMEOUT` for
unacknowledged or zero-window-buffered data. Healthy idle connections have no
application idle expiry. These physical bounds do not prove delivery or
non-admission; operation/lifetime budgets and reconciliation remain with the
client owner. Socket-option failure refuses that connection with its typed
option and I/O cause rather than silently disabling reclamation. Other
platforms use keepalive without the Linux user timeout.

The shared frame reader validates the complete header before allocating, then
starts with at most 64 KiB and grows fallibly only as bytes arrive. Receiving a
header cannot reserve the entire declared frame. The configured maximum still
limits a complete frame and must fit the application's memory budget; this is
not a process-wide memory cap, nor protection from an unaffordable fully
received/decoded frame. Allocation failure retires the connection without
discarding the client's original batch or proving it was not admitted.

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
