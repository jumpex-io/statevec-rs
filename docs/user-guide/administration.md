# Official administration tool

Use the `statevec-admin` binary from the matching official host distribution.
Applications invoke it as a subprocess; they do not need a private Rust
dependency, an admin protocol encoder, or an SDK-side admin server. This tool
is on the unreleased 0.6.0 paired development line: local builds are not evidence
that an official binary/revision has been published. Keep the host and SDK
revisions pinned together for deployment. The admin wire is release-matched:
a CLI and node from different builds are not supported together.

Detailed operating semantics, including the upgrade procedure, are in the host
repository's `docs/cluster-next/material.md` and `docs/cluster-next/upgrade.md`.
This page is the stable invocation and JSON contract.

## Invocation and authority

Every command except `combine` takes `--socket ABSOLUTE_PATH`, the node-local
admin socket. The endpoint must be enabled in the node configuration. It is the
existing same-user Unix socket, in an operator-owned private directory; there
is no network admin surface. `--help` alone returns a versioned JSON usage
object.

| Group | Commands |
| --- | --- |
| Membership | `query-membership`, `add-learner --peer-id DECIMAL_U64` |
| Operator stream | `query-operator-frontier`, `operator-stream --request-hex SVI0_FRAME_HEX` |
| Checkpoint | `request-checkpoint`, `query-selected-checkpoint` |
| Local material | `query-core-residency [--cursor INSTANCE:GENERATION:START] [--limit 1..=8]`, `prepare-core --core-version HEX --package-digest HEX --target TRIPLE`, `query-provisioned-core [--after V_HEX:PACKAGE_HEX] [--limit 1..=8]`, `check-core-presence --core-version HEX --target TRIPLE`, `verify-core-material --core-version HEX --target TRIPLE` |
| Lifecycle views | `query-active-core`, `query-core-activations [--after TX_SEQ] [--frontier FRONTIER] [--limit 1..=24]`, `query-required-core [--after V_HEX] [--frontier FRONTIER] [--limit 1..=24]`, `check-required-core --target TRIPLE [--stored-bytes]`, `query-pending-lifecycle` |
| Saved intent | `create-lifecycle-intent --intent ABSOLUTE_PATH (--initialize-core \| --source-version HEX --target-version HEX) [--input-hex HEX]`, `submit-lifecycle --intent ABSOLUTE_PATH [--wait-ms 1..=600000]` (the wait bounds the whole reconciliation, including a read in flight), `reconcile-lifecycle --intent ABSOLUTE_PATH [--wait-ms 1..=600000]` |
| Collection | `collect --collection ID --cluster-identity HEX --genesis-identity HEX --peer-id DECIMAL_U64`, `combine --collection ID --cluster-identity HEX --genesis-identity HEX --members PEER[,PEER...] [--core-version HEX --package-digest HEX] --file PATH...` (offline, no `--socket`) |

`TRIPLE` is `x86_64-unknown-linux-gnu` or `aarch64-unknown-linux-gnu`.
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

The tool never creates or removes the socket, follows a leader hint, retries a
request, promotes voters, rewrites topology, changes membership policy, loads a
package as a side effect, or resequences an intent. Learner IDs/endpoints must
already be allowed by the node's installed topology.

`proposal_accepted` means the owner proposed the change. It does **not** mean
the change committed/applied. A later `query-membership` observes the current
applied membership, not a durable receipt for an earlier particular request.
After an unknown outcome, inspect that membership and reconcile before choosing
another explicit operation. Do not automatically replay an uncertain request.

## JSON v1 and exits

Stdout is one JSON object plus newline, at most 32768 bytes. Every object has
integer `version: 1` and string `category`. All u64-valued IDs, indices, terms
and sequences are **decimal strings**, including arrays and refusal fields.
Hashes/digests are lowercase hex. Booleans remain JSON booleans; optional
observations are `null` when absent. No membership/WAL/checkpoint DTO is
published in the SDK.

**Node replies carry `peer_id`**: the answering node's peer ID from its local
configuration, not an owner sample, so saved outputs from several nodes stay
attributable. Every single-request command has it at the top level, and so
does `check-required-core` (all of its replies must come from that one node,
and every lifecycle page must name the first page's cluster, genesis and
lineage).
`collect` records it as `observation.peer_id` beside the operator's echoed
`target.peer_id`. Outputs produced before any reply (input, connect and setup
failures), the intent commands and the offline `combine` have no top-level
`peer_id`.

