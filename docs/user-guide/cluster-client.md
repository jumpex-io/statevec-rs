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

## Command payload codec

Use `statevec_frame::command::encode_runtime_binary_v0_command_payload(&Command)`
and `CommandDecoder::{decode_checked, decode_checked_fields}`.
`Command` is the public `statevec_model::command::Command` (also re-exported by
`statevec` and `statevec-api`). Native engine compatibility imports re-export
this exact implementation. No private engine crate is needed by a client or
signing service.

The codec tuple remains `runtime_binary_v0`, version 0, with the existing
30-byte little-endian header. The body length must fit u32 and header plus body
must fit usize; deployment request limits are checked separately by ingress.
Command kind, presence flags and sequence/time fields have their existing
meaning. Signed intent bytes and stored recovery-journal payloads do not change.
Switch imports, not stored bytes; do not decode/re-encode uncertain original
intents as a recovery migration.

`CommandPayloadError` is exhaustive: `UnsupportedCodec`,
`UnsupportedCodecVersion`, `Truncated`, `WrongMagic`, `UnsupportedVersion`,
`ReservedFlags`, `InvalidKind`, `LengthOverflow`, `TrailingBytes`,
`SequenceWithoutFlag`, and `TimeWithoutFlag`. Match these typed variants, not
Display text. The codec neither signs nor submits commands and its success is
not an admission receipt. These additions are on the unreleased 0.6.0 dev line;
pin a published revision containing them before external deployment.

## Prepare and submit

Encode commands using your application's schema. `ClientStreamBatch::try_new`
takes the stream's next sequence and the ordered payloads. A batch contains one
contiguous stream range. Construction validates the complete batch; a refusal
returns the original input. The caller provides the service's frame and command
limits, which are not negotiated over the connection.

Check request/reply codec versions, frame limits in both directions and command
counts against the deployed service before rollout. A header or limit rejection
before full decoding closes the socket without a correlated `Refused` reply.
Repeated reconnects do not repair this configuration mismatch; the original
batch may ultimately return `OutcomeUnknown`. A connection close is not proof
of `NotAdmitted`, including when the service closes excess connections at its
capacity limit. Size application connection pools for each node's ingress cap,
including failover concentration; healthy idle connections still occupy slots.

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
fully correlated batch reply does. A connection held for at least
`max(retry_millis, 5000)` also retires old idle backoff when it closes, even
without a business reply. The owner's monotonic connection timestamp decides
this; no background timer or new retry authority is added. Connection establishment remains eager,
because local batch admission requires a connected session. Pending batches
retain the configured retry interval and their original lifetime; this idle
backoff adds no retry authority, random source or new application timeout.

The default TCP worker enables kernel keepalive (5 seconds idle, 1 second
between probes, 3 probes) and, on Linux, an 8-second `TCP_USER_TIMEOUT` for
unacknowledged or zero-window-buffered data. Linux also uses that timeout
instead of the keepalive probe count: the default permits probes at 5, 6 and
7 seconds before expiry at about 8 seconds. `BatchTcpWorker::with_tcp_user_timeout`
accepts a longer physical budget (whole milliseconds in `8000..=2147483647`).
It is not derived from the owner's operation budget; TCP acknowledgements
and an application reply measure different progress. Healthy idle connections have no
application idle expiry. These physical bounds do not prove delivery or
non-admission; operation/lifetime budgets and reconciliation remain with the
client owner. Socket-option failure refuses that connection with its typed
option and I/O cause rather than silently disabling reclamation. Other
platforms use keepalive without the Linux user timeout.

`BatchBackground::spawn` uses the default physical timeout. To change it,
construct `BatchTcpWorker::with_tcp_user_timeout` and pass the worker and its
notifier to `BatchBackground::spawn_with_io`. Increasing `operation_millis`
alone does not change the socket option.

On Linux with `unshare`, `ip` and `iptables`, the opt-in kernel regression is
`cargo nextest run -p statevec-ingress-client --test tcp_liveness --run-ignored only`.
It drops a probe, checks continued use, then bounds sustained packet loss inside
a disposable user/network namespace; it never modifies host firewall rules.

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

A dispatch is not itself proof that bytes reached the socket. The physical
worker can return `WriteUnwritten`, or `ClosedUnwritten` after disposing the
socket, for an exact connection/request whose entire frame has zero written
bytes. Only that certified attempt can be excluded from delivery history. A
later sole actual Submit can then use an exact permanent refusal to prove
`NotAdmitted`. Partial writes, ordinary errors/`Closed`, imported uncertainty
and earlier possibly delivered requests remain uncertain. Request IDs are
never reused, and a delayed zero-write observation cannot undo an acknowledged
write or qualified reply. Custom drivers without this evidence remain
conservative. Local stop and lifetime expiry still return `OutcomeUnknown`;
neither is a server refusal, even if no send was observed.

For a Submit without known commitment, `InternalFailure` without a node `Failed`
observation, `CoreNotPrepared` (target not resident), or `CoreRuntimeFailure`
(local target acquisition failure) suspends the batch and publishes a problem rather than automatically
settling or replaying it. Correct the reported environment/configuration problem,
retain the original intent, and use the uncertain-original recovery path below.
Do not interpret the refusal name as proof that no earlier attempt was admitted.

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
