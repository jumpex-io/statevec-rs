#!/usr/bin/env bash
set -euo pipefail

source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/common.sh"

usage() {
  cat >&2 <<'USAGE'
usage: send_command.sh <command> key=value...

Commands:
  add-flight flight_id=JX100 flight_date=20260503 origin=SIN destination=NRT airline=JX aircraft_model=A350 economy_total=2 business_total=1 first_total=1
  reserve-order flight_id=JX100 order_id=ORD-1 passenger_document_id=P1234567 nationality=SG birth_date=19900101 document_type=1 cabin=economy
  cancel-reservation flight_id=JX100 passenger_document_id=P1234567 order_id=ORD-1
  retire-flight flight_id=JX100

Examples:
  demo/flight-booking/deploy/send_command.sh add-flight flight_id=JX100 flight_date=20260503 origin=SIN destination=NRT airline=JX aircraft_model=A350 economy_total=2 business_total=1 first_total=1
  demo/flight-booking/deploy/send_command.sh reserve-order flight_id=JX100 order_id=ORD-1 passenger_document_id=P1234567 nationality=SG birth_date=19900101 document_type=1 cabin=economy
  demo/flight-booking/deploy/send_command.sh cancel-reservation flight_id=JX100 passenger_document_id=P1234567 order_id=ORD-1
USAGE
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" || $# -eq 0 ]]; then
  usage
  exit 0
fi

if ! docker_rpk topic describe "$FLIGHT_EVAL_COMMAND_TOPIC" >/dev/null 2>&1; then
  log "command topic is not reachable: $FLIGHT_EVAL_COMMAND_TOPIC" >&2
  log "run $DEPLOY_DIR/start_infra.sh first, or check COMPOSE_PROJECT_NAME=$COMPOSE_PROJECT_NAME" >&2
  exit 1
fi

log "encoding and sending command to $FLIGHT_EVAL_COMMAND_TOPIC"
cargo run --quiet -p flight-booking --bin flight-booking-command -- "$@" \
  | docker_rpk topic produce "$FLIGHT_EVAL_COMMAND_TOPIC" \
      -p 0 \
      -f '%v{hex}\n' \
      -o 'produced offset=%o timestamp=%d\n'
