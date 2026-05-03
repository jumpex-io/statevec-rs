#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

"$SCRIPT_DIR/send_command.sh" add-flight flight_id=JX100 flight_date=20260503 origin=SIN destination=NRT airline=JX aircraft_model=A350 economy_total=120 business_total=24 first_total=8
"$SCRIPT_DIR/send_command.sh" add-flight flight_id=JX101 flight_date=20260503 origin=NRT destination=SIN airline=JX aircraft_model=A350 economy_total=120 business_total=24 first_total=8
"$SCRIPT_DIR/send_command.sh" add-flight flight_id=JX200 flight_date=20260504 origin=SIN destination=HKG airline=JX aircraft_model=B789 economy_total=180 business_total=32 first_total=0
"$SCRIPT_DIR/send_command.sh" add-flight flight_id=JX201 flight_date=20260504 origin=HKG destination=SIN airline=JX aircraft_model=B789 economy_total=180 business_total=32 first_total=0
"$SCRIPT_DIR/send_command.sh" add-flight flight_id=JX300 flight_date=20260505 origin=SIN destination=SYD airline=JX aircraft_model=A359 economy_total=200 business_total=40 first_total=6
"$SCRIPT_DIR/send_command.sh" add-flight flight_id=JX301 flight_date=20260505 origin=SYD destination=SIN airline=JX aircraft_model=A359 economy_total=200 business_total=40 first_total=6
"$SCRIPT_DIR/send_command.sh" add-flight flight_id=JX400 flight_date=20260506 origin=SIN destination=BKK airline=JX aircraft_model=A321 economy_total=160 business_total=12 first_total=0
"$SCRIPT_DIR/send_command.sh" add-flight flight_id=JX401 flight_date=20260506 origin=BKK destination=SIN airline=JX aircraft_model=A321 economy_total=160 business_total=12 first_total=0
"$SCRIPT_DIR/send_command.sh" add-flight flight_id=JX500 flight_date=20260507 origin=SIN destination=ICN airline=JX aircraft_model=B789 economy_total=176 business_total=30 first_total=4
"$SCRIPT_DIR/send_command.sh" add-flight flight_id=JX501 flight_date=20260507 origin=ICN destination=SIN airline=JX aircraft_model=B789 economy_total=176 business_total=30 first_total=4
