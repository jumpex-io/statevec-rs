#!/usr/bin/env bash

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
DEMO_DIR="$ROOT_DIR/demo/bank"
DEPLOY_EVAL_DIR="$DEMO_DIR/deploy-eval"
COMPOSE_FILE="$DEPLOY_EVAL_DIR/docker-compose.yml"
ENV_FILE="${BANK_EVAL_ENV_FILE:-$DEPLOY_EVAL_DIR/.env.eval}"
ENV_EXAMPLE_FILE="$DEPLOY_EVAL_DIR/.env.eval.example"

load_env_file_preserving_overrides() {
  local line key value
  while IFS= read -r line || [[ -n "$line" ]]; do
    line="${line#"${line%%[![:space:]]*}"}"
    line="${line%"${line##*[![:space:]]}"}"
    [[ -z "$line" || "$line" == \#* ]] && continue
    [[ "$line" == *"="* ]] || continue
    key="${line%%=*}"
    value="${line#*=}"
    key="${key%"${key##*[![:space:]]}"}"
    value="${value#"${value%%[![:space:]]*}"}"
    if [[ -n "$key" && -z "${!key+x}" ]]; then
      export "$key=$value"
    fi
  done
}

if [[ -f "$ENV_FILE" ]]; then
  load_env_file_preserving_overrides < "$ENV_FILE"
fi

: "${COMPOSE_PROJECT_NAME:=bank-eval}"
export COMPOSE_PROJECT_NAME

: "${BANK_EVAL_KAFKA_BROKER:=127.0.0.1:19094}"
: "${BANK_EVAL_COMMAND_TOPIC:=statevec.bank.eval.commands}"
: "${BANK_EVAL_PUBLICATION_TOPIC:=statevec.bank.eval.publication}"
: "${BANK_EVAL_GROUP_ID:=bank-eval}"
: "${BANK_EVAL_DATA_DIR:=run/bank-eval}"
: "${BANK_EVAL_OPERATION_BIND:=127.0.0.1:19181}"
: "${BANK_EVAL_STATE_MEMORY_LIMIT_MB:=512}"
: "${BANK_EVAL_TOPIC_REPLICATION_FACTOR:=1}"
: "${BANK_EVAL_STATEVEC_EVAL_BIN:=}"
: "${BANK_EVAL_STATEVEC_REPLAY_BIN:=}"
: "${BANK_EVAL_STATEVEC_CLI_BIN:=}"
: "${BANK_EVAL_BANK_PLUGIN_DYLIB:=}"

log() {
  printf '[%s] %s\n' "$(date -u '+%Y-%m-%dT%H:%M:%SZ')" "$*"
}

compose() {
  docker compose -f "$COMPOSE_FILE" "$@"
}

docker_rpk() {
  compose exec -T redpanda rpk --brokers redpanda:9092 "$@"
}

escape_sed_replacement() {
  printf '%s' "$1" | sed -e 's/[\/&]/\\&/g'
}

toml_string_list() {
  local csv="$1"
  local out="" item trimmed escaped
  local -a items
  IFS=',' read -r -a items <<< "$csv"
  for item in "${items[@]}"; do
    trimmed="${item#"${item%%[![:space:]]*}"}"
    trimmed="${trimmed%"${trimmed##*[![:space:]]}"}"
    [[ -z "$trimmed" ]] && continue
    escaped="${trimmed//\\/\\\\}"
    escaped="${escaped//\"/\\\"}"
    if [[ -n "$out" ]]; then
      out+=", "
    fi
    out+="\"$escaped\""
  done
  printf '[%s]\n' "$out"
}

resolve_path() {
  local path="$1"
  if [[ -z "$path" ]]; then
    printf '\n'
    return 0
  fi
  if [[ "$path" = /* ]]; then
    printf '%s\n' "$path"
  else
    printf '%s\n' "$ROOT_DIR/$path"
  fi
}

statevec_eval_bin_path() {
  resolve_path "$BANK_EVAL_STATEVEC_EVAL_BIN"
}

statevec_replay_bin_path() {
  resolve_path "$BANK_EVAL_STATEVEC_REPLAY_BIN"
}

statevec_cli_bin_path() {
  resolve_path "$BANK_EVAL_STATEVEC_CLI_BIN"
}

default_bank_plugin_path() {
  local dylib_ext
  case "$(uname -s)" in
    Darwin) dylib_ext="dylib" ;;
    *) dylib_ext="so" ;;
  esac
  printf '%s\n' "$ROOT_DIR/target/release/libbank.${dylib_ext}"
}

bank_plugin_path() {
  if [[ -n "$BANK_EVAL_BANK_PLUGIN_DYLIB" ]]; then
    resolve_path "$BANK_EVAL_BANK_PLUGIN_DYLIB"
  else
    default_bank_plugin_path
  fi
}

eval_data_dir() {
  resolve_path "$BANK_EVAL_DATA_DIR"
}

eval_pid_file() {
  printf '%s\n' "$ROOT_DIR/run/bank_eval.pid"
}

eval_out_file() {
  printf '%s\n' "$ROOT_DIR/run/bank_eval.out"
}

eval_err_file() {
  printf '%s\n' "$ROOT_DIR/run/bank_eval.err"
}

eval_rendered_config_path() {
  printf '%s\n' "$ROOT_DIR/run/bank_eval.rendered.toml"
}

eval_running() {
  local pid_file pid
  pid_file="$(eval_pid_file)"
  [[ -f "$pid_file" ]] || return 1
  pid="$(cat "$pid_file")"
  [[ -n "$pid" ]] || return 1
  kill -0 "$pid" 2>/dev/null
}

eval_pid_by_config_path() {
  local config_path="$1"
  local process_list
  process_list="$(ps -ax -o pid= -o command= 2>/dev/null || true)"
  printf '%s\n' "$process_list" \
    | awk -v config="$config_path" '
        index($0, "statevec-eval run") && (index($0, "--config " config) || index($0, "--config=" config)) {
          print $1
          exit
        }
      '
}

require_executable() {
  local path="$1"
  local label="$2"
  if [[ -z "$path" ]]; then
    log "$label path is not configured" >&2
    log "copy $ENV_EXAMPLE_FILE to $ENV_FILE and set the matching BANK_EVAL_*_BIN value" >&2
    return 1
  fi
  if [[ ! -x "$path" ]]; then
    log "$label is not executable: $path" >&2
    log "set the matching BANK_EVAL_*_BIN value in $ENV_FILE to a downloaded runtime binary" >&2
    return 1
  fi
}

ensure_runtime_binaries() {
  require_executable "$(statevec_eval_bin_path)" "statevec-eval"
  require_executable "$(statevec_replay_bin_path)" "statevec-replay"
  require_executable "$(statevec_cli_bin_path)" "statevec-cli"
}

ensure_bank_plugin_release_build() {
  local plugin_bin
  plugin_bin="$(bank_plugin_path)"
  if [[ -n "$BANK_EVAL_BANK_PLUGIN_DYLIB" ]]; then
    if [[ ! -f "$plugin_bin" ]]; then
      log "configured bank plugin dylib does not exist: $plugin_bin" >&2
      return 1
    fi
    return 0
  fi

  if [[ "${BANK_EVAL_FORCE_BUILD:-0}" != "1" ]] \
    && [[ -f "$plugin_bin" ]] \
    && ! find "$DEMO_DIR/src" "$DEMO_DIR/Cargo.toml" "$ROOT_DIR/crates" \
      -type f \
      \( -name '*.rs' -o -name 'Cargo.toml' \) \
      -newer "$plugin_bin" \
      -print \
      -quit | grep -q .; then
    return 0
  fi

  log "building bank plugin"
  cargo build --release -p bank --lib
}

wait_for_redpanda() {
  local attempt
  for attempt in $(seq 1 120); do
    if compose exec -T redpanda rpk cluster health >/dev/null 2>&1; then
      return 0
    fi
    sleep 2
  done
  log "timed out waiting for redpanda cluster health" >&2
  return 1
}

kafka_broker_listener_reachable() {
  local broker="$1"
  local broker_host="${broker%%:*}"
  local broker_port="${broker##*:}"
  (exec 3<>"/dev/tcp/${broker_host}/${broker_port}") 2>/dev/null
}

close_tcp_probe_fd() {
  exec 3<&- 2>/dev/null || true
  exec 3>&- 2>/dev/null || true
}

wait_for_kafka_listener() {
  local attempt broker
  for attempt in $(seq 1 120); do
    IFS=',' read -r -a brokers <<< "$BANK_EVAL_KAFKA_BROKER"
    for broker in "${brokers[@]}"; do
      broker="${broker#"${broker%%[![:space:]]*}"}"
      broker="${broker%"${broker##*[![:space:]]}"}"
      [[ -z "$broker" ]] && continue
      if kafka_broker_listener_reachable "$broker"; then
        close_tcp_probe_fd
        return 0
      fi
      close_tcp_probe_fd
    done
    sleep 1
  done
  log "timed out waiting for kafka listener at $BANK_EVAL_KAFKA_BROKER" >&2
  return 1
}

wait_for_topic_ready() {
  local topic="$1"
  local attempt
  for attempt in $(seq 1 120); do
    if docker_rpk topic describe "$topic" >/dev/null 2>&1; then
      return 0
    fi
    sleep 1
  done
  log "timed out waiting for topic readiness: $topic" >&2
  return 1
}

ensure_topic() {
  local topic="$1"
  if docker_rpk topic describe "$topic" >/dev/null 2>&1; then
    log "topic exists: $topic"
    wait_for_topic_ready "$topic"
    return 0
  fi
  docker_rpk topic create "$topic" -p 1 -r "$BANK_EVAL_TOPIC_REPLICATION_FACTOR"
  wait_for_topic_ready "$topic"
}

render_eval_config() {
  local output_path="$1"
  local template_path="$DEPLOY_EVAL_DIR/config_eval.toml"
  local plugin_config_path="$DEPLOY_EVAL_DIR/plugin_eval.toml"
  sed \
    -e "s|__STATE_MEMORY_LIMIT_MB__|$(escape_sed_replacement "$BANK_EVAL_STATE_MEMORY_LIMIT_MB")|g" \
    -e "s|__STATEVEC_DATA_DIR__|$(escape_sed_replacement "$(eval_data_dir)")|g" \
    -e "s|__KAFKA_BROKERS__|$(escape_sed_replacement "$(toml_string_list "$BANK_EVAL_KAFKA_BROKER")")|g" \
    -e "s|__COMMAND_TOPIC__|$(escape_sed_replacement "$BANK_EVAL_COMMAND_TOPIC")|g" \
    -e "s|__GROUP_ID__|$(escape_sed_replacement "$BANK_EVAL_GROUP_ID")|g" \
    -e "s|__PUBLICATION_TOPIC__|$(escape_sed_replacement "$BANK_EVAL_PUBLICATION_TOPIC")|g" \
    -e "s|__OPERATION_BIND__|$(escape_sed_replacement "$BANK_EVAL_OPERATION_BIND")|g" \
    -e "s|__BANK_PLUGIN_DYLIB__|$(escape_sed_replacement "$(bank_plugin_path)")|g" \
    -e "s|__BANK_PLUGIN_CONFIG__|$(escape_sed_replacement "$plugin_config_path")|g" \
    "$template_path" > "$output_path"
}

wait_for_http() {
  local url="$1"
  local label="$2"
  local max_attempts="${3:-60}"
  local attempt
  for attempt in $(seq 1 "$max_attempts"); do
    if curl -fsS --max-time 2 "$url" >/dev/null 2>&1; then
      return 0
    fi
    sleep 1
  done
  log "timed out waiting for $label at $url" >&2
  return 1
}

stop_pid_file() {
  local pid_file="$1"
  local label="$2"
  local pid
  if [[ ! -f "$pid_file" ]]; then
    return 0
  fi
  pid="$(cat "$pid_file")"
  if [[ -z "$pid" ]]; then
    rm -f "$pid_file"
    log "removed empty stale pid file for $label"
    return 0
  fi
  if ! kill -0 "$pid" 2>/dev/null; then
    rm -f "$pid_file"
    log "removed stale pid file for $label pid $pid"
    return 0
  fi
  kill -TERM "$pid" || true
  local attempt
  for attempt in $(seq 1 50); do
    if ! kill -0 "$pid" 2>/dev/null; then
      rm -f "$pid_file"
      log "stopped $label pid $pid"
      return 0
    fi
    sleep 0.2
  done
  log "$label pid $pid did not exit after SIGTERM" >&2
  return 1
}
