# Local configured-host launcher

`sv-local` starts a local single-voter cluster by default. It uses
Python 3.11+ and the standard library. Supply the paths of the official
`sv-node` and `sv-admin` binaries. The launcher runs these binaries
as foreground child processes on Linux. A single voter provides local
persistence and recovery, but no machine-failure availability.

Run from this public checkout without installing anything:

```sh
python3 tools/sv-local/cli.py --help
PYTHONPATH=tools/sv-local python3 -m unittest discover -s tools/sv-local/tests -v
```

Alternatively install this directory with `pip install ./tools/sv-local`
and invoke `sv-local`. Applications may import `sv_local` instead of
copying its process management. With a source checkout, add this directory to
`PYTHONPATH`; do not copy the SDK into an application.

## Artifact selection and initialization

For a release bundle, use its `profiles/` and follow the bundle's `README.md`.
The profiles point to the shared `statevec.release-manifest` in
`artifacts.json`. The launcher checks the native target, SDK revision and
file digests, and resolves asset paths relative to the bundle. A shipped
profile with no genesis hash gets a fresh one on `init`; reopen retains the
generated Node configs. Use `--store-path` for the provisioned Core store,
`--projector-config` for the application's history configuration and
`--base-port` to override the profile's port range.

For explicit local builds, record the binary paths below before `init`.

`artifacts OUTPUT --node PATH --admin PATH --host-revision SHA --sdk-revision SHA
[--file projector=PATH]` records absolute paths and SHA256 hashes
of explicitly supplied files. Other application labels are allowed. OUTPUT must
not exist. All files and revisions are operator inputs: a manifest is not build
provenance or release qualification. This tool downloads nothing, builds nothing
and does not imply official releases are available. Keep the matched host/SDK
pair and the application's plugin build fixed together.

For example, select the two host binaries explicitly:

```sh
sv-local artifacts artifacts.json \
  --node /absolute/path/to/sv-node \
  --admin /absolute/path/to/sv-admin \
  --host-revision "$HOST_REV" --sdk-revision "$SDK_REV"
```

Both revision variables must contain full, lowercase, 40-character Git SHAs.
The saved paths select the binaries for all subsequent `run` commands.

Prepare the Core package store with the matching host's `sv-package`:

```sh
/absolute/path/to/sv-package provision /absolute/path/to/materials core.svtar
/absolute/path/to/sv-package bind /absolute/path/to/materials V0 PACKAGE_DIGEST x86_64-unknown-linux-gnu
```

Replace `V0` and `PACKAGE_DIGEST` with the values from the provision result,
and the target with the host's own (`aarch64-unknown-linux-gnu` on aarch64).
The Core package contains its configuration and artifact. The launcher passes
the store selection to Node; Node checks and loads it. Store provisioning and
binding remain explicit operations, separate from startup and reopen.

`init PROFILE ROOT` reads a JSON object with `format`, `version`, `artifacts` (manifest
path), `base_port`, `node` (shared official Node configuration), `core` and an
optional `projector`. Relative paths are relative to PROFILE, not cwd.
Omitting `initial_voters` selects `[1]`; omitting `learner_only_peers` selects
no learners. Its only node is the leader, so with one voter and no learners
the launcher also enables direct client reads there, leader reads included
(`"client_reads": {"enabled": true, "leader": true}`), unless the profile's
`node` sets `client_reads` itself. Every other topology keeps Node's default,
client reads off; enable them explicitly where wanted. This single-voter
profile is the default local setup:

```json
{
  "format": "statevec.local-profile",
  "version": 1,
  "artifacts": "artifacts.json",
  "base_port": 24100,
  "node": {
    "cluster_name": "application-demo",
    "cluster_mode": "cross_check",
    "genesis_config_hash": "REPLACE_WITH_APPLICATION_ARRAY_OF_32_BYTES",
    "streams": [[7, 1]],
    "raft_tick_millis": 10,
    "raft_log_retention": "checkpoint",
    "checkpoint_interval_raft_indices": 1000,
    "checkpoint_min_selected_duration_ms": 3600000
  },
  "core": {
    "store_path": "materials",
    "initial_core_version": "REPLACE_WITH_V0_FROM_PROVISION"
  }
}
```

Replace both placeholders with the application's actual values. The Core
declaration also accepts Node's optional `max_load_attempts` and
`max_artifact_bytes` limits. Old Core `artifact/path/sha256/config_path` fields
are rejected; update the profile explicitly to use its provisioned store.
The Python `initialize` API requires an absolute `store_path`; the CLI resolves
relative paths against PROFILE.

