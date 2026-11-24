# Official administration tool

Use the `sv-admin` binary from the matching official host distribution.
Applications invoke it as a subprocess; they do not need a private Rust
dependency, an admin protocol encoder, or an SDK-side admin server. The bundle
manifest pins the host and SDK revisions together for deployment. For initial
setup, use the admin and node binaries from that bundle.

The release bundle includes an installation guide at
`docs/cluster-next/deployment.md`. It explains how to install the systemd
services, configure networking and monitoring, and initialize a cluster.
Choose the Linux bundle for your node's architecture. The [user guide](README.md)
lists the supported distributions.

Detailed operating semantics, including the upgrade procedure, are in the host
repository's `docs/cluster-next/package.md` and `docs/cluster-next/upgrade.md`;
WAL cleanup policy is in `docs/cluster-next/wal-cleanup.md`, the
member-by-member restart procedure in `docs/cluster-next/restart.md`, and
the shipped alert rules and their procedures in `docs/cluster-next/alerts.md`.
This page is the stable invocation and JSON contract.

## Invocation and authority

Every command except `combine`, `render-alert-rules`, `support`,
`cleanup-wal --members-file`, `check-restart` and `transfer-leader` takes
`--admin-port PORT` and optional `--admin-ip IP` (default `127.0.0.1`). The
node enables its dedicated TCP admin endpoint with `admin_listen_port`; its
separate `admin_listen_ip` also defaults to `127.0.0.1`. Omitting both config
fields disables admin. Ports must be in `1..=65535`. Use literal IPv4 addresses
for the installed deployment profile. Client destinations require a specific
address.
Operations chooses the internal listen IP for cross-host use. No TLS,
authentication, authorization or Unix compatibility mode is provided.
Metrics/status HTTP, business ingress and peer transport retain separate
ports. `--help` alone returns a versioned JSON usage object; `--version`
alone prints the release version as one plain-text line.

An explicit endpoint always needs a port; `--admin-ip` alone is invalid.
For `support`, omitting both options selects the local Node configuration.

The five membership mutations, `prepare-core` and `request-checkpoint` also
require `--cluster-identity CLUSTER_HEX --expected-peer RECEIVING_PEER`.
`--peer-id` still names the member to change; it is not the receiving peer.
Use trusted inventory or local `sv-node inspect NODE_CONFIG` fields
`configured_cluster_identity` and `peer_id`, not an endpoint reply, for this
expectation. The node checks it before any local job or membership proposal.
`target_mismatch` (exit 4) includes `request.expected` and `actual`, each with
hex `cluster_identity` and decimal-string `peer_id`; membership requests also
carry `admission: not_admitted`. Unbound older mutation frames are refused.
This is a destination check, not authentication or proof of current membership.
Operator requests retain their SDK cluster/genesis binding; cleanup retains
its selected-root/storage/checkpoint target and owner checks.

Client timeout and disconnect never cancel a sent mutation. A request queued
behind another connection can execute after the caller has reported unknown.
An intervening unchanged read is not permission to reissue it. Stop for
investigation if a membership proposal identity was lost; reconcile the same
saved lifecycle intent for operator-stream work instead of creating a new one.

Loopback limits remote reachability; it does not isolate users on one host.
Any local process that can connect has full admin access, including
OperatorStream, CleanupWal, PrepareCore, membership changes and QueryRecords.
Deployment must supply firewall or network-namespace restrictions when access
is restricted. Remote-source filtering alone does not isolate local users.

The node checks listener overlap and reserves admin before Startup without
listening. It starts listening at Binding. A direct connection during Startup
is refused and remains not-sent; a dropped connection reaches its timeout.
The bound socket uses `SO_REUSEADDR`, so a competing process can still bind
and listen during Startup. In that case Binding fails; it does not close the
competing listener.

