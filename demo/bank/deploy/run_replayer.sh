#!/usr/bin/env bash
set -euo pipefail

source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/common.sh"

usage() {
  cat >&2 <<'USAGE'
usage: run_replayer.sh [replay shell flags]

This deploy profile opens the statevec-replay shell:
  statevec-replay --config <rendered eval config> [replay shell flags]

Examples:
  demo/bank/deploy/run_replayer.sh
  demo/bank/deploy/run_replayer.sh --to-tx-seq 1000
  demo/bank/deploy/run_replayer.sh --from-tx-seq 1000 --emit-events
  BANK_EVAL_STATEVEC_REPLAY_BIN=/path/to/statevec-replay demo/bank/deploy/run_replayer.sh
USAGE
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  usage
  exit 0
fi

ensure_runtime_binaries

DATA_DIR="$(eval_data_dir)"
RENDERED_CONFIG_PATH="$(eval_rendered_config_path)"
STATEVEC_REPLAY_BIN="$(statevec_replay_bin_path)"
OUT_FILE="$ROOT_DIR/run/bank_eval.local_replay.out"
ERR_FILE="$ROOT_DIR/run/bank_eval.local_replay.err"

if eval_running; then
  log "refusing local replay while statevec-eval is running: pid=$(cat "$(eval_pid_file)")" >&2
  log "stop statevec-eval first or replay from a copied data directory" >&2
  exit 1
fi

if [[ ! -d "$DATA_DIR" ]]; then
  log "data dir does not exist: $DATA_DIR" >&2
  exit 1
fi

mkdir -p "$ROOT_DIR/run"
ensure_bank_plugin_release_build
render_eval_config "$RENDERED_CONFIG_PATH"
: > "$OUT_FILE"
: > "$ERR_FILE"

log "running eval local replayer"
log "  data_dir: $DATA_DIR"
log "  rendered_config: $RENDERED_CONFIG_PATH"
log "  statevec_replay_bin: $STATEVEC_REPLAY_BIN"
log "  out_file: $OUT_FILE"
log "  err_file: $ERR_FILE"

if "$STATEVEC_REPLAY_BIN" \
  --config "$RENDERED_CONFIG_PATH" \
  "$@" > >(tee -a "$OUT_FILE") 2> >(tee -a "$ERR_FILE" >&2); then
  log "local replay completed"
else
  exit_code=$?
  log "local replay failed: exit_code=$exit_code err_file=$ERR_FILE out_file=$OUT_FILE" >&2
  exit "$exit_code"
fi
