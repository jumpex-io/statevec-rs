# Local configured-host launcher

`statevec-local` is the public, application-independent part of the former
future-ex deployment helper. It uses Python 3.11+ and the standard library.
It launches supplied official `statevec-node` / `statevec-admin` binaries;
it neither embeds private engine/Raft code nor replaces those executables.
This is a local Linux foreground tool, not a remote deployment controller.

Run from this public checkout without installing anything:

```sh
python3 tools/statevec-local/cli.py --help
PYTHONPATH=tools/statevec-local python3 -m unittest discover -s tools/statevec-local/tests -v
```

Alternatively install this directory with `pip install ./tools/statevec-local`
and invoke `statevec-local`. Applications may import `statevec_local` instead of
copying its process management. With a source checkout, add this directory to
`PYTHONPATH`; do not copy the SDK into an application.

## Artifact selection and initialization

`artifacts OUTPUT --node PATH --admin PATH --host-revision SHA --sdk-revision SHA
--file core=PATH [--file projector=PATH]` records absolute paths and SHA256 hashes
of explicitly supplied files. Other application labels are allowed. OUTPUT must
not exist. All files and revisions are operator inputs: a manifest is not build
provenance or release qualification. This tool downloads nothing, builds nothing
and does not imply official releases are available. Keep the matched host/SDK
pair and the application's plugin build fixed together.

`init PROFILE ROOT` reads a JSON object with `version: 1`, `artifacts` (manifest
path), `base_port`, `node` (shared official Node configuration), `core` and an
optional `projector`. Relative paths are relative to PROFILE, not cwd. Example:

```json
{
  "version": 1,
  "artifacts": "artifacts.json",
  "base_port": 24100,
  "node": {
    "cluster_name": "application-demo",
    "cluster_mode": "full_verify",
    "initial_voters": [1, 2, 3],
    "learner_only_peers": [4],
    "genesis_config_hash": "REPLACE_WITH_APPLICATION_ARRAY_OF_32_BYTES",
    "streams": [[7, 1]],
    "raft_tick_millis": 10,
    "raft_log_retention": "external",
    "checkpoint_interval_raft_indices": 1,
    "checkpoint_min_selected_duration_ms": 3600000
  },
  "core": {
    "artifact": "core",
    "target": "x86_64-unknown-linux-gnu",
    "config_path": "core.json"
  },
  "projector": {
    "artifact": "projector",
    "target": "x86_64-unknown-linux-gnu",
    "config_path": "projector.json",
    "id": "application-view",
    "projector_id": "application-view",
    "recovery_mode": "continuous-history"
  }
}
```

The placeholder is not a valid Node genesis value; applications must supply
their actual configuration. The tool assigns three consecutive loopback ports
per peer (ingress/control/bulk), with at most 16 peers. Only learners receive the
optional projector. Plugin configurations are opaque application-owned files.
Node itself validates the complete configuration and decides admission/recovery.
The tool does not understand business schemas, offsets, readiness or bootstrap.
ROOT must not exist. A physical initialization failure may leave a partial new
directory; it is retained, never deleted or automatically reused.

## Run and stop

```sh
statevec-local run /absolute/path/cluster fresh
# After stopping, reopen the exact same files:
statevec-local run /absolute/path/cluster existing
```

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
