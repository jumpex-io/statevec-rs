# Cluster client

Use `statevec-ingress-client` to submit batches and read current records.
The [Bank writer](#run-the-bank-writer) is a runnable example that submits
a deposit and recovers it after a restart.

Pin all StateVec dependencies to the same release. After publication of
the RC1 tag, use the following dependencies.

```toml
[dependencies]
statevec-ingress-client = { git = "https://github.com/jumpex-io/statevec-rs.git", tag = "v1.0.0-rc.1" }
```

If your application defines a schema with SDK macros, also depend on the facade
under its crate name `statevec` at the same tag.

For a staged bundle, use paths into its extracted SDK. Keep `Cargo.lock`
under version control to pin the reviewed dependency graph.

## Prepare and submit

1. Give one writer exclusive use of a stream. Create a `BatchClient` with
   `BatchClient::bounded(settings, clock)`. Give `BatchClientSettings::new` a
   `DeploymentIdentity::new(cluster_identity, genesis_identity, execution_lineage)`,
   a fresh session incarnation, the stream's next sequence and configured voter
   endpoints.
   Use the deployment's command, frame and outcome limits and pass its Raft
   tick to `BatchRetryConfig::for_raft_tick`. A leader accepts new batches up to
   its `whole_batch_max_commands` limit (default and maximum 1,024). If voters
   have different limits, size new batches for the smallest one so that they
   also fit after a leader change.
2. Build each command with your application's schema. Encode it with
   `statevec_ingress_client::encode_runtime_binary_command_payload` and pass
   the ordered payloads and first input to `ClientStreamBatch::try_new`.
   One batch occupies consecutive sequences on one stream. Save the
   [original batch and deployment identities](#recover-a-saved-batch-after-a-writer-restart)
   before the first send.
3. Once connected and idle, call `try_submit`. Drive the client with
   `BatchDriverWorker<BatchTcpWorker>` or use `BatchBackground`. Keep
   polling events until the terminal result. Each client holds one batch
   at a time.

Set the deployment retry policy with `with_retry`. The default interval is
100 ms (1–60,000 ms), and handover retries default to 5 ms, capped at four
Raft ticks. Without `with_retry`, the client assumes a 15 ms Raft tick.
`with_operation_millis` defaults to 5,000 ms (1–300,000 ms).
`with_lifetime_millis` defaults to 120,000 ms (2 ms–24 hours). A batch
lifetime must cover the operation and exceed the retry interval. Supply
1–32 endpoints and an unavailable-reply limit of 1–1,024: how many
consecutive replies without progress the client accepts from one voter
before it tries the next.

### Routing after a silent connection

The SDK retries a pending batch and looks for a new leader among the voter
addresses you supplied. Let it finish within the batch lifetime. If that
lifetime expires, recover the original batch at its original sequence.
Repeated connection failures also warrant checking the deployed SDK version,
frame limits and voter addresses. A node accepts at most 32 ingress
connections across all clients and closes any further one at once.

`BatchRetryConfig::routing_poll_millis` sets how long the SDK waits for a
silent reply before checking another voter. The default is 150 ms and the
supported range is 50–500 ms. `route_discovery()` reports how often the
client looked and found a successor. After membership changes, follow the
[endpoint handoff](#move-the-writer-to-a-replacement-voter).

## Outcomes and recovery

`IngressAdmitted` reports receipt. `CommittedAwaitingApply` confirms
commitment. Save the entry binding when you see the commit or the applied
result. Advance the upstream cursor only after recording the business result.

| Terminal result | What the writer should do |
| --- | --- |
| `Applied` | Inspect every `MemberResult`. An `Outcome` carries the runtime's `system_code`, the Core's `business_code` and the returned events. Reconcile any `Unavailable` member before acknowledging it upstream. |
| `ExactAppliedResultUnavailable` | The batch was applied. Keep its binding and original input, then [reconcile the missing business result](#resolve-a-deposit-whose-exact-result-is-unavailable). |
| `NotAdmitted` | The SDK established that the batch was not admitted. Use the returned original and cause to decide how to proceed. |
| `OutcomeUnknown` | Execution is still possible. Keep the returned original and any known commit binding, then recover at the same sequence. |

You can split an oversized batch once the SDK returns `NotAdmitted` with
`TooLarge`. This requires a definite refusal on the batch's first and only
send. After a retry or import, an earlier send may have succeeded. Keep the
original batch and sequence until you know the outcome.

For example, the installed Bank returns `(system_code, business_code) = (0, 4)`
when a deposit exceeds its balance limit. The rejection consumes the sequence,
leaves the balance unchanged and emits no events.

A disconnect, timeout or local stop can leave a write uncertain. If a
`Problem` reports a suspended submission such as `CoreNotPrepared`,
`CoreRuntimeFailure` or an `Unrecognized` refusal, retain the original and
any known commit binding. Check the reported condition before recovering
at the same sequence. The Bank writer prints the cause, stops its client
and records any known commitment from the returned terminal.

### Recover a saved batch after a writer restart

Before submitting, persist the first `InputRef`, the exact encoded commands
in order, and the three identities supplied to `BatchClientSettings::new`. Save
any known commit binding with the same original. The batch itself carries
no deployment identity.

| Saved identity | What to compare |
| --- | --- |
| `cluster_identity` | The cluster derived from its name and configured `genesis_config_hash`. Use a fresh hash for an independent rebuild. |
| `genesis_identity` | The genesis record, including the initial voters and stream registry. |
| `execution_lineage` | The execution family and original engine genesis, including the initial Core. An InPlace upgrade preserves this value. |

1. Take exclusive ownership of the stream. Compare all three saved identities
   with the intended deployment. Obtain them from a known member with
   [`sv-admin query-selected-checkpoint`](administration.md#compare-cmdonly-replicas).
   If any is missing or differs, keep the original and cursor and stop before
   contacting ingress. A rebuilt cluster can have the same stream numbers.
2. Open a fresh session at the original first sequence. Reconstruct the
   exact batch and call `try_import(batch, committed)`, passing the saved
   binding or `None` when commitment is unknown. A smaller send limit still
   allows recovery up to the release ceiling of 2048 commands. Configure frame
   capacity for the original batch and its largest possible reply.
3. Resume driving the client. Record the recovered business result upstream
   before advancing the durable cursor.

#### Run the Bank writer

The [single-stream writer](../../crates/statevec-ingress-client/examples/bank_writer.rs)
uses the [installed Bank Core](../../crates/statevec/examples/core_bank/schema.rs)
and client 7, stream 9. Set `SDK` to the extracted SDK directory and build
the writer.

```sh
cargo build --manifest-path "$SDK/Cargo.toml" --locked --release \
  -p statevec-ingress-client --example bank_writer
writer="$SDK/target/release/examples/bank_writer"
```

Prepare a deployment file independently of the writer journal. Use the
`sv-admin` from the node's release bundle. The example below uses the
bundle's default single-voter setup. For an installed three-voter cluster,
set `admin_ip` and `admin_port` to a known member's admin address. In
`endpoints`, list each voter's ID and **client** address, for example
`[[1,"192.0.2.11:19120"],[2,"192.0.2.12:19120"],[3,"192.0.2.13:19120"]]`.
Set `raft_tick_millis` to the deployment's configured tick.

```sh
admin_ip=127.0.0.1
admin_port=24103
endpoints='[[1,"127.0.0.1:24100"]]'
raft_tick_millis=15

sv-admin --admin-ip "$admin_ip" --admin-port "$admin_port" \
  query-selected-checkpoint > member.json
jq -e --argjson endpoints "$endpoints" --argjson tick "$raft_tick_millis" \
  'select(.category == "selected_checkpoint" and .checkpoint.cluster_identity
     and .checkpoint.genesis_identity and .checkpoint.execution_lineage) |
   {identity: (.checkpoint | {cluster_identity,genesis_identity,execution_lineage}),
    endpoints: $endpoints, raft_tick_millis: $tick, max_outcome_bytes: 256}' \
  member.json > deployment.json
"$writer" init deployment.json writer-state
"$writer" submit deployment.json writer-state 1 7 10
```

This deposits 10 into account 7 at upstream sequence 1. The writer prints
the exact applied result and waits on standard input. Enter `ack 1` after
the upstream consumer accepts the result. The writer atomically saves cursor
1 and clears the pending original. To practice writer recovery, use a fresh
journal and keep the cluster running. Stop the writer after it prints the
applied result and before entering `ack 1`. Then resume from that journal.

```sh
"$writer" recover deployment.json writer-state
# After inspecting the recovered result, enter: ack 1
"$writer" submit deployment.json writer-state 2 8 20
# After inspecting the result, enter: ack 2
```

Recovery uses the commit binding saved before the result was printed. If the
serving node has restarted or the original reply is no longer available,
`recover` may print `needs_reconciliation`. Use the
[reconciliation procedure](#resolve-a-deposit-whose-exact-result-is-unavailable)
before submitting the next sequence. Keep the journal across writer restarts.
A second process on the same filesystem
must obtain the journal lock. Fence the old writer separately when moving
hosts. A different deployment identity stops recovery before a connection
opens.

#### Move the writer to a replacement voter

After the operators confirm the new membership, stop the old writer and
wait for it to release the journal lock. Check the three deployment
identities, replace the retired voter's ID and ingress address in
`deployment.json`, and install the updated file atomically.

Start a fresh client and driver with that file. Run `recover` for any
pending batch and settle its result before submitting the next upstream
position. With an empty journal slot, continue at the durable cursor's
successor.

#### Resolve a deposit whose exact result is unavailable

`ExactAppliedResultUnavailable` or an applied `MemberResult::Unavailable`
proves that the deposit was processed. Its business outcome could have
been a rejection. The writer saves the pending batch and its applied
binding, prints a JSON object named `needs_reconciliation`, and exits
with status 1. Capture that object after a restart with the same journal.

```sh
"$writer" recover deployment.json writer-state > pending-result.json
```

Find the original business decision in the upstream ledger, or use a
continuous Projector that recorded the result for this entry. For a
rejected deposit that emitted no events, look up the recorded result code.
Match the deployment, stream, sequence and entry to the original deposit.
A current balance alone cannot identify which deposit changed it. Persist
the acceptance or rejection upstream and obtain its durable receipt reference.
Leave the original pending if the evidence is insufficient.

```sh
jq --arg ref "$upstream_receipt" \
  '.needs_reconciliation | .upstream_receipt = $ref' \
  pending-result.json > receipt.json
"$writer" resolve deployment.json writer-state receipt.json
```

`resolve` checks the receipt against the saved applied binding. It
records the receipt reference and advances the cursor in one local
journal update. Repeating the same receipt at that cursor is safe.
This command opens no connection and submits no deposit. An uncertain
outcome or a commit awaiting apply still requires `recover`. An older
journal from this example can be opened directly. Run `recover` once
to record the applied-without-result observation before using `resolve`.

#### If the writer journal is lost

Back up the writer journal with its upstream cursor. Admin queries report
the operator stream's sequence. Projectors do not see which stream submitted
a transaction. Neither can reconstruct a missing application journal.

1. Fence every old writer. From a durable record outside the journal,
   establish the highest stream sequence any of them could have sent, called
   the bound. The Bank writer maps one upstream position to one command, so
   its last readable upstream position bounds the sequence. Other mappings
   need a record of assigned command sequences. Without a bound, keep the
   stream stopped and contact support.
2. With that bound, submit one batch starting **at least two sequences above
   it**. This leaves a gap even if every possible old command has applied.
   The node refuses it without execution. Look for `NotAdmitted` with
   `ClientError::StreamGap { expected_sequence }`. All sequences below
   `expected_sequence` have been processed. A value above the bound plus one
   contradicts the bound. Keep the stream stopped in that case.
3. Repeat the gap check until `expected_sequence` equals the bound plus one.
   An older batch can still commit after a lower reading. If the prefix
   stays short, keep the stream stopped and contact support. A missing
   result cannot establish that a command will never execute.
4. Once the entire prefix is confirmed, [reconcile each unacknowledged
   result](#resolve-a-deposit-whose-exact-result-is-unavailable) with the
   upstream ledger or a continuous Projector. Then create an application
   journal at the confirmed bound, using your stream-to-upstream mapping.
   The Bank example has no command to set this cursor.

## Direct reads

Direct reads are disabled by default. Enable client reads on a voter to use
`IngressReadOwner` for occasional point lookups. A leader also needs
leader reads enabled. Each answer reflects that voter's applied state and
may lag the leader. Use a [Projector](projectors.md)
for sustained queries or a full read model. If that service is down, keep
its query traffic off voters so writes can continue.

- Construct `IngressReadOwner::bounded(settings, clock)` with `ReadClientSettings::new` and the same deployment identities
  as the writer, the voter endpoints to read from and the deployed frame
  limit. Use `try_read(query, timeout_millis)` and drive the client with
  `BatchDriverWorker<BatchTcpWorker>` or `ReadBackground`. Consume the
  outcome with `poll_event` before starting another read on that client.
- A `ReadQuery` holds up to 256 keys in request order, by `sys_id` or
  unique key. A unique key can occupy up to 1024 bytes. An answer carries
  up to 1 MiB of record bytes and one optional record per key.
  `typed_record::<R>` decodes a found record using the generated type.
- To read your own write, use `ReadSession::Floor` and pass the applied
  transaction's `tx_seq` to `observe_write` before submitting the read.
  Subsequent reads also honor the highest position this session has read.
  With `ReadSession::Independent`, set `min_applied_tx_seq` on each query
  that needs a minimum.
- During catch-up or load, the SDK can try another configured voter within
  the read's deadline. `DeadlineExceeded` carries the last refusal, if any.
  For `ReadsDisabled`, check the voter configuration. For a size, key,
  deployment or protocol refusal, check the request and deployment before
  retrying. Resolve a pending write through the write client.
