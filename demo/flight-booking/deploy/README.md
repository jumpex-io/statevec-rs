# flight-booking Evaluation Deploy

This directory runs the `flight-booking` demo plugin against the single-host
`statevec-eval` runtime.

It starts one local Redpanda broker with Docker Compose and one host-level
`statevec-eval` process. Runtime binaries are not built by this repository;
set their paths in a local `.env.eval` file.

Download the runtime binaries from
[`jumpex-io/statevec-runtime`](https://github.com/jumpex-io/statevec-runtime).

## Runtime Binary Paths

Create a local env file from the checked-in example:

```bash
cp demo/flight-booking/deploy/.env.eval.example demo/flight-booking/deploy/.env.eval
```

`.env.eval` defines the runtime binary paths explicitly. Download the runtime
binaries, extract them locally, and set these values to the executable paths:

```text
FLIGHT_EVAL_STATEVEC_EVAL_BIN=/absolute/path/to/statevec-eval
FLIGHT_EVAL_STATEVEC_REPLAY_BIN=/absolute/path/to/statevec-replay
FLIGHT_EVAL_STATEVEC_CLI_BIN=/absolute/path/to/statevec-cli
```

The scripts fail fast if any configured runtime binary path is missing or not
executable.

GNU/Linux runtime binaries and demo plugin `.so` files are expected to run on
glibc `>= 2.30`. Ubuntu 20.04+ and Debian 11+ satisfy this baseline. Build
custom `*-unknown-linux-gnu` plugins on a system with a compatible glibc
baseline, and use matching architecture/runtime binaries.

On Windows, run this deploy profile from WSL2 rather than PowerShell or cmd.exe.
Use a WSL2 Linux distro with Docker access and the matching GNU/Linux runtime
binaries.

The flight booking plugin is built from this repository by default:

```text
target/release/libflight_booking.so      # Linux
target/release/libflight_booking.dylib   # macOS
```

Set `FLIGHT_EVAL_FLIGHT_PLUGIN_DYLIB` if you want to load a prebuilt plugin.

## Start

```bash
demo/flight-booking/deploy/start_infra.sh
demo/flight-booking/deploy/start_eval.sh
demo/flight-booking/deploy/status.sh
```

`start_eval.sh` starts the Docker infra automatically unless
`FLIGHT_EVAL_SKIP_INFRA_BOOTSTRAP=1` is set.

Default endpoints:

- statevec-eval operation server: `http://127.0.0.1:19182`
- Kafka bootstrap: `127.0.0.1:19095`
- Redpanda Console: `http://127.0.0.1:8083`

## Command Input

This deploy profile prepares Kafka topics, starts the runtime, and includes
helper scripts for sending flight booking commands.

Enter the `statevec-cli` client console:

```bash
demo/flight-booking/deploy/run_client_console.sh
```

Query operation API paths through the same wrapper:

```bash
demo/flight-booking/deploy/run_client_console.sh /status
demo/flight-booking/deploy/run_client_console.sh /state/stats
```

To drive the demo, write StateVec command envelopes for the flight booking
schema to:

```text
statevec.flight_booking.eval.commands
```

The plugin supports these command kinds:

- `AddFlight`
- `RetireFlight`
- `ReserveOrder`
- `CancelReservation`

Reserve and cancel business failures are committed as `ReserveResult` and
`CancelResult` events instead of plugin runtime errors.

Send commands with the bundled helper:

```bash
demo/flight-booking/deploy/send_command.sh add-flight flight_id=JX100 flight_date=20260503 origin=SIN destination=NRT airline=JX aircraft_model=A350 economy_total=120 business_total=24 first_total=8
demo/flight-booking/deploy/send_command.sh reserve-order flight_id=JX100 order_id=ORD-1 passenger_document_id=P1234567 nationality=SG birth_date=19900101 document_type=1 cabin=economy
demo/flight-booking/deploy/send_command.sh cancel-reservation flight_id=JX100 passenger_document_id=P1234567 order_id=ORD-1
```

Seed a small flight schedule:

```bash
demo/flight-booking/deploy/seed_flights.sh
```

## Domain Scope

This demo models flight inventory, passenger identity, duplicate active
reservation prevention, cancellation inventory release, and flight retirement.
It intentionally does not model seat assignment, payment, ticketing, itinerary
changes, PNR multi-passenger orders, prices, connecting segments, oversell,
waitlists, or airline inventory synchronization.

A reservation record is keyed by `(flight_id, passenger_document_id)`. Cancel
does not delete the row; it flips status. Reserving again after cancel reuses
the row with a new `order_id`.

This demo treats `birth_date == 0` as invalid input. Passenger records are
immutable after creation; `nationality`, `birth_date`, and `document_type`
changes require an out-of-band data migration.

## Local Replay

`run_replayer.sh` is wired as the deploy entrypoint for `statevec-replay`:

```bash
demo/flight-booking/deploy/stop_eval.sh
demo/flight-booking/deploy/run_replayer.sh
demo/flight-booking/deploy/run_replayer.sh --to-tx-seq 1000
demo/flight-booking/deploy/run_replayer.sh --emit-state-deltas --emit-events
```

The script refuses to read the eval data directory while `statevec-eval` is
still running.

Example replay session after seeding flights and making a reservation:

```text
statevec-replay> status
{
  "committed_tx_seq": 11,
  "record_count": 12
}

statevec-replay> inspect-by-sysid 1
{
  "record_type": "Flight",
  "value": {
    "flight_id": {
      "utf8": "JX100"
    },
    "origin": {
      "utf8": "SIN"
    },
    "destination": {
      "utf8": "NRT"
    },
    "economy_reserved": 1,
    "economy_total": 120,
    "status": 1
  }
}
```

Replay is useful for deterministic debugging:

```bash
demo/flight-booking/deploy/run_replayer.sh --to-tx-seq 10
demo/flight-booking/deploy/run_replayer.sh --from-tx-seq 10 --emit-events
demo/flight-booking/deploy/run_replayer.sh --emit-state-deltas --emit-events
```

Use it to inspect committed `Flight`, `Passenger`, and `Reservation` records
without adding demo-specific query APIs.

## Stop / Reset

```bash
demo/flight-booking/deploy/stop_eval.sh
demo/flight-booking/deploy/stop_infra.sh
demo/flight-booking/deploy/reset_eval_env.sh
```

Generated files go under `run/`:

- data dir: `flight-booking-eval`
- rendered config: `flight_booking_eval.rendered.toml`
- pid/log files: `flight_booking_eval.*`
