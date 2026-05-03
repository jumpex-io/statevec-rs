#!/usr/bin/env bash
set -euo pipefail

source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/common.sh"

log "stopping statevec-eval"
"$DEPLOY_EVAL_DIR/stop_eval.sh" || true

log "stopping evaluation docker compose dependencies"
compose down -v --remove-orphans >/dev/null 2>&1 || true

log "removing local evaluation run state"
rm -rf \
  "$(eval_data_dir)" \
  "$(eval_rendered_config_path)" \
  "$(eval_pid_file)" \
  "$(eval_out_file)" \
  "$(eval_err_file)" \
  "$ROOT_DIR/run/bank_eval.local_replay.out" \
  "$ROOT_DIR/run/bank_eval.local_replay.err"

log "bank evaluation environment reset complete"
