#!/usr/bin/env bash
set -euo pipefail

source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/common.sh"

STATEVEC_CLI_BIN="$(statevec_cli_bin_path)"
require_executable "$STATEVEC_CLI_BIN" "statevec-cli"

"$STATEVEC_CLI_BIN" --url "http://$BANK_EVAL_OPERATION_BIND" "${1:-/status}"