| Exit | Meaning |
| --- | --- |
| 0 | A known observation or result. It is not upgrade success unless the category says so. |
| 2 | `invalid_input`: nothing was sent. |
| 3 | Not sent: `io_failure` at `setup`/`connect`, an unavailable saved intent, or `unsupported_platform`. Exit 3 always means the request was never sent. |
| 4 | A known refusal or negative answer from the node (for example `owner_refusal`, `queue_full`, a busy or refused view, a stale continuation, a conflict). |
| 5 | Unknown: the outcome of a sent request is not known (`outcome_unknown`, `owner_reply_unknown`, unknown preparation or verification, an unsettled intent, an incomplete collection). |
| 6 | Output failed after the request may have been sent. The outcome is unknown; never infer non-admission. |

**`owner_reply_unknown` (exit 5)**: the node accepted the request, but its own
wait for the Raft owner ended first. It has `request` and `cause`
(`timeout` or `disconnected`). For a membership change or operator batch,
`admission` is `unknown`. Accepted work is not cancelled. It is never a
refusal and never permission to send a different request.

**Exit 6**: writing or flushing stdout failed after a send attempt. Stdout may
be incomplete or empty. An output-size failure after a send emits a small
`output_failure` object with `stage: "output"`, `cause.kind: "output_bound"` and
`admission: "unknown"`. The same failures before any send are exit 3
`io_failure` with `stage: "output"` and `admission: "not_sent"`. Stderr then
carries a one-line note; it is supplementary, not a stable protocol. A consumer
must check both the exit status and complete JSON.

### Categories