The TCP launcher assigns four consecutive loopback ports per peer. For slot
`s`, ingress is `base_port + 4*s`, Control is `+1`, bulk is `+2`, and admin
is `+3`. The slot order remains voters followed by learners. Validate
`1024 <= base_port` and `base_port + 4*N - 1 <= 65535` before creating files.
The generated `deployment.json` uses `statevec.deployment` and version 1.
Its `port` field is the base. `run` checks the format and version before
starting a child.
The launcher supplies the per-node admin fields; shared templates cannot set
them. It also gives each peer its own `checkpoint_root`, the sibling
`checkpoints-<peer>` of its `root-<peer>` data root; a template that sets
`checkpoint_root` is refused, since one value would be shared by every peer.
Both roots share the local filesystem, which does not qualify installed I/O
behavior. Metrics is optional and separate from this four-port layout.

Explicit multi-peer profiles remain available, with at most 16 peers.
A Projector requires an additional learner. For that profile,
declare `learner_only_peers`, use `external` retention, and supply a
`projector` object with `artifact`, `target`, `config_path`, `id`, `projector_id`
and `recovery_mode`. Only learners receive that configuration. Projector
artifact paths and hashes still come from the artifact manifest.

Node validates the complete configuration and decides admission and recovery.
Before adding a configured learner, the launcher derives the expected cluster
and receiving peer from local Node inspection and passes them to the admin
command. It never learns the intended identity from an answering TCP endpoint.
`init` only creates deployment files. It does not execute `InitializeCore`.
ROOT must not exist. A physical initialization failure may leave a partial new
directory; it is retained, never deleted or automatically reused.

Use a new deployment root for this format. `existing` reopens a deployment
written in the same format. Shared-host isolation for loopback admin uses
deployment controls such as a network namespace.

A deployment made before checkpoint roots existed cannot be reopened. Node
refuses a data root that still holds `checkpoints/`, and an existing root
never gains the checkpoint root's binding marker; moving the files does not
help. Create a new deployment with `init` and fresh roots, as the paired
host repository's `docs/cluster-next/deployment.md` describes.

## Run and stop

```sh
sv-local init profile.json /absolute/path/cluster
sv-local run /absolute/path/cluster fresh
# After stopping, reopen the exact same files:
sv-local run /absolute/path/cluster existing
```

After the node becomes leader, initialize the Core explicitly from another
terminal with the same admin binary. Read the generated node configuration's
`admin_listen_ip`/`admin_listen_port`; replace the illustrative port 7401 below
with its actual port. The launcher assigns a dedicated loopback admin port
per node, separate from ingress and both peer transports.

```sh
cat > /absolute/path/prepared-members.json <<'JSON'
{"format":"statevec.members","version":1,"members":[{"peer_id":"1","admin_ip":"127.0.0.1","admin_port":7401}]}
JSON
/absolute/path/to/sv-admin --admin-ip 127.0.0.1 --admin-port 7401 \
  query-active-core
```

For this single-node profile, verify `local.package.core_version` equals
`lifecycle.initial_version` in the answer and copy its
`local.package.package_digest` as `V0_PACKAGE_DIGEST` below. Multi-member
profiles must list every applied voter and learner's admin IP/port and prepare the
same V0 package on each before submitting.

```sh
/absolute/path/to/sv-admin --admin-ip 127.0.0.1 --admin-port 7401 \
  create-lifecycle-intent --intent /absolute/path/initialize.svi0 --initialize-core
/absolute/path/to/sv-admin --admin-ip 127.0.0.1 --admin-port 7401 \
  submit-lifecycle --intent /absolute/path/initialize.svi0 \
  --prepared-members /absolute/path/prepared-members.json \
  --package-digest V0_PACKAGE_DIGEST
```

Keep the intent file. Follow the [admin guide](../../docs/user-guide/administration.md)
to reconcile an unknown result. `existing` reuses the data root and does not
create another initialization intent. Process startup alone does not establish
that the Core or application is ready.

Voters start first. Learners are admitted through the official admin CLI and
start after applied membership includes them. Proposal acceptance is not applied
membership. Unknown/missing/malformed write replies never authorize another
proposal; an in-process unknown result is followed only by membership queries.
After interruption or a failed run, reconcile membership before starting again.
Explicit transient non-admission can be retried; permanent refusals stop startup.
No journal, root, offset or checkpoint is reset. There is no automatic restart.

The foreground launcher owns and reaps its direct child processes on partial
startup, failure, Ctrl-C, SIGTERM and SIGHUP. A five-second stop budget escalates
to SIGKILL and reports failure; it is never called graceful retirement. Startup
signals latch stop rather than raising inside physical process construction:
an accepted child is registered before cleanup, and the next checked spawn is
refused. Application startup/service loops must call `check()` at bounded
intervals (the built-in admin calls are bounded); stop does not interrupt
arbitrary blocking application code. Node startup may wait for an already
accepted blocking construction. SIGKILL of the
launcher itself cannot guarantee child cleanup; service managers should use
process-group/cgroup containment. Node must exit zero. Applications can explicitly
permit SIGTERM exit for their own non-Node processes through `spawn`.

Application code may subclass `Deployment` to add its services and readiness
after `super().start(mode)`, using `spawn`/`adopt_child` for the same teardown.
Business initialization, REST/WS routes, credentials, market feeds and trade
smoke workloads stay in the application. Child startup is not application-ready.
