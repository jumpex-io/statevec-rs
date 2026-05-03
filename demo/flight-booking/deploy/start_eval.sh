#!/usr/bin/env bash
set -euo pipefail

source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/common.sh"

ensure_runtime_binaries

if [[ "${FLIGHT_EVAL_SKIP_INFRA_BOOTSTRAP:-0}" != "1" ]]; then
  "$DEPLOY_DIR/start_infra.sh"
fi

DATA_DIR="$(eval_data_dir)"
PID_FILE="$(eval_pid_file)"
OUT_FILE="$(eval_out_file)"
ERR_FILE="$(eval_err_file)"
RENDERED_CONFIG_PATH="$(eval_rendered_config_path)"
STATEVEC_EVAL_BIN="$(statevec_eval_bin_path)"

mkdir -p "$ROOT_DIR/run"

if eval_running; then
  log "statevec-eval already running: pid=$(cat "$PID_FILE") pid_file=$PID_FILE data_dir=$DATA_DIR" >&2
  exit 1
fi

RUNNING_PID_BY_CONFIG="$(eval_pid_by_config_path "$RENDERED_CONFIG_PATH")"
if [[ -n "$RUNNING_PID_BY_CONFIG" ]]; then
  log "statevec-eval already has a running process: pid=$RUNNING_PID_BY_CONFIG rendered_config=$RENDERED_CONFIG_PATH data_dir=$DATA_DIR" >&2
  exit 1
fi

ensure_flight_plugin_release_build
render_eval_config "$RENDERED_CONFIG_PATH"

"$STATEVEC_EVAL_BIN" --config "$RENDERED_CONFIG_PATH" --check-config
"$STATEVEC_EVAL_BIN" plugin check --config "$RENDERED_CONFIG_PATH" >/dev/null

mkdir -p "$DATA_DIR"
rm -f "$PID_FILE"
: > "$OUT_FILE"
: > "$ERR_FILE"

log "starting statevec-eval config=$RENDERED_CONFIG_PATH"
log "  data_dir: $DATA_DIR"
log "  operation_server: http://$FLIGHT_EVAL_OPERATION_BIND"
log "  kafka: $FLIGHT_EVAL_KAFKA_BROKER"
log "  command_topic: $FLIGHT_EVAL_COMMAND_TOPIC"
nohup "$STATEVEC_EVAL_BIN" run --config "$RENDERED_CONFIG_PATH" >>"$OUT_FILE" 2>>"$ERR_FILE" &
PID=$!
printf '%s\n' "$PID" > "$PID_FILE"
disown "$PID" 2>/dev/null || true

sleep 1
if ! kill -0 "$PID" 2>/dev/null; then
  rm -f "$PID_FILE"
  if wait "$PID"; then
    EXIT_CODE=0
  else
    EXIT_CODE=$?
  fi
  log "statevec-eval startup failed: pid=$PID exit_code=$EXIT_CODE err_file=$ERR_FILE out_file=$OUT_FILE" >&2
  if [[ -s "$ERR_FILE" ]]; then
    tail -n 40 "$ERR_FILE" >&2 || true
  elif [[ -s "$OUT_FILE" ]]; then
    tail -n 40 "$OUT_FILE" >&2 || true
  fi
  exit 1
fi

wait_for_http "http://$FLIGHT_EVAL_OPERATION_BIND/health" "statevec-eval operation server"

for _ in $(seq 1 5); do
  if ! kill -0 "$PID" 2>/dev/null; then
    rm -f "$PID_FILE"
    log "statevec-eval exited after operation server became reachable: pid=$PID err_file=$ERR_FILE out_file=$OUT_FILE" >&2
    if [[ -s "$ERR_FILE" ]]; then
      tail -n 40 "$ERR_FILE" >&2 || true
    elif [[ -s "$OUT_FILE" ]]; then
      tail -n 40 "$OUT_FILE" >&2 || true
    fi
    exit 1
  fi
  sleep 1
done

log "started statevec-eval"
log "  pid: $PID"
log "  data_dir: $DATA_DIR"
log "  rendered_config: $RENDERED_CONFIG_PATH"
log "  out_file: $OUT_FILE"
log "  err_file: $ERR_FILE"
log "  operation_server: http://$FLIGHT_EVAL_OPERATION_BIND"