| Group | Commands |
| --- | --- |
| Membership | `query-membership`, `add-learner --peer-id DECIMAL_U64`, `promote-learner --peer-id DECIMAL_U64`, `remove-peer --peer-id DECIMAL_U64`, `enter-joint [--add-learners ID[,ID...]] [--remove-peers ID[,ID...]]`, `leave-joint` |
| Spare inventory | `remaining-spares --genesis-record ABSOLUTE_PATH [--inventory ABSOLUTE_PATH]` on the current leader. The exported genesis and operator-maintained inventory must name the same history. Success lists unused IDs by eligibility and recorded retirements. Missing or incomplete inventory returns exit 4 and `remaining: null`. See the deployment guide in the host repository for the inventory schema. |
| Operator stream | `query-operator-frontier`, `operator-stream --request-hex SVIN_FRAME_HEX` |
| Checkpoint | `request-checkpoint`, `query-selected-checkpoint` |
| WAL retention | `query-wal-retention`, `cleanup-wal [--manual] [--max-segments 1..=64] [--peer-wait all\|voters] [--min-voter-space BYTES\|PERCENT%] [--learner-cleanup eligible\|min-learner-space\|manual] [--min-learner-space BYTES\|PERCENT%]`, `cleanup-wal --members-file ABSOLUTE_PATH --cluster-identity HEX [--max-segments 1..=64] [--peer-wait all\|voters] [--learner-cleanup eligible\|min-learner-space\|manual] [--min-learner-space BYTES\|PERCENT%]` (no single-member endpoint options); see [WAL retention and cleanup](#wal-retention-and-cleanup) |
| Restart | `check-restart --cluster-identity CLUSTER_HEX --members-file ABSOLUTE_PATH --stop DECIMAL_U64 [--leader \| --handover-report ABSOLUTE_PATH] [--major-roll] [--wait-ms 0..=86400000]`, `check-restart --cluster-identity CLUSTER_HEX --members-file ABSOLUTE_PATH --returned DECIMAL_U64 --stop-report ABSOLUTE_PATH [--major-roll] [--wait-ms 251..=86400000]` (no single-member endpoint options); see [Restart checks](#restart-checks) |
| Leader transfer | `transfer-leader --cluster-identity CLUSTER_HEX --members-file ABSOLUTE_PATH --source DECIMAL_U64 --target DECIMAL_U64 [--max-raft-index-gap N] [--wait-ms 0..=86400000] [--expected-target-build-sha256 HEX] [--for-source-stop]` (no single-member endpoint options; the wait defaults to 10000 ms); the host repository's `docs/cluster-next/restart.md` gives the procedure |
| Records | `query-records (--sys-id KIND:SYS_ID \| --unique-key KIND:UK_ID:KEY_HEX)...`, 1 to 16 keys, answered in order |
| Observations | `query-observations --group node\|wal\|checkpoint\|projector\|core-residency\|replication\|submission\|execution\|memory\|services\|transport\|peer-routes\|diagnostics [--offset N]`; only `replication`, `memory`, `services` and `peer-routes` are paged |
| Local packages | `query-core-residency [--cursor INSTANCE:GENERATION:START] [--limit 1..=8]`, `prepare-core --core-version HEX --package-digest HEX --target TRIPLE`, `query-provisioned-core [--after V_HEX:PACKAGE_HEX] [--limit 1..=8]`, `check-core-presence --core-version HEX --target TRIPLE`, `verify-core-package --core-version HEX --target TRIPLE` |
| Lifecycle views | `query-active-core`, `query-core-activations [--after TX_SEQ] [--frontier FRONTIER] [--limit 1..=24]`, `query-required-core [--after V_HEX] [--frontier FRONTIER] [--limit 1..=24]`, `check-required-core --target TRIPLE [--stored-bytes]`, `query-pending-lifecycle` |
| Saved intent | `create-lifecycle-intent --intent ABSOLUTE_PATH (--initialize-core \| --source-version HEX --target-version HEX [--upgrade-mode in-place\|revert-previous]) [--input-hex HEX]`, `submit-lifecycle --intent ABSOLUTE_PATH --prepared-members ABSOLUTE_PATH --package-digest HEX [--preflight-ms 1..=600000] [--wait-ms 1..=600000]` (both budgets default to 30000), `reconcile-lifecycle --intent ABSOLUTE_PATH [--wait-ms 1..=600000]` |
| Alert rules | `render-alert-rules --inputs ABSOLUTE_PATH --output ABSOLUTE_NEW_PATH` (offline, no endpoint options); see [Render alert rules](#render-alert-rules) |
| Support | `support --config ABSOLUTE_PATH --log-file ABSOLUTE_PATH --output ABSOLUTE_NEW_DIRECTORY [--admin-ip IP] [--admin-port PORT] [--latest-start START_ID] [--cluster-identity HEX] [--release-manifest ABSOLUTE_PATH] [--include-raw-logs]`; see [Support collection](#support-collection) |
| Collection | `collect --collection ID --cluster-identity HEX --genesis-identity HEX --peer-id DECIMAL_U64`, `combine --collection ID --cluster-identity HEX --genesis-identity HEX --members PEER[,PEER...] [--core-version HEX --package-digest HEX] --file PATH...` (offline, no endpoint options) |

`TRIPLE` is `x86_64-unknown-linux-gnu` or `aarch64-unknown-linux-gnu`.
Membership changes return `proposal_accepted`, never applied success. Observe
all four role sets with `query-membership`; follow the host repository's
`docs/cluster-next/membership.md` for identity checks, unknown outcomes and
replacement. Joint input needs at least one nonempty list. The tool preserves
both lists for exact reply matching; installed genesis and the node check IDs
and eligibility. Standalone AddLearner cannot demote a voter. Voter-set changes
can return `membership_quorum_unavailable` (exit 4) when the candidate lacks a
recently active majority. This sampled guard is not a reservation or fencing.
There is no retry, apply wait, force option or automatic leader following.

`FRONTIER` is `ROOT_GENERATION:APPLIED_INDEX:APPLIED_TERM:TX_SEQ:CHAIN_HEX`,
copied exactly from a previous lifecycle page. A collection `ID` has 1 to 64
characters from `A-Z a-z 0-9 . _ -`.

`--timeout-ms` is optional (1 through 60000 inclusive). The default is twice
the node's owner reply wait (10000 ms), so a node that gives up waiting on its
owner can still deliver its typed unknown reply. `query-core-residency` reads a
published view and defaults to 5000 ms. The timeout bounds each physical
connect/send/receive exchange, including partial reads and writes. It does not
cancel accepted work or prove non-admission. Unknown, duplicate or
cross-command options, missing values, relative or unrepresentable paths,
non-decimal/out-of-u64 IDs and invalid timeouts are refused before connecting.
Leading decimal zeros are allowed; zero peer IDs are sent to the owner, which
alone decides membership eligibility.

A host outage or packet DROP can consume the full physical connect timeout.
Sequential member reads accumulate these waits. `submit-lifecycle` preflight
keeps its common budget. `cleanup-wal --members-file` makes one bounded pass.
`check-restart` completes its current sample and starts no later sample after
its wait deadline; 250 ms is the pause between samples, not a guaranteed
sampling period. No timeout triggers a retry.

The tool never automatically follows a leader hint or retries a request,
rewrites topology, loads a package as a side effect,
or resequences an intent. Membership changes require explicit commands and
node-side admission; the tool does not autonomously promote/remove peers or
own membership policy. Learner IDs/endpoints must already be allowed by the
node's installed topology. The command table above describes the currently
exposed operations.

`proposal_accepted` means the owner proposed the change. It does **not** mean
the change committed/applied. A later `query-membership` observes the current
applied membership, not a durable receipt for an earlier particular request.
After an unknown outcome, inspect that membership and reconcile before choosing
another explicit operation. Do not automatically replay an uncertain request.

## JSON v1 and exits

Stdout is one JSON object plus newline, at most 262144 bytes (four admin
replies of 64 KiB). Every object has
integer `version: 1` and string `category`. All u64-valued IDs, indices, terms
and sequences are **decimal strings**, including arrays and refusal fields.
Observation pages (`query-observations`, and `support`'s observation and
diagnostics files) are the exception: they carry u64 values as JSON numbers.
Read them as unsigned 64-bit integers, never as doubles; a peer ID may exceed
2^53.
Hashes/digests are lowercase hex. Booleans remain JSON booleans; optional
observations are `null` when absent. No membership/WAL/checkpoint DTO is
published in the SDK.

**Ordinary node replies carry `peer_id`** from the answering node's local
configuration, so saved outputs from several nodes stay attributable. The
frozen compatibility refusal carries the answering build's version and
supported ranges and omits the peer ID. Other single-request replies have
the peer ID at the top level, as does `check-required-core` (all of its replies must come from that one node,
and every lifecycle page must name the first page's cluster, genesis and
lineage).
`collect` records it as `observation.peer_id` beside the operator's echoed
`target.peer_id`; `check-restart` echoes its target the same way and puts
each answering member's `peer_id` in its `members` entries. Outputs produced before any reply (input, connect and setup
failures), the intent commands and the offline `combine` have no top-level
`peer_id`, with one exception: when `create-lifecycle-intent`'s frontier read
fails, its output is that node reply, with its `peer_id`.

| Exit | Meaning |
| --- | --- |
| 0 | A known observation or result. It is not upgrade success unless the category says so. |
| 2 | `invalid_input`: nothing was sent. |
| 3 | The operation request was not sent: `io_failure` at `setup`/`connect`, an unavailable saved intent or lifecycle preflight, or `unsupported_platform`. Checked Submit may already have sent read-only queries; see below. |
| 4 | A known refusal or negative answer from the node, or an observed lifecycle preflight refusal (for example `owner_refusal`, `queue_full`, a busy or refused view, a stale continuation, a conflict). |
| 5 | Unknown: the outcome of a sent request is not known (`outcome_unknown`, `owner_reply_unknown`, unknown preparation or verification, an unsettled intent, an incomplete collection). |
| 6 | Output failed after the request may have been sent. The outcome is unknown; never infer non-admission. |

The admin envelope and the ingress frame inside `operator-stream` each
have a CRC. For a damaged local request, the CLI exits with code 2,
`cause = invalid_frame` and `frame.kind = frame_checksum` before sending.
For a damaged reply, the CLI exits with code 5 and
`cause.kind = frame_checksum`. Reconcile the original intent.

For a single-request command such as `operator-stream` or `prepare-core`,
exit 3 means that request was not sent. For `submit-lifecycle`, it means this
invocation sent no saved Submit; preflight may already have sent read-only
queries, including when the final Submit connection fails after a successful
check. Its stderr note makes this distinction. Neither exit 3 nor a failed
preflight settles an earlier unknown Submit: reconcile the same saved intent.

**`owner_reply_unknown` (exit 5)**: the node accepted the request, but its own
wait for the Raft owner ended first. It has `request` and `cause`
(`timeout` or `disconnected`). For a membership change or operator batch,
`admission` is `unknown`. Accepted work is not cancelled. It is never a
refusal and never permission to send a different request.

**Exit 6**: writing or flushing stdout failed after a send attempt. Stdout may
be incomplete or empty. An output-size failure after a send emits a small
`output_failure` object with `stage: "output"`, `cause.kind: "output_bound"` and
`admission: "unknown"`. The same failures before the operation request is sent
are exit 3 `io_failure` with `stage: "output"` and `admission: "not_sent"`.
Read-only preflight queries do not turn an unsent Submit into exit 6. Stderr then
carries a one-line note; it is supplementary, not a stable protocol. A consumer
must check both the exit status and complete JSON.

### Categories

| Exit | Category | Notes |
| --- | --- | --- |
| 0 | `applied_membership` | `applied_index`, `applied_term`, nullable `engine_tx_seq`/`tx_chain_hash`, `voters`, `voters_outgoing`, `learners`, `learners_next`, `auto_leave`, `role`, `observed_term`, nullable `leader_hint` |
| 0 | `proposal_accepted` | `term`, `index`, `entry_digest` |
| 0 | `applied_operator_frontier` | identities, `client_id`, `stream_id`, `next_sequence`, applied index/term, `observed_term`, `role`, nullable `last_applied` batch binding. Leader only; reserves nothing. |
| 0/4/5 | `operator_observation` | One exact SDK reply, staged as separate `admission`/`commit`/`apply`/`execution` fields plus `disposition` and `reply_hex` |
| 0 | `checkpoint_requested` | The node accepted the request. The reply identifies the checkpoint selected at that moment. Use `query-selected-checkpoint` to check when the new checkpoint is ready. |
| 4 | `checkpoint_request_busy` / `checkpoint_request_refused` | `reason.kind`: busy is `snapshot_installing`, `capture_in_progress` or `request_pending`; refused is `owner_fatal`, `owner_draining`, `recovery_pending`, `projector_rebuilding`, `checkpoints_disabled` (no `checkpoint_interval_raft_indices` configured) or `selected_covers_applied`; nullable `checkpoint` selection |
| 0 | `selected_checkpoint` | identities, role, applied frontier, root generation, nullable `selected` base (`raft_index`/`raft_term`, `core_version`, `engine_tx_seq`, `engine_applied_evidence`, `operator_next_sequence`, `operator_last_applied`), `capture_requested`, `progress` |
| 0 | `wal_retention` | `status`: one node's inspection; see [WAL retention and cleanup](#wal-retention-and-cleanup) |
| 0/2/3/4 | `restart_check` | every member's stop or return conditions; see [Restart checks](#restart-checks) |
| 4 | `wal_retention_unavailable` | the inspection could not be taken; typed `reason` |
| 0/4/5 | `wal_cleanup` | one node: `policy`, `role_policy`, `inapplicable_settings`, `decision`, `inspection`, `cleanup_sent`, and after a request `cleanup`, `inspection_after` and nullable `threshold_after` |
| 0/2/3/4/5 | `wal_cleanup_run` | every listed member: `policy`, `endpoints`, nullable `leader`, `cleanup_sent`, `summary` and one `members` entry per endpoint; an unusable endpoint file has `input` instead |
| 0 | `core_residency` | `instance`, `generation`, limits, retained/reserved/remaining charges, nullable `in_flight` job, `rows`, nullable `next_cursor` |
| 0 | `core_prepared` | `instance`, `generation`, exact `package` |
| 4 / 5 | `core_preparation_refused` / `core_preparation_unknown` | the accepted job is not cancelled on unknown |
| 4 | `core_resource_unavailable` | `reason`: `not_configured` on a node without a resident Core owner; `busy` or `closed` for `prepare-core`; `stale_view` or `page_limit` for `query-core-residency`; `page_limit` for `query-provisioned-core`; `busy` or `thread` for `verify-core-package` |
| 0 | `provisioned_core` | `check_level: "declared_metadata"`, `rows`, nullable `next_after` |
| 0 | `core_present` | `check_level: "presence"`: binding, manifest and declared length only |
| 0 | `core_package_verified` | `check_level: "stored_bytes"`: package and artifact digests |
| 4 / 5 | `core_package_refused` / `core_verification_unknown` | a timed-out check keeps its slot until it finishes |
| 0 | `active_core` | `lifecycle` (identities, role, `frontier`, `active_version`, `initialized`, counts) and `local` (`resident`, `not_resident` or `not_configured`) |
| 0 | `core_activations` / `required_core` | `lifecycle` with `rows`, nullable `next_after`. A required row has `initial`, `activated_at` and `rejected`. |
| 4 | `core_lifecycle_stale` | the continuation's frontier changed; `observed_frontier`; restart the traversal |
| 4 | `core_lifecycle_busy` / `core_lifecycle_refused` | typed `reason`; busy is another held view or a snapshot install |
| 0/4/5 | `required_core_checked` | `check_level`, the checked `cluster_identity`/`genesis_identity`/`execution_lineage`, `frontier`, `complete` (true only when every required V passed at an unchanged frontier), counts, up to 16 failing `rows` |
| 0 | `records` | `observation` (`current_term`, `applied_index`, `applied_term`, `committed_index`, nullable `leader_hint`, `role`), `applied_tx_seq`, `tx_chain_hash`, and one `records` entry per key in order: `null`, or `kind`, `sys_id`, `revision`, `data_hex` |
| 0 | `observations` | `page`: `format` `statevec.observations`, `version` 1, `process` scope, `collected_from_ms`/`collected_until_ms`, `rows` tagged by `group`, nullable `next_offset`. Unreadable values are `{"unavailable": CAUSE}`. The host repository's `docs/cluster-next/observations.md` is the field catalog, including the node's optional HTTP `/metrics` endpoint (`metrics_listen_address`) that samples the same groups. |
| 4 | `observations_unavailable` | `request`, `cause`: `invalid_request`, `size_limit` or `panicked` |
| 4 | `observations_unsupported` | `schema`: the page `version` this `sv-admin` does not read; none of its fields is rendered. A page of another `format` is `outcome_unknown` (exit 5) with cause `unsupported_observation_format`. |
| 4 | `records_busy` / `records_refused` | `request`, typed `refusal.reason`, nullable `observation`; see [Record lookup](#record-lookup) |
| 0 | `pending_lifecycle` | leader-only operator-stream batches in the log beyond applied, each `proposed` or `committed_awaiting_apply`; empty means none in the log. A Submit the owner still holds before admission is not listed yet. |
| 0/4 | `lifecycle_intent` | `publication` (`created` or `adopted`), `matches_request` (exit 4 when false), `intent` summary; nothing was sent |
| 0/4/5 | `lifecycle_outcome` | `outcome`, `submitted`, `submission` and last `observation`, `known` (the furthest admission/commit/apply evidence this command saw, never lowered by a later read), `intent` summary. Only `outcome: "succeeded"` is a successful lifecycle operation. |
| 3 | `intent_unavailable` | the saved intent could not be read, checked or synced; nothing was sent |
| 2/3/4 | `lifecycle_preflight` | `submitted: false`, `admission: "not_sent"`, `prior_outcome: "unobserved"`, stored intent/target/package and bounded member issues. Invalid input is 2, unavailable observations 3, observed disagreement or missing preparation 4. No Submit was sent by this invocation; read-only queries may have been sent. |
| 0/4/5 | `collection` | echoed `target` beside the actual `observation` (`null` when the node did not answer; `contradictions` between its replies; `missing` required reads: membership, active view, residency, and a leader's operator frontier) and per-step categories |
| 0/5 | `support` | `complete`, `directory`, `files`, `omitted`, `endpoint` (`answered`, `not_configured`, `no_reply`, or the stop with its `reason`) and `current_cause`; 0 only for a complete collection |
| 0/4/5 | `leader_transfer` | `outcome`: `transferred` (0), `refused` (4) or `indeterminate` (5) with `cause`; a `--for-source-stop` result is the input of `check-restart --handover-report` |
| 3 | No `category` field | `transfer-leader` reports `outcome: "invalid_input"` when the members file cannot be read or validated, or does not list the source or target. Check `cause` and correct the file before retrying. |
| 0/4 | `remaining_spares` / `spares_unknown` | the leader's `peer_id`, `retired` and `remaining` (`count`, `voter_capable`, `learner_only`); unknown has `reason` and `remaining: null` |
| 0 | `alert_rules_rendered` | offline: `schema` and per `clusters` entry `cluster`, `members`, `annotations` and `missing_inputs` (`input`, and `member` for a per-member input) |
| 0/4/5 | `collection_summary` | offline: `complete`, per-member state (`observed`, `partial` with `missing`, `unreachable`, `missing`), `foreign` files with `reason`, `membership_agrees` (every member applied the same voters, learners, joint sets and `auto_leave`, over exactly the reference peers), `frontiers_differ`, `prepared`, `checkpoint_covered`, same-version/different-package rows. Exit 0 needs a complete batch with no negative answer and a known previous attempt; a known disagreement is 4; incomplete or `null` coverage is 5. |
| 4 | `owner_refusal` | `admission: "not_admitted"` for intents only, exact `request`, typed `reason` |
| 4 | `compatibility_refused` | `admission: "not_admitted"`, `stage: "receive"`, with the frozen reply under `cause.refusal`. The node refused this exchange before dispatch. |
| 4 | `queue_full` / `queue_closed` | as above, without `reason` |
| 5 | `owner_reply_unknown` | see above |
| 5 | `outcome_unknown` | `admission: "unknown"`, `stage: "send"` or `"receive"`, typed `cause` |
| 2 | `invalid_input` | `admission: "not_sent"`, typed `cause` string |
| 3 | `io_failure` | `admission: "not_sent"`, `stage`, typed `cause` |
| 0 | `help` | `usage` |

Submit's endpoint file is JSON `{"format":"statevec.members","version":1,"members":[{"peer_id":"1",
"admin_ip":"127.0.0.1","admin_port":7401}]}`, kept on the operator host that
runs the tool; no node reads it. It must cover exactly the live applied
membership, including learners and joint roles; the leader entry uses the
same parsed address as the CLI's IP/port pair. Limits are 64 KiB, 32 unique
peers/endpoints and 32 resident rows per member. Each member must have the
exact target/package resident with admission open; spare load capacity is not required. Initialize
checks V0 (`initial_version`); obtain its digest from the V0 resident row or
from `query-active-core.local.package` when that package is V0. An Update
checks the target encoded in the file, including RevertPrevious.

Preflight observes identities, applied membership and process/generation
stability. No read is retried; each shares the common budget and is also
bounded by `--timeout-ms`. Recovery, snapshot installation, rebuild and pending
queries can return transient unavailability. A preparation job advancing
generation can refuse even if unrelated to the target. The tool does not
prepare, reserve or guarantee that nothing changes after the last read.
An offline collection is not an authorization token. Cross-host commands
connect directly to the full TCP admin endpoint. The TCP tools reject member
files that name a socket, `--socket` and `admin_socket_path`.
Unavailable members must be recovered or removed through the supported
procedure, not bypassed.

Reconcile accepts neither new option and works independently of preparation.
A failed preflight cannot resolve an earlier unknown Submit or authorize
replacement bytes. `lifecycle_outcome.outcome` is one of:

| `outcome` | Exit | Meaning |
| --- | --- | --- |
| `succeeded` | 0 | Applied, and every member's system and business codes are zero. |
| `rejected` | 4 | A committed callback rejection, with the member codes. |
| `conflict` | 4 | Another intent holds this sequence. |
| `not_admitted` | 4 | No record at this read and the sequence is still next. This does not prove that an earlier Submit of the file will not run: the owner may still hold one, sent by this command, an earlier one or a caller before its restart. Submit the same file again, never a different intent. After an unknown Submit, `submit-lifecycle` keeps reading instead. |
| `stream_gap` | 4 | The Submit is beyond the stream's next sequence. |
| `refused` | 4 | The Submit was refused before proposal, for example `core_not_prepared`. A refused Reconcile read never settles; reading continues. |
| `not_sent` | 3 / 4 | The Submit never reached the owner: 3 when this Submit was not transmitted (connect/setup, possibly after preflight reads), 4 when the node refused it first (full or closed queue, typed rejection such as `not_leader`). |
| `result_unavailable` / `processed_result_unavailable` | 5 | Applied or covered, but the result is gone. This is not success. |
| `unsettled` | 5 | The wait ended first; the saved intent is unchanged. |

Only queries and admin views omit `admission`. Membership changes and operator
batches carry `admission`: `not_sent`, `not_admitted`, `known` or `unknown`.
`role` is `follower`, `pre_candidate`, `candidate` or `leader`, and is only an
observation. `request` echoes the operation as `operation` (snake_case command
name) with its identifying fields. A mismatched reply is never a refusal
receipt: it is `outcome_unknown` with cause `response_mismatch`.

Each admin connection carries one request. A verified compatibility reply
reports `operation_unsupported` when its protocol range includes the sent
protocol and `protocol_unsupported` when that range excludes it. A contradictory
range or reason yields `response_mismatch`. A known refusal applies to this
transmission alone. Reconcile any earlier uncertain Submit with its saved intent.

Input causes are `too_many_arguments`, `unknown_argument`, `duplicate_argument`,
`missing_value`, `missing_endpoint`, `invalid_endpoint`, `missing_command`,
`peer_required`, `unexpected_peer`, `invalid_peer`, `expected_peer_required`,
`unexpected_expected_peer`, `invalid_joint`, `unexpected_joint`, `invalid_timeout`,
`frame_required`, `unexpected_frame`, `invalid_frame`, `invalid_package`,
`unexpected_package`, `invalid_page`, `unexpected_page`, `intent_required`,
`unexpected_intent`, `invalid_intent`, `invalid_collection`,
`unexpected_collection`, `record_key_required`, `invalid_record_key`,
`preflight_required`, `invalid_preflight`, `unexpected_preflight`,
`invalid_cleanup_policy`, `missing_cluster_identity` and
`invalid_cluster_identity`, `alert_rules_path_required`,
`invalid_alert_rules_path`, `support_path_required`, `invalid_support_path`,
`invalid_support_identity`, `support_output_in_data_root`,
`support_output_in_checkpoint_root`, `invalid_restart_check` and
`invalid_spare_inventory`.
Building a fresh intent can also refuse with
`session_entropy`, `lifecycle_command` or `intent_too_large`; nothing is
created or sent.

Owner `reason.kind` preserves the closed native categories: `ready_pending`,
`apply_pending`, `recovery_pending`, `operator_frontier_missing`,
`snapshot_maintenance_busy`, `owner_fatal`, `owner_draining`, `not_leader`,
`leader_not_current`, `leader_not_applied` (string `committed`/`applied`),
`not_serving`, `proposal_busy`, `pending_configuration`, `promotion_unavailable`,
`membership_invalid_peer` or `membership_duplicate_peer` (string `peer`),
`membership_shape_invalid`, `membership_quorum_unavailable`,
`proposal_entry_too_large` (u32 integer
`observed`/`limit`), `pending_scan_limited`, `pending_scan_unavailable`,
`raw_node` and `membership_validation`. Owner diagnostics are bounded to 512
bytes so a refusal always fits its reply. Rendered private diagnostic strings
are omitted, never parsed as a classification.

Transport cause `kind: "io"` includes `io_kind` (the typed Rust ErrorKind name)
and nullable integer `os_code`. OS codes are platform-specific; never interpret
an errno as an owner's refusal. Invalid replies have `frame_header`,
`frame_too_large` (integer `observed`/`limit`), `json` (class
`io`/`syntax`/`data`/`eof`), or `response_mismatch`. Paths, raw inputs and raw I/O
messages are not echoed.

The node reads each whole request, and writes each whole reply, within its
own five-second deadline (one second for an observation page's reply), so a
slow client cannot hold the admin connection indefinitely.

## Compare CmdOnly replicas

Use CrossCheck for RC1 design-partner trials. In CmdOnly, each replica
executes committed commands and keeps its own result. A nondeterministic
Core can make two voters answer the same direct read differently. An
operator can compare the transaction-chain hashes manually.

Collect both replies from each member's own admin endpoint. Use the member
addresses from the deployment inventory. For example:

```sh
set -e
for ip in "$VOTER_A_IP" "$VOTER_B_IP"; do
  sv-admin --admin-ip "$ip" --admin-port 19123 query-selected-checkpoint > "$ip.identity.json"
  sv-admin --admin-ip "$ip" --admin-port 19123 query-membership > "$ip.membership.json"
done
jq '{peer_id} + (.checkpoint | {cluster_identity, genesis_identity, execution_lineage})' \
  "$VOTER_A_IP.identity.json" "$VOTER_B_IP.identity.json"
jq '{peer_id, applied_index, applied_term, engine_tx_seq, tx_chain_hash}' \
  "$VOTER_A_IP.membership.json" "$VOTER_B_IP.membership.json"
```

1. Verify the expected peer ID on both replies from each endpoint.
   The cluster identity, genesis identity and execution lineage must all
   be available and equal between members.
2. Both membership replies must report `applied_membership`. Compare
   `applied_index`, `applied_term` and `engine_tx_seq`. If any value is
   missing or any pair differs, the comparison is inconclusive. Let the
   slower member catch up, then sample both again.
3. At the same position, compare the complete `tx_chain_hash` strings.
   A missing hash makes the comparison inconclusive. Unequal hashes
   establish a transaction-chain mismatch at that position. Equal hashes
   show agreement of the sampled transaction chains.

These rows come from a three-voter CmdOnly run with one Bank follower
whose balance rule differed under the same declared Core version.
Both followers reported the same history and execution lineage.

| Sample | Peer | Applied index | Applied term | Engine tx seq | Transaction-chain hash |
| --- | ---: | ---: | ---: | ---: | --- |
| Earlier | 2 | 3 | 1 | 2 | `54f9591aa20a407bbd68482cb251951535f096aec6984fad7978d282af345d0e` |
| After the command | 3 | 4 | 1 | 3 | `820d18989f7e31ae2a541957bca22ad1b57b4214c6a962e6971a4f07eb73d2d7` |
| After the command | 2 | 4 | 1 | 3 | `76a8f3ce11ebca47dae1375fb9289ee5ad5c45ce1a4e030943be21beccc43d24` |

Peer 2's earlier sample and peer 3's later sample are **inconclusive**.
The two samples at `(4, 1, 3)` show a **mismatch**. Preserve the replies,
the member builds and the exact Core packages for diagnosis.

## Record lookup

`query-records` reads current business records from the selected node,
for operators and monitoring agents. When admin is enabled it needs no extra
setting: the leader, followers and learners all answer it, whatever their
`client_reads` setting. It is an operator interface, not a client read
API; applications read through the SDK read client or a Projector.

Each key is `--sys-id KIND:SYS_ID` (both decimal) or `--unique-key
KIND:UK_ID:KEY_HEX`, where `KEY_HEX` is the encoded unique key, at most 1024
bytes. A request holds 1 to 16 keys. The answer is the node's records at its
applied entry: every record comes from one engine state, stated by
`observation` and the `applied_tx_seq`/`tx_chain_hash` frontier, with one
entry per key in request order. It is not linearizable and proves no commit:
compare `applied_index` with `committed_index` to see how far the node lags.
Record `data_hex` is the record's bytes; decode them with the application's
schema.

Encode `KEY_HEX` using the schema's key encoder. For example, the Bank Core
stores accounts under kind 1, key 0. Account 18 is looked up with
`--unique-key 1:0:0000000000000012`. The key and the bytes returned in
`data_hex` use different encodings.

A node holds one lookup at a time; another one at the same moment is
`records_busy` (`refusal.reason: "busy"`). The answer's records are bounded at
3072 bytes; a larger one is refused `reply_too_large`. The other
`refusal.reason` values are `recovery_pending` (initial replay has not
finished), `snapshot_installing`, `projector_rebuilding` (a learner's
latest-state rebuild is not acknowledged), `not_yet_visible` (no entry applied
yet, with `applied_tx_seq`), `unknown_kind`, `unknown_unique_key`,
`draining`, `fatal`, `request_too_large` and `unavailable`. A refusal is a known answer for this request only; run the
command again for a later state. The tool never retries or caches a lookup.

## WAL retention and cleanup

`query-wal-retention` reads one node's retention facts. `cleanup-wal` inspects,
decides under the tool's policy and sends at most one bounded request, bound
to the identities of that inspection. Neither is retried, and a later
invocation always inspects again. The host repository's
`docs/cluster-next/wal-cleanup.md` describes the policy and procedures.

`wal_retention.status` has the node's `peer_id`, `role`, `membership_role`
(`voter` or `learner`), `term`, `root_generation`, `storage_generation`, the
nullable `selected_index`/`selected_term` of the selected checkpoint and the
cleanup `target` bound to it, `durable_commit`, the retained
`first_index`/`last_index`, `sealed_segments`, `sealed_bytes`, `retention`,
`projector`, the owner's nullable `refusal`, and `filesystem`. `peers` is the
leader's sample of every member's `matched` index and is `null` elsewhere.

Single-member `cleanup-wal --admin-port PORT` reports category `wal_cleanup`
for that node.
`decision.kind` is `request` with the deciding `rule` (`peer_wait`,
`follower_default`, `leader_sample`, `low_voter_space`, `learner_eligible`,
`low_learner_space` or `manual`), or `deferred` with a typed `cause`. A
follower's report also has its applied `membership`. Exit 0 is a completed
cleanup, which can retire nothing. Exit 4 is a deferral, an owner refusal, a
failure or a threshold still unmet afterwards. Exit 5 is a request whose
terminal result was not observed.

`cleanup-wal --members-file PATH --cluster-identity HEX` takes the Submit
endpoint file described above and the verified deployment's cluster identity.
The latter is required: omitted or malformed values return exit 2 before
network access. It takes no `--admin-ip`, `--admin-port`, `--manual` or
`--min-voter-space`. It reads each listed member's applied membership, then
the leader's peer sample and all other inspections before sending any cleanup.
Every selected target must match the expected cluster; a member without a
selected target cannot be cleaned. It sends each member at most one cleanup
under that member's rule. Its `wal_cleanup_run`
object has:

- `endpoints`: `match`, or `mismatch` when a reachable member applied other
  members than the listed peer IDs, an endpoint answered as another peer, or
  a selected target belongs to another cluster (`foreign_cluster`). A
  mismatch sends no cleanup to anyone. `match` does not certify IP addresses
  against genesis; admin endpoints are deployment inputs. Unavailable members
  remain unavailable rather than being certified by that run-level label.
- `cluster_identity`: the expected identity supplied by the operator.
- `leader`: `peer_id` and `term` of the member whose peer sample was used, or
  `null`.
- `cleanup_sent`: whether any cleanup request left the tool.
- `summary`: the number of members per `outcome`.
- `members`: per endpoint, `peer_id`, `outcome` and, once inspected, `role`,
  `membership_role`, `selected_index`, `sealed_segments` and `decision`. A
  sent request adds `cleanup` (`completed` with `stop` and `progress`,
  `refused` or `failed` with `cause`, or the unobserved result). A member that
  could not be read has `read` and `cause` instead. Peer lists hold at most
  eight names beside their exact `count`.

| `outcome` | Exit | Meaning |
| --- | --- | --- |
| `completed` | 0 | The owner completed the request; `progress` has the retired segments and bytes. |
| `deferred` | 0 | The tool or the owner's inspection chose not to request cleanup now. This is the ordinary result of a period with nothing to release. |
| `unavailable` | 4 | A read failed or was refused, or the member's owner reports `owner_fatal`; nothing was sent to this member. |
| `refused` / `failed` | 4 | The owner refused the request, or a cleanup step failed with its typed cause. |
| `threshold_unmet` | 4 | Cleanup completed and the member's space threshold is still unmet or unobserved. |
| `endpoint_mismatch` / `not_sent` | 4 | The member disagrees with the endpoint file, or agrees while another member does not. |
| `unknown` | 5 | The cleanup was sent and no terminal result was observed. It is not cancelled and is never resent. |

The process exits with the most severe member outcome. It exits 3 when no
request reached any node or the endpoint file could not be read, and 2 for
invalid options or an invalid file; `input.cause` then names the file error.
As for every command, exit 6 means the output failed after a request was
sent; treat every member's outcome as unknown.

A run decides on the memberships it read at its start, and a member it
reported `unknown` finishes the accepted cleanup after the run has exited. To
add a member, list it in the endpoint file first: from then on a run reports
`mismatch` and sends nothing until every reachable member has applied the
addition. Then wait for every run started before the edit to exit, at most
four times the member count times `--timeout-ms`, and for each member such a
run reported `unknown`, or every member after exit 6, until its
`query-wal-retention` reports no `maintenance_busy` refusal. Then send
`add-learner`. To remove a member, remove its entry only after every
remaining member has applied the removal.

When the run is scheduled, watch three conditions in its reports: a nonzero
exit; a leader whose `decision` defers with `peer_lagging` and names the same
peers in consecutive runs, which keeps the segments those members still need;
and a member whose `sealed_segments` grows across runs while it defers. A
learner defers with `no_checkpoint` while its Projector is behind, because it
captures no checkpoint until the Projector has taken every delivered frame.
Watch the report's age as well: no new report for two periods means the
schedule is not running, and nothing reclaims until it does.

A node whose WAL filesystem fills up stops with exit code 5 and a typed
`StorageFull` write cause at its next segment publication; a restart repeats
the failure. Return space outside the node's data root or extend the volume,
reopen the same root as `existing`, then request cleanup. The host
repository's `docs/cluster-next/wal-cleanup.md` has the procedure and the
space budget to keep.

## Render alert rules

`render-alert-rules` renders the shipped alert rule template with one
deployment inputs file into one new rules file. It reads no node and sends
nothing. Both paths are absolute. The output is created and never replaces an
existing file; a write that fails removes the file it created.

Exit 0 is `alert_rules_rendered`. Each input left `null` or out is listed in
`missing_inputs` and renders as a series that raises the prerequisite alert;
render again once it is known. Exit 2 is `invalid_input` with `cause`
`invalid_alert_inputs` and `input`: `kind` `alert_inputs` and a `reason`, one
of `too_large`, `malformed` (with serde's `line` and `column`),
`unsupported_schema`, `invalid_window`, `no_clusters`, `too_many_clusters`,
`invalid_cluster`, `duplicate_cluster`, `no_members`, `too_many_members`,
`invalid_member`, `duplicate_member`, `invalid_declared_voters`,
`invalid_startup_limit`, `invalid_cleanup`, `invalid_threshold`,
`invalid_maintenance_max`, `invalid_mount_point`, `too_many_annotations`,
`invalid_annotation` or `duplicate_annotation`. No reason echoes input text.
Exit 3 is `io_failure` with `stage` `inputs` or `output`; an existing output
is `AlreadyExists`. The host repository's `docs/cluster-next/alerts.md`
defines the inputs schema.

## Support collection

`support` collects one node's support data into a new private directory
(mode 0700, files 0600). It never reuses an existing directory and never
opens the data root, a WAL or checkpoint file, a Core artifact or a plugin's
own configuration. It sends only read-only queries.

- `config.json` records allowlisted fields of the node configuration file
  (identities, addresses, paths and limits). Other fields are counted in
  `unlisted_fields`, never copied. Support checks the `statevec.node-config` format and
  version before using any settings. A missing or different format records
  `unsupported_format`. An unsupported 16-bit version records
  `unsupported_version`. In either case support keeps the local diagnostics,
  skips online collection even with an explicit endpoint, and exits 5.
- `diagnostics.json`: the structured terminal diagnostics found in the
  service log `--log-file` and its rotated predecessors `PATH.1` to `PATH.4`,
  newest file first, reading at most 16 MiB from the end. Other lines are
  counted, never copied. Each file has a status: `read` (with
  `started_mid_record`, `incomplete_last_record` and `changed_during_read`),
  `absent`, `symlink`, `not_regular_file`, an I/O cause, or `input_limit`.
  A symlink is never followed. The rendered `detail` of each record is left
  out.
- With `--latest-start`, `diagnostics.json` also records `panics` from the
  local system journal for that invocation. Each row gives the thread, Rust
  source location and `time_us` in microseconds since the Unix epoch.
  Support reads stdout and stderr newest first, up to 1024 journal entries or
  512 KiB of JSON, then reports the complete records in chronological order.
  It redacts application-defined thread names, absolute build-directory
  prefixes and unconventional source paths. The `journal` field records a
  limit or an unavailable source: a missing command,
  `insufficient_permissions` when the account cannot read the system
  journal, another failed read with its exit status, an elapsed collection
  deadline, or `empty` when the invocation has no readable entries. The other support sources
  determine the collection exit code and `current_cause`. If the output
  budget fills, the existing diagnostics keep their priority. The manifest
  records `sources.journal.diagnostics_omitted` as `output_limit` when the
  journal detail cannot fit, and lists a skipped raw excerpt in `omitted`.
  The collecting account needs journal access, normally through the
  `systemd-journal` or `adm` group. Save the failed invocation ID before
  restart to read that invocation while persistent journal records remain.
- The same file reports the node's `crash_dump_collector` and
  `panic_text_sink` startup checks. `start_checks_source` and
  `start_checks_process` identify the live diagnostics page or the selected
  invocation's terminal record. An earlier boot's recorded checks retain
  that boot ID. Unavailable readings are null.
- `observations-GROUP.json` for every observation group and `packages.json`
  (Core residency, provisioned and active Core pages), when the admin endpoint
  answers. Explicit `--admin-port` with optional `--admin-ip` overrides the
  local configuration's `admin_listen_ip`/`admin_listen_port`. A wildcard
  listen address needs an explicit concrete destination. Local config/log
  collection stays local even when queries use a remote endpoint.
  Without that explicit destination, `explicit_endpoint_required` preserves
  the configured listen address. Invalid address fields are reported as
  `invalid_configuration`, and unreadable config as `configuration_unavailable`;
  only absent admin configuration is `not_configured`. These incomplete
  bundles still collect local files and exit 5. The manifest's
  `inputs.endpoint_source` distinguishes an argument from local configuration.
  Online collection requires `--cluster-identity` from the verified
  deployment; without it, `cluster_identity_required` leaves a partial
  local-only bundle. The first Node observation must match that identity and
  the configured peer ID. Later observation pages must have that same process
  scope; package reads are bracketed by Node observations of that process.
  An unreachable endpoint or identity mismatch stops later queries; foreign
  rows are not kept. A retired `admin_socket_path` is invalid configuration,
  even though the collector can still recover the local configuration/logs.
- `manifest.json`: the tool's build, times, inputs, host boot ID, limits,
  source statuses, `current_cause`, the files with their SHA-256 and the
  omitted items. `--release-manifest` adds `declared_digests`, copied from a
  `{"format": "statevec.release-manifest", "version": 1,
  "digests": {NAME: SHA256_HEX}}` file and labelled as
  declared: nothing is computed over a binary or package to fill them.

`current_cause` is `attributed` only for a record whose process is the node's
latest start: its start identity equals `--latest-start` (systemd's
`INVOCATION_ID` of the unit's latest start, from `systemctl show -p
InvocationID --value UNIT`; an empty value is no latest start), its peer is
the configured peer, its boot ID is the host's current one and, when the
record has a cluster identity, it equals `--cluster-identity` (the
`cluster_identity` of any running member's `node` observations; with none
running, take it from an earlier bundle's `observations-node.json`). A start
identity is 1–64 ASCII letters, digits, `-`, `_` or `.`, the identities the
node records. An attributed record keeps its `exit_code` and `outcome`,
which the exit code decides: `stopped` (exit 0 without a failure),
`returned_projector_custody` (exit 7, a requested stop that returned the
Projector's unsent frame for the next start to replay; not a fault) or
`failed`. The record's log level follows the outcome: INFO, WARN or ERROR.
It is written at any configured level except `off`. A panic that reaches the
node's entry is `failed` with exit 101 and a `panic` cause. A local I/O cause
carries `io.class`, from its OS error code: `space`, `media` (the device or
filesystem failed, became read-only or reports damage), `resource`
(descriptors, memory, locks, a file size limit),
`permission` or `other`. `storage_cause` is then true for a required-WAL or
selected-checkpoint cause, or a local I/O cause of class `media`.
`storage_full` is true for class `space`, a write the filesystem refused for
space or quota: that is not damage, and the same root reopens once space
returns. A `resource` or `permission` cause says nothing about the files.
Every other record is history with the failed check named, and the cause stays
`unknown` with a `reason`. `history_storage_causes` counts the storage causes
kept as history; they do not choose a procedure. Terminal reporting is best
effort: a SIGKILL, a full log queue or a failed sink leaves the cause unknown.

`--include-raw-logs` adds `raw-log-excerpt.txt`, at most 1 MiB of complete
lines from the newest readable service log, and keeps each terminal record's
`detail`. When the selected invocation has journal entries it also writes
their messages and stack frames to `raw-journal-excerpt.txt`. These files
can contain plugin and business text, as marked in the manifest's `sensitivity`.

Limits: 30 s in total, 32 files, 16 MiB of output, 1 MiB per file and 64
pages per group. A budget that stops a source records it in that file or in
`omitted`. Exit 0 is a complete collection. Exit 5 is a partial one, such as
a stopped node, or a log longer than the 16 MiB read from its end, as after
the first rotation; it keeps every file written. The manifest's `endpoint` is
`answered` when some query returned the node's reply, and `no_reply` when
every exchange failed or timed out, as with a stopped process. Exit 2
`support_output_in_data_root` refuses an `--output` at or below the
configuration's `data_root`, and `support_output_in_checkpoint_root` one at
or below its `checkpoint_root`, through symbolic links too, before anything
is written. Exit 3 is `io_failure` at stage
`output`: the directory exists already or could not be written;
`written_files` counts the files it holds.

## Restart checks

Both `check-restart` forms read the `node` page between two Core residency
reads on each member. A successful sample includes `process_observation`
with the OS process scope and build metadata. Its `executable_sha256` gives
the running executable's hash when available. A failed node-page read marks
that member unavailable.

`check-restart` takes the Submit endpoint file described above and no
single-member endpoint options. Both forms require `--cluster-identity HEX`
from trusted deployment inventory; it is rendered as `expected_cluster_identity`.
Each member must belong to that cluster, not merely agree with the answering
leader. A mismatch reports `foreign_cluster` and its `observed` identity.
`--returned` refuses a `foreign_stop_report` before network access (exit 4).
It only reads: per member, the Core residency
page, applied membership, WAL retention, selected checkpoint and, on a member claiming
leadership, pending lifecycle, then the node page and residency again. Both
residency readings must name the same `instance` and `generation`. It sends no write,
keeps no state and never stops or starts a node. The host repository's
`docs/cluster-next/restart.md` gives the procedure around it.

`--stop PEER` decides whether that member can be stopped now: stable
membership equal to the file, one leader at the highest term that every
member names, no pending lifecycle operation, every member durably committed
and applied through the leader's durable commit taken once as `barrier`, the
leader's sample at or above it, every Projector serving, and `PEER` not the
leader unless `--leader` is given. A multi-voter cluster must retain a
majority after the stop; `quorum_unavailable` reports `remaining` and
`required`. Learners do not count, and `--leader` cannot bypass this check.
A single voter instead requires `--leader` for its planned outage.
`--returned PEER --stop-report FILE`
takes a passing `--stop` report of the same member back and decides whether
it has returned: the shared membership, binding, identity and leader
conditions, a process instance other than the recorded one, and a `barrier`
above both the recorded last log index and the new instance's first observed
last log index (`first_last_index`). The first instance and its index remain
fixed; a restart after that first valid sample ends this invocation with
`process_changed`.
`PEER` must durably commit, apply and match through it. Its Projector must
serve and, for continuous history, consume through the barrier.
If the leader commit does not exceed both fixed log indices,
the member reports `waiting_for_commit`.

`--wait-ms` (at most 86400000) sets one deadline. `--stop` defaults to 0,
one sample, which can report a member behind under continuous writes.
`--returned` defaults to 600000 and rejects values at or below 250 ms.
Its first sample fixes the new instance and index; later samples must
observe replication beyond that index. Allow time for the exchanges too.
Keep application submissions active during an otherwise idle return wait.
Samples repeat
every 250 ms until every condition holds or the next one would start at the
deadline; each sample completes. `--timeout-ms` bounds each exchange.

The `restart_check` object has `mode` (`stop` or `returned`), `target`
(`peer_id`), `outcome` (`ready`, `unmet` or `endpoint_mismatch`), `samples`,
`endpoints`, the nullable `cluster` condition (`no_leader`,
`leader_conflict`), `identities`, `leader` (`peer_id`, `term`), the nullable
`barrier`, `unmet` (exact `count`, at most eight `peers`) and `members`.
Each member entry has `peer_id` and, once read, `role`, `membership_role`,
`instance`, `generation`, `term`, `applied_index`, `durable_commit`,
`last_index`, the leader's nullable `matched`, and `projector`.
Every member has `filesystem`: available/total bytes or a typed unavailable
cause. If the sample fails, its cause is `member_sample` with the exact
`reason`; partial bytes from an unbound sample are not reported as current.
The command sets no disk-headroom threshold. `unmet` is
`null` or the first unmet condition with its observed values. With
`endpoints: "mismatch"` the report has only `members`, each with `peer_id`
and `filesystem` plus `unmet`: `null`, `membership_mismatch` (the member's
`applied` index and
term, and the file's `missing_endpoints` and `extra_endpoints`), or the read
failure. A passing
`--stop` adds `stop_record`; `--returned` adds `returned`
(`recorded_instance`, `instance`, `first_instance`, `first_last_index`, `remaining_loads`,
`remaining_artifact_bytes`). An input failure has `input` with a typed
`kind`: `members_file`, `peer_not_listed`, `stop_report` (with `cause`),
`stop_report_missing_build_major`, `foreign_stop_report`,
`stop_report_stale`, or for `--handover-report` `invalid_handover_report` or
`handover_target_mismatch`.

| Exit | Meaning |
| --- | --- |
| 0 | Every condition held. It is an observation, not a reservation. |
| 4 | A condition was unmet at the deadline, a member was unreachable or refused a read, the file differs from the applied membership, or the stop report is about other members. Nothing was sent for a stale report. |
| 3 | No request reached any node, or the file or stop report could not be read. |
| 2 | Invalid options, an invalid file, a peer the file does not list, or a stop report that is not a passing `--stop` report for that peer. |

## Saved intents and collections

Before switching development nodes, SDKs and admin tools to the shared CRC
header, reconcile every outstanding saved intent with the previous build.
The new tool checks the stored Submit before it connects. If the CRC fails,
it reports `intent_unavailable` with `cause.kind = frame_checksum` and keeps
the original file.

A saved intent file holds one complete SVIN Submit frame. It is published once,
by linking a synced private temporary file to the operator-named path, and is
never replaced. A second caller, or a file already present, is adopted. Every
use reads the stored bytes back and syncs the file and its directory before
any send; a path that is not a regular file is refused without waiting on it.
The file is client recovery input, not a transaction-status record.
Only `succeeded`, `rejected`, `conflict`, `result_unavailable` and
`processed_result_unavailable` resolve it. Every other outcome leaves it
unresolved, in later commands and after a caller restart too, because a
Submit of it sent earlier may still be admitted; exit 4 is no reason to
discard it. Keep an unresolved file, keep at most one unresolved intent per
caller, and resume with `reconcile-lifecycle` or by submitting that same
file, never with other content or another sequence. Never replace a saved
file: each use reads it and then syncs it by reopening the path.

Updates encode `UpgradeMode::InPlace`, or `RevertPrevious` with
`--upgrade-mode revert-previous` and empty input. Submission and
reconciliation retain those exact saved bytes, including mode, source/target
versions and callback input. They do not rewrite a prior-format intent or
choose another migration mode. Only readonly migration, compatible zero-span
field additions and new empty kinds are supported; see
[initialization and migration](core-plugins.md#initialization-and-migration).

`collect` and `combine` replace a network admin surface. Run `collect` on every
voter and learner, keep each JSON file, then run `combine` against the
reference membership. A file whose collection, target, identity, lineage or
peer does not match is reported as foreign and never fills a gap, as is a
file whose replies contradict its own identity or process instance
(`inconsistent_observation`), one that is not a readable v1 collection of at
most 64 KiB (`unreadable`), one with no target peer (`no_target_peer`) and one
for a peer outside the reference (`not_a_reference_member`). A second file for
an observed member (`duplicate_peer`) makes the batch incomplete. A member
without an actual observation in this batch leaves it incomplete (exit 5); so
does a file missing a required read, whose member is `partial`. The
summary certifies that batch only, not current liveness.

A collection summary does not show that a learner can bootstrap from a
snapshot.