| Exit | Category | Notes |
| --- | --- | --- |
| 0 | `applied_membership` | `applied_index`, `applied_term`, nullable `engine_tx_seq`/`tx_chain_hash`, `voters`, `voters_outgoing`, `learners`, `learners_next`, `auto_leave`, `role`, `observed_term`, nullable `leader_hint` |
| 0 | `proposal_accepted` | `term`, `index`, `entry_digest` |
| 0 | `applied_operator_frontier` | identities, `client_id`, `stream_id`, `next_sequence`, applied index/term, `observed_term`, `role`, nullable `last_applied` batch binding. Leader only; reserves nothing. |
| 0/4/5 | `operator_observation` | One exact SDK reply, staged as separate `admission`/`commit`/`apply`/`execution` fields plus `disposition` and `reply_hex` |
| 0 | `checkpoint_requested` | one capture made due on this node, with the unchanged selection sampled in the same owner turn; acceptance is not selection |
| 4 | `checkpoint_request_busy` / `checkpoint_request_refused` | typed `reason` |
| 0 | `selected_checkpoint` | identities, role, applied frontier, root generation, nullable `selected` base (Raft index/term, V, `tx_seq`, evidence, operator `last_applied`), `capture_requested`, `progress` |
| 0 | `core_residency` | `instance`, `generation`, limits, retained/reserved/remaining charges, nullable `in_flight` job, `rows`, nullable `next_cursor` |
| 0 | `core_prepared` | `instance`, `generation`, exact `material` |
| 4 / 5 | `core_preparation_refused` / `core_preparation_unknown` | the accepted job is not cancelled on unknown |
| 4 | `core_resource_unavailable` | typed `reason`, for example `busy`, `stale_view`, `page_limit` |
| 0 | `provisioned_core` | `check_level: "declared_metadata"`, `rows`, nullable `next_after` |
| 0 | `core_present` | `check_level: "presence"`: binding, manifest and declared length only |
| 0 | `core_material_verified` | `check_level: "stored_bytes"`: package and artifact digests |
| 4 / 5 | `core_material_refused` / `core_verification_unknown` | a timed-out check keeps its slot until it finishes |
| 0 | `active_core` | `lifecycle` (identities, role, `frontier`, `active_version`, `initialized`, counts) and `local` (`resident`, `not_resident` or `not_configured`) |
| 0 | `core_activations` / `required_core` | `lifecycle` with `rows`, nullable `next_after`. A required row has `initial`, `activated_at` and `rejected`. |
| 4 | `core_lifecycle_stale` | the continuation's frontier changed; `observed_frontier`; restart the traversal |
| 4 | `core_lifecycle_busy` / `core_lifecycle_refused` | typed `reason` |
| 0/4/5 | `required_core_checked` | `check_level`, the checked `cluster_identity`/`genesis_identity`/`execution_lineage`, `frontier`, `complete` (true only when every required V passed at an unchanged frontier), counts, up to 16 failing `rows` |
| 0 | `pending_lifecycle` | leader-only operator-stream batches beyond applied, each `proposed` or `committed_awaiting_apply`; empty means none pending |
| 0/4 | `lifecycle_intent` | `publication` (`created` or `adopted`), `matches_request` (exit 4 when false), `intent` summary; nothing was sent |
| 0/4/5 | `lifecycle_outcome` | `outcome`, `submitted`, `submission` and last `observation`, `known` (the furthest admission/commit/apply evidence this command saw, never lowered by a later read), `intent` summary. Only `outcome: "succeeded"` is a successful lifecycle operation. |
| 3 | `intent_unavailable` | the saved intent could not be read, checked or synced; nothing was sent |
| 0/4/5 | `collection` | echoed `target` beside the actual `observation` (`null` when the node did not answer; `contradictions` between its replies; `missing` required reads: membership, active view, residency, and a leader's operator frontier) and per-step categories |
| 0/4/5 | `collection_summary` | offline: `complete`, per-member state, `foreign` files, `membership_agrees`, `frontiers_differ`, `prepared`, `checkpoint_covered`, same-version/different-package rows |
| 4 | `owner_refusal` | `admission: "not_admitted"` for intents only, exact `request`, typed `reason` |
| 4 | `queue_full` / `queue_closed` | as above, without `reason` |
| 5 | `owner_reply_unknown` | see above |
| 5 | `outcome_unknown` | `admission: "unknown"`, `stage: "send"` or `"receive"`, typed `cause` |
| 2 | `invalid_input` | `admission: "not_sent"`, typed `cause` string |
| 3 | `io_failure` | `admission: "not_sent"`, `stage`, typed `cause` |
| 0 | `help` | `usage` |

`lifecycle_outcome.outcome` is one of:

| `outcome` | Exit | Meaning |
| --- | --- | --- |
| `succeeded` | 0 | Applied, and every member's system and business codes are zero. |
| `rejected` | 4 | A committed callback rejection, with the member codes. |
| `conflict` | 4 | Another intent holds this sequence. |
| `not_admitted` | 4 | The server has no record and the sequence is still next. |
| `refused` | 4 | Refused before proposal, for example `core_not_prepared`. |
| `not_sent` | 4 | The node refused before reaching the owner. |
| `result_unavailable` / `processed_result_unavailable` | 5 | Applied or covered, but the result is gone. This is not success. |
| `unsettled` | 5 | The wait ended first; the saved intent is unchanged. |

Only queries and admin views omit `admission`. Membership changes and operator
batches carry `admission`: `not_sent`, `not_admitted`, `known` or `unknown`.
`role` is `follower`, `pre_candidate`, `candidate` or `leader`, and is only an
observation. `request` echoes the operation as `operation` (snake_case command
name) with its identifying fields. A mismatched reply is never a refusal
receipt: it is `outcome_unknown` with cause `response_mismatch`.

Input causes are `too_many_arguments`, `unknown_argument`, `duplicate_argument`,
`missing_value`, `missing_socket`, `invalid_socket`, `missing_command`,
`peer_required`, `unexpected_peer`, `invalid_peer`, `invalid_timeout`,
`frame_required`, `unexpected_frame`, `invalid_frame`, `invalid_material`,
`unexpected_material`, `invalid_page`, `unexpected_page`, `intent_required`,
`unexpected_intent`, `invalid_intent`, `invalid_collection` and
`unexpected_collection`.

Owner `reason.kind` preserves the closed native categories: `ready_pending`,
`apply_pending`, `recovery_pending`, `operator_frontier_missing`,
`snapshot_maintenance_busy`, `owner_fatal`, `owner_draining`, `not_leader`,
`leader_not_current`, `leader_not_applied` (string `committed`/`applied`),
`not_serving`, `proposal_busy`, `pending_configuration`, `promotion_unavailable`,
`membership_invalid_peer` or `membership_duplicate_peer` (string `peer`),
`membership_shape_invalid`, `proposal_entry_too_large` (u32 integer
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

The node reads each whole request, and writes each whole reply, within one
five-second deadline, so a slow local client cannot hold the single admin
worker. Non-Unix platforms report exit 3 with `unsupported_platform`/`not_sent`;
this release's qualification is local Linux.

## Saved intents and collections

A saved intent file holds one complete SVI0 Submit frame. It is published once,
by linking a synced private temporary file to the operator-named path, and is
never replaced. A second caller, or a file already present, is adopted. Every
use reads the stored bytes back and syncs the file and its directory before
any send; a path that is not a regular file is refused without waiting on it.
The file is client recovery input, not a transaction-status record.
Keep it until `lifecycle_outcome` settles, keep at most one unresolved intent
per caller, and resume with `reconcile-lifecycle`, never with a new intent.

`collect` and `combine` replace a network admin surface. Run `collect` on every
voter and learner, keep each JSON file, then run `combine` against the
reference membership. A file whose collection, target, identity, lineage or
peer does not match is reported as foreign and never fills a gap, as is a
file whose replies contradict its own identity or process instance
(`inconsistent_observation`). A member
without an actual observation in this batch leaves it incomplete (exit 5); so
does a file missing a required read, whose member is `partial`. The
summary certifies that batch only, not current liveness.

The public usage-example phase is optional/skipped. Full public-only application
provisioning and official-release availability remain separate migration and
acceptance work; this CLI does not qualify learner snapshot bootstrap by itself.
