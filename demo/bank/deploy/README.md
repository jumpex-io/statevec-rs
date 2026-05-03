# bank Evaluation Deploy

This directory runs the `bank` demo plugin against the single-host
`statevec-eval` runtime.

It starts one local Redpanda broker with Docker Compose and one host-level
`statevec-eval` process. Runtime binaries are not built by this repository;
set their paths in a local `.env.eval` file.

Download the runtime binaries from
[`jumpex-io/statevec-runtime`](https://github.com/jumpex-io/statevec-runtime).

## Runtime Binary Paths

Create a local env file from the checked-in example:

```bash
cp demo/bank/deploy/.env.eval.example demo/bank/deploy/.env.eval
```

`.env.eval` defines the runtime binary paths explicitly. Download the runtime
binaries, extract them locally, and set these values to the executable paths:

```text
BANK_EVAL_STATEVEC_EVAL_BIN=/absolute/path/to/statevec-eval
BANK_EVAL_STATEVEC_REPLAY_BIN=/absolute/path/to/statevec-replay
BANK_EVAL_STATEVEC_CLI_BIN=/absolute/path/to/statevec-cli
```

The scripts fail fast if any configured runtime binary path is missing or not
executable.

GNU/Linux runtime binaries and demo plugin `.so` files are expected to run on
glibc `>= 2.30`. Ubuntu 20.04+ and Debian 11+ satisfy this baseline. Build
custom `*-unknown-linux-gnu` plugins on a system with a compatible glibc
baseline, and use matching architecture/runtime binaries.

The single-host evaluation memory limit is configured in MiB:

```text
BANK_EVAL_STATE_MEMORY_LIMIT_MB=512
```

The current evaluation build accepts up to `512` MiB.

The bank plugin is built from this repository by default:

```text
target/release/libbank.so      # Linux
target/release/libbank.dylib   # macOS
```

Set `BANK_EVAL_BANK_PLUGIN_DYLIB` if you want to load a prebuilt plugin.

## Start

```bash
demo/bank/deploy/start_infra.sh
demo/bank/deploy/start_eval.sh
demo/bank/deploy/status.sh
```

`start_eval.sh` starts the Docker infra automatically unless
`BANK_EVAL_SKIP_INFRA_BOOTSTRAP=1` is set.

Default endpoints:

- statevec-eval operation server: `http://127.0.0.1:19181`
- Kafka bootstrap: `127.0.0.1:19094`
- Redpanda Console: `http://127.0.0.1:8082`

The operation server exposes `/health`, `/status`, `/state/stats`, and
`/state/inspect-by-sysid?sysid=<id>`.

## Command Input

This deploy profile prepares Kafka topics and starts the runtime. It does not
include a bank command producer binary yet.

To drive the demo, write StateVec command envelopes for the bank schema to:

```text
statevec.bank.eval.commands
```

The bank plugin supports these command kinds:

- `Deposit`
- `Withdraw`
- `Transfer`

## Local Replay

`run_replayer.sh` is wired as the deploy entrypoint for `statevec-replay`:

```bash
demo/bank/deploy/stop_eval.sh
demo/bank/deploy/run_replayer.sh
demo/bank/deploy/run_replayer.sh --to-tx-seq 1000
demo/bank/deploy/run_replayer.sh --emit-state-deltas --emit-events
```

The script refuses to read the eval data directory while `statevec-eval` is
still running.

## Stop / Reset

```bash
demo/bank/deploy/stop_eval.sh
demo/bank/deploy/stop_infra.sh
demo/bank/deploy/reset_eval_env.sh
```

## Files

- `docker-compose.yml`: one-node Redpanda and Redpanda Console
- `.env.eval.example`: ports, topics, data dir, and runtime binary path template
- `config_eval.toml`: `statevec-eval` config template
- `plugin_eval.toml`: bank plugin config
- `start_infra.sh`, `stop_infra.sh`
- `start_eval.sh`, `stop_eval.sh`
- `status.sh`
- `run_replayer.sh`
- `reset_eval_env.sh`

Generated files go under `run/`:

- data dir: `bank-eval`
- rendered config: `bank_eval.rendered.toml`
- pid/log files: `bank_eval.*`
