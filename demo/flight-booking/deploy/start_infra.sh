#!/usr/bin/env bash
set -euo pipefail

source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/common.sh"

REDPANDA_TOPIC_NAMES=(
  "$FLIGHT_EVAL_COMMAND_TOPIC"
  "$FLIGHT_EVAL_PUBLICATION_TOPIC"
)

compose up -d redpanda redpanda-console

wait_for_redpanda
wait_for_kafka_listener

for topic in "${REDPANDA_TOPIC_NAMES[@]}"; do
  ensure_topic "$topic"
done

log "topic details:"
for topic in "${REDPANDA_TOPIC_NAMES[@]}"; do
  docker_rpk topic describe "$topic"
done

log "flight booking evaluation docker environment started"
log "project: $COMPOSE_PROJECT_NAME"
log "topics: ${REDPANDA_TOPIC_NAMES[*]}"
log "endpoints:"
log "  kafka: $FLIGHT_EVAL_KAFKA_BROKER"
log "  redpanda_console: http://127.0.0.1:8083"
