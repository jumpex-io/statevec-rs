#!/usr/bin/env bash
set -euo pipefail

source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/common.sh"

usage() {
  cat >&2 <<'USAGE'
usage: run_client_console.sh [statevec-cli args...]

Open the statevec-cli client console for the flight-booking eval runtime.
Extra arguments are passed through to statevec-cli.

Examples:
  demo/flight-booking/deploy/run_client_console.sh
  demo/flight-booking/deploy/run_client_console.sh /status
  demo/flight-booking/deploy/run_client_console.sh /state/stats
USAGE
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  usage
  exit 0
fi

STATEVEC_CLI_BIN="$(statevec_cli_bin_path)"
require_executable "$STATEVEC_CLI_BIN" "statevec-cli"

exec "$STATEVEC_CLI_BIN" --url "http://$FLIGHT_EVAL_OPERATION_BIND" "$@"
