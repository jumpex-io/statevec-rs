# Official administration tool

Use the `statevec-admin` binary from the matching official host distribution.
Applications invoke it as a subprocess; they do not need a private Rust
dependency, an admin protocol encoder, or an SDK-side admin server. This tool
is on the unreleased 0.4.0 paired development line: local builds are not evidence
that an official binary/revision has been published. Keep the host and SDK
revisions pinned together for deployment.

## Invocation and authority

The two commands are:

- `statevec-admin --socket ABSOLUTE_PATH query-membership`
- `statevec-admin --socket ABSOLUTE_PATH add-learner --peer-id DECIMAL_U64`

`--timeout-ms` is optional (1 through 60000 inclusive, default 5000). It bounds
the physical connect/send/receive exchange, including partial reads and writes;
it does not cancel an accepted proposal or prove non-admission. `--help` alone
returns a versioned JSON usage object. Unknown/duplicate options, missing values,
relative or unrepresentable socket paths, non-decimal/out-of-u64 IDs and invalid
timeouts are refused before connecting. Leading decimal zeros are allowed;
zero IDs are sent to the owner, which alone decides membership eligibility.

The endpoint must be enabled in the node configuration. It is the existing
node-local same-user Unix socket, in an operator-owned private directory.
The tool never creates or removes the socket, follows a leader hint, retries a
request, promotes voters, rewrites topology or changes membership policy.
Learner IDs/endpoints must already be allowed by the node's installed topology.

`proposal_accepted` means the owner proposed the change. It does **not** mean
the change committed/applied. A later `query-membership` observes the current
applied membership, not a durable receipt for an earlier particular request.
After an unknown outcome, inspect that membership and reconcile before choosing
another explicit operation. Do not automatically replay an uncertain request.

## JSON v1 and exits

Stdout is one JSON object plus newline, at most 32768 bytes. Every object has
integer `version: 1` and string `category`. All u64-valued IDs, indices, terms
and sequences are **decimal strings**, including arrays and refusal fields.
Hashes/digests are 64 lowercase hex characters. Booleans remain JSON booleans;
optional observations are `null` when absent. No membership/WAL/checkpoint DTO
is published in the SDK.

| Exit | Category | Fields in addition to version/category |
| --- | --- | --- |
| 0 | `applied_membership` | `applied_index`, `applied_term`, nullable `engine_tx_seq`, nullable `tx_chain_hash`, `voters`, `voters_outgoing`, `learners`, `learners_next`, `auto_leave`, `role`, `observed_term`, nullable `leader_hint` |
| 0 | `proposal_accepted` | `term`, `index`, `entry_digest` |
| 0 | `help` | `usage` |
| 2 | `invalid_input` | `admission: "not_sent"`, typed `cause` string |
| 3 | `io_failure` | `admission: "not_sent"`, `stage: "setup"` or `"connect"`, typed `cause` object |
| 4 | `owner_refusal` | `admission: "not_admitted"`, exact `request`, typed `reason` object |
| 4 | `queue_full` / `queue_closed` | `admission: "not_admitted"`, exact `request` |
| 5 | `outcome_unknown` | `admission: "unknown"`, `stage: "send"` or `"receive"`, typed `cause` object |

`role` is `follower`, `pre_candidate`, `candidate` or `leader`, and is only an
observation. `request` has `operation: "query_membership"` or `"add_learner"`
(the latter also has string `peer_id`). A mismatched reply is never a refusal
receipt: it is `outcome_unknown` with cause `response_mismatch`.

Input causes are `too_many_arguments`, `unknown_argument`, `duplicate_argument`,
`missing_value`, `missing_socket`, `invalid_socket`, `missing_command`,
`peer_required`, `unexpected_peer`, `invalid_peer`, and `invalid_timeout`.

Owner `reason.kind` preserves the closed native categories: `ready_pending`,
`apply_pending`, `owner_fatal`, `owner_draining`, `not_leader`,
`leader_not_current`, `leader_not_applied` (string `committed`/`applied`),
`not_serving`, `proposal_busy`, `pending_configuration`, `promotion_unavailable`,
`membership_invalid_peer` or `membership_duplicate_peer` (string `peer`),
`membership_shape_invalid`, `proposal_entry_too_large` (u32 integer
`observed`/`limit`), `raw_node`, and `membership_validation`. Rendered private
diagnostic strings are deliberately omitted, never parsed as a classification.

Transport cause `kind: "io"` includes `io_kind` (the typed Rust ErrorKind name)
and nullable integer `os_code`. OS codes are platform-specific; never interpret
an errno as an owner's refusal. Invalid replies have `frame_header`,
`frame_too_large` (integer `observed`/`limit`), `json` (class
`io`/`syntax`/`data`/`eof`), or `response_mismatch`. Paths, raw inputs and raw I/O
messages are not echoed. Stderr is supplementary, not a stable protocol.

An output write/flush failure also exits 3, and may leave incomplete or no
stdout; it cannot undo a request already sent. An output-size failure emits a
small `io_failure` with `stage: "output"`, `cause.kind: "output_bound"` and
`admission: "unknown"`. A consumer must check both exit status and complete JSON,
not assume exit 3 alone means `not_sent`. Non-Unix platforms report exit 3 with
`unsupported_platform`/`not_sent`; this release's qualification is local Linux.

The public usage-example phase is optional/skipped. Full public-only application
provisioning and official-release availability remain separate migration and
acceptance work; this CLI does not qualify learner snapshot bootstrap by itself.
