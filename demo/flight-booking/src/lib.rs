// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

//! Flight booking demo: inventory-backed reservation and cancellation rules.

use statevec::event::GeneratedEventAccess;
use statevec::record::RecordSchema;
use statevec::{
    BizInvariantReadContext, CommandSchema, GeneratedCommandAccess, InvariantReadContextExt,
    RuntimeCommandEnvelope, RuntimeHostContext, RuntimePlugin, RuntimePluginError,
    RuntimePluginFactory, RuntimePluginLoadError, RuntimePluginUnloadError, TypedTxContext,
};
#[allow(unused_imports)]
use statevec::{statevec_api, statevec_model};

mod schema;
pub use schema::*;

pub const CABIN_ECONOMY: u8 = 1;
pub const CABIN_BUSINESS: u8 = 2;
pub const CABIN_FIRST: u8 = 3;

pub const RESULT_ACCEPTED: u8 = 1;
pub const RESULT_FLIGHT_NOT_FOUND: u8 = 2;
pub const RESULT_FLIGHT_NOT_ACTIVE: u8 = 3;
pub const RESULT_DUPLICATE_RESERVATION: u8 = 4;
pub const RESULT_CABIN_FULL: u8 = 5;
pub const RESULT_INVALID_PASSENGER: u8 = 6;
pub const RESULT_RESERVATION_NOT_FOUND: u8 = 7;
pub const RESULT_RESERVATION_NOT_ACTIVE: u8 = 8;
pub const RESULT_INVALID_CABIN: u8 = 9;

const FLIGHT_STATUS_ACTIVE: u8 = 1;
const FLIGHT_STATUS_RETIRED: u8 = 2;
const RESERVATION_STATUS_ACTIVE: u8 = 1;
const RESERVATION_STATUS_CANCELED: u8 = 2;

#[derive(Debug)]
pub enum FlightBookingError {
    Host(String),
    Message(String),
}

impl From<statevec::RuntimeHostError> for FlightBookingError {
    fn from(value: statevec::RuntimeHostError) -> Self {
        Self::Host(value.to_string())
    }
}

impl std::fmt::Display for FlightBookingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Host(err) => write!(f, "{err}"),
            Self::Message(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for FlightBookingError {}

pub(crate) struct FlightBookingRuntime;

#[derive(Clone, Copy)]
struct FlightSnapshot {
    status: u8,
    economy_total: u32,
    economy_reserved: u32,
    business_total: u32,
    business_reserved: u32,
    first_total: u32,
    first_reserved: u32,
}

#[derive(Clone, Copy)]
struct ReservationSnapshot {
    order_id: statevec::FixedBytes<32>,
    cabin_class: u8,
    status: u8,
}

impl FlightBookingRuntime {
    fn bad(msg: impl Into<String>) -> FlightBookingError {
        FlightBookingError::Message(msg.into())
    }

    fn emit_flight_added<Tx: TypedTxContext + ?Sized>(
        tx: &mut Tx,
        flight_id: statevec::FixedBytes<16>,
    ) {
        tx.emit_typed_event::<FlightAdded>(FlightAdded::builder().set_flight_id(flight_id).build());
    }

    fn emit_flight_removed<Tx: TypedTxContext + ?Sized>(
        tx: &mut Tx,
        flight_id: statevec::FixedBytes<16>,
    ) {
        tx.emit_typed_event::<FlightRemoved>(
            FlightRemoved::builder().set_flight_id(flight_id).build(),
        );
    }

    fn emit_reserve_result<Tx: TypedTxContext + ?Sized>(
        tx: &mut Tx,
        command: &ReserveOrderAccess<'_>,
        result_code: u8,
    ) {
        tx.emit_typed_event::<ReserveResult>(
            ReserveResult::builder()
                .set_flight_id(command.flight_id())
                .set_passenger_document_id(command.passenger_document_id())
                .set_order_id(command.order_id())
                .set_cabin_class(command.cabin_class())
                .set_result_code(result_code)
                .build(),
        );
    }

    fn emit_cancel_result<Tx: TypedTxContext + ?Sized>(
        tx: &mut Tx,
        flight_id: statevec::FixedBytes<16>,
        passenger_document_id: statevec::FixedBytes<32>,
        order_id: statevec::FixedBytes<32>,
        cabin_class: u8,
        result_code: u8,
    ) {
        tx.emit_typed_event::<CancelResult>(
            CancelResult::builder()
                .set_flight_id(flight_id)
                .set_passenger_document_id(passenger_document_id)
                .set_order_id(order_id)
                .set_cabin_class(cabin_class)
                .set_result_code(result_code)
                .build(),
        );
    }

    fn is_valid_cabin(cabin_class: u8) -> bool {
        matches!(cabin_class, CABIN_ECONOMY | CABIN_BUSINESS | CABIN_FIRST)
    }

    fn cabin_has_capacity(flight: FlightSnapshot, cabin_class: u8) -> bool {
        match cabin_class {
            CABIN_ECONOMY => flight.economy_reserved < flight.economy_total,
            CABIN_BUSINESS => flight.business_reserved < flight.business_total,
            CABIN_FIRST => flight.first_reserved < flight.first_total,
            _ => false,
        }
    }

    fn increment_cabin(
        flight: &mut UpdateFlightBuilder<'_>,
        cabin_class: u8,
    ) -> Result<(), FlightBookingError> {
        match cabin_class {
            CABIN_ECONOMY => flight.set_economy_reserved(
                flight
                    .economy_reserved()
                    .checked_add(1)
                    .ok_or_else(|| Self::bad("economy reserved overflow"))?,
            ),
            CABIN_BUSINESS => flight.set_business_reserved(
                flight
                    .business_reserved()
                    .checked_add(1)
                    .ok_or_else(|| Self::bad("business reserved overflow"))?,
            ),
            CABIN_FIRST => flight.set_first_reserved(
                flight
                    .first_reserved()
                    .checked_add(1)
                    .ok_or_else(|| Self::bad("first reserved overflow"))?,
            ),
            _ => return Err(Self::bad("invalid cabin class")),
        };
        Ok(())
    }

    fn decrement_cabin(
        flight: &mut UpdateFlightBuilder<'_>,
        cabin_class: u8,
    ) -> Result<(), FlightBookingError> {
        match cabin_class {
            CABIN_ECONOMY => flight.set_economy_reserved(
                flight
                    .economy_reserved()
                    .checked_sub(1)
                    .ok_or_else(|| Self::bad("economy reserved underflow"))?,
            ),
            CABIN_BUSINESS => flight.set_business_reserved(
                flight
                    .business_reserved()
                    .checked_sub(1)
                    .ok_or_else(|| Self::bad("business reserved underflow"))?,
            ),
            CABIN_FIRST => flight.set_first_reserved(
                flight
                    .first_reserved()
                    .checked_sub(1)
                    .ok_or_else(|| Self::bad("first reserved underflow"))?,
            ),
            _ => return Err(Self::bad("invalid cabin class")),
        };
        Ok(())
    }

    fn passenger_matches(
        passenger: PassengerAccess<'_>,
        nationality: statevec::FixedBytes<4>,
        birth_date: u32,
        document_type: u8,
    ) -> bool {
        passenger.nationality() == nationality
            && passenger.birth_date() == birth_date
            && passenger.document_type() == document_type
    }

    fn validate_passenger_input(command: &ReserveOrderAccess<'_>) -> bool {
        // This demo treats zero as "not provided" for date-like command fields.
        !command.passenger_document_id().is_empty()
            && !command.passenger_nationality().is_empty()
            && command.passenger_birth_date() != 0
            && command.passenger_document_type() != 0
    }

    fn handle_add_flight<Tx: TypedTxContext<Error: Into<FlightBookingError>> + ?Sized>(
        &self,
        tx: &mut Tx,
        command: AddFlightAccess<'_>,
    ) -> Result<(), FlightBookingError> {
        let flight_id = command.flight_id();
        if flight_id.is_empty() {
            return Err(Self::bad("flight_id must not be empty"));
        }
        if command.economy_total() == 0
            && command.business_total() == 0
            && command.first_total() == 0
        {
            return Err(Self::bad("at least one cabin capacity must be positive"));
        }
        if tx
            .with_read_typed_by_pk::<Flight, _, _, _>(Flight::pk(&flight_id), |_| ())
            .map_err(Into::into)?
            .is_some()
        {
            return Err(Self::bad("flight already exists"));
        }

        tx.create_typed::<Flight, _>(|flight| {
            flight.init_flight_id(&flight_id);
            flight.set_flight_date(command.flight_date());
            flight.set_origin(&command.origin());
            flight.set_destination(&command.destination());
            flight.set_airline(&command.airline());
            flight.set_aircraft_model(&command.aircraft_model());
            flight.set_status(FLIGHT_STATUS_ACTIVE);
            flight.set_economy_total(command.economy_total());
            flight.set_economy_reserved(0);
            flight.set_business_total(command.business_total());
            flight.set_business_reserved(0);
            flight.set_first_total(command.first_total());
            flight.set_first_reserved(0);
        })
        .map_err(Into::into)?;
        Self::emit_flight_added(tx, flight_id);
        Ok(())
    }

    fn handle_retire_flight<Tx: TypedTxContext<Error: Into<FlightBookingError>> + ?Sized>(
        &self,
        tx: &mut Tx,
        command: RetireFlightAccess<'_>,
    ) -> Result<(), FlightBookingError> {
        let flight_id = command.flight_id();
        let retired = tx
            .update_typed_by_pk::<Flight, _, _, _>(Flight::pk(&flight_id), |flight| {
                flight.set_status(FLIGHT_STATUS_RETIRED);
            })
            .map_err(Into::into)?;
        if retired.is_none() {
            return Err(Self::bad("flight not found"));
        }
        Self::emit_flight_removed(tx, flight_id);
        Ok(())
    }

    fn reserve_failure_code<Tx: TypedTxContext<Error: Into<FlightBookingError>> + ?Sized>(
        &self,
        tx: &Tx,
        command: &ReserveOrderAccess<'_>,
    ) -> Result<Option<u8>, FlightBookingError> {
        if !Self::validate_passenger_input(command) {
            return Ok(Some(RESULT_INVALID_PASSENGER));
        }

        let passenger_valid = tx
            .with_read_typed_by_pk::<Passenger, _, _, _>(
                Passenger::pk(&command.passenger_document_id()),
                |passenger| {
                    Self::passenger_matches(
                        passenger,
                        command.passenger_nationality(),
                        command.passenger_birth_date(),
                        command.passenger_document_type(),
                    )
                },
            )
            .map_err(Into::into)?;
        if passenger_valid == Some(false) {
            return Ok(Some(RESULT_INVALID_PASSENGER));
        }

        let Some(flight) = tx
            .with_read_typed_by_pk::<Flight, _, _, _>(Flight::pk(&command.flight_id()), |flight| {
                FlightSnapshot {
                    status: flight.status(),
                    economy_total: flight.economy_total(),
                    economy_reserved: flight.economy_reserved(),
                    business_total: flight.business_total(),
                    business_reserved: flight.business_reserved(),
                    first_total: flight.first_total(),
                    first_reserved: flight.first_reserved(),
                }
            })
            .map_err(Into::into)?
        else {
            return Ok(Some(RESULT_FLIGHT_NOT_FOUND));
        };
        if flight.status != FLIGHT_STATUS_ACTIVE {
            return Ok(Some(RESULT_FLIGHT_NOT_ACTIVE));
        }

        if !Self::is_valid_cabin(command.cabin_class()) {
            return Ok(Some(RESULT_INVALID_CABIN));
        }

        let duplicate = tx
            .with_read_typed_by_pk::<Reservation, _, _, _>(
                Reservation::pk(&command.flight_id(), &command.passenger_document_id()),
                |reservation| reservation.status() == RESERVATION_STATUS_ACTIVE,
            )
            .map_err(Into::into)?
            .unwrap_or(false);
        if duplicate {
            return Ok(Some(RESULT_DUPLICATE_RESERVATION));
        }

        if !Self::cabin_has_capacity(flight, command.cabin_class()) {
            return Ok(Some(RESULT_CABIN_FULL));
        }

        Ok(None)
    }

    fn handle_reserve_order<Tx: TypedTxContext<Error: Into<FlightBookingError>> + ?Sized>(
        &self,
        tx: &mut Tx,
        command: ReserveOrderAccess<'_>,
        ref_time_us: u64,
    ) -> Result<(), FlightBookingError> {
        if let Some(result_code) = self.reserve_failure_code(tx, &command)? {
            Self::emit_reserve_result(tx, &command, result_code);
            return Ok(());
        }

        let passenger_document_id = command.passenger_document_id();
        let flight_id = command.flight_id();
        let order_id = command.order_id();
        let cabin_class = command.cabin_class();

        if tx
            .with_read_typed_by_pk::<Passenger, _, _, _>(
                Passenger::pk(&passenger_document_id),
                |_| (),
            )
            .map_err(Into::into)?
            .is_none()
        {
            tx.create_typed::<Passenger, _>(|passenger| {
                passenger.init_document_id(&passenger_document_id);
                passenger.set_nationality(&command.passenger_nationality());
                passenger.set_birth_date(command.passenger_birth_date());
                passenger.set_document_type(command.passenger_document_type());
            })
            .map_err(Into::into)?;
        }

        tx.update_typed_by_pk::<Flight, _, _, _>(Flight::pk(&flight_id), |flight| {
            Self::increment_cabin(flight, cabin_class)
        })
        .map_err(Into::into)?
        .ok_or_else(|| Self::bad("flight disappeared during reserve"))??;

        tx.update_or_create_typed_by_pk::<Reservation, _, _, _, _>(
            Reservation::pk(&flight_id, &passenger_document_id),
            |reservation| {
                reservation.set_order_id(&order_id);
                reservation.set_cabin_class(cabin_class);
                reservation.set_status(RESERVATION_STATUS_ACTIVE);
                reservation.set_reserved_at_ref_time_us(ref_time_us);
            },
            |reservation| {
                reservation.init_flight_id(&flight_id);
                reservation.init_passenger_document_id(&passenger_document_id);
                reservation.set_order_id(&order_id);
                reservation.set_cabin_class(cabin_class);
                reservation.set_status(RESERVATION_STATUS_ACTIVE);
                reservation.set_reserved_at_ref_time_us(ref_time_us);
            },
        )
        .map_err(Into::into)?;

        Self::emit_reserve_result(tx, &command, RESULT_ACCEPTED);
        Ok(())
    }

    fn handle_cancel_reservation<Tx: TypedTxContext<Error: Into<FlightBookingError>> + ?Sized>(
        &self,
        tx: &mut Tx,
        command: CancelReservationAccess<'_>,
    ) -> Result<(), FlightBookingError> {
        let flight_id = command.flight_id();
        let passenger_document_id = command.passenger_document_id();
        let command_order_id = command.order_id();

        let Some(reservation) = tx
            .with_read_typed_by_pk::<Reservation, _, _, _>(
                Reservation::pk(&flight_id, &passenger_document_id),
                |reservation| ReservationSnapshot {
                    order_id: reservation.order_id(),
                    cabin_class: reservation.cabin_class(),
                    status: reservation.status(),
                },
            )
            .map_err(Into::into)?
        else {
            Self::emit_cancel_result(
                tx,
                flight_id,
                passenger_document_id,
                command_order_id,
                0,
                RESULT_RESERVATION_NOT_FOUND,
            );
            return Ok(());
        };

        if reservation.status != RESERVATION_STATUS_ACTIVE {
            Self::emit_cancel_result(
                tx,
                flight_id,
                passenger_document_id,
                reservation.order_id,
                reservation.cabin_class,
                RESULT_RESERVATION_NOT_ACTIVE,
            );
            return Ok(());
        }

        if !Self::is_valid_cabin(reservation.cabin_class) {
            Self::emit_cancel_result(
                tx,
                flight_id,
                passenger_document_id,
                reservation.order_id,
                reservation.cabin_class,
                RESULT_INVALID_CABIN,
            );
            return Ok(());
        }

        tx.update_typed_by_pk::<Flight, _, _, _>(Flight::pk(&flight_id), |flight| {
            Self::decrement_cabin(flight, reservation.cabin_class)
        })
        .map_err(Into::into)?
        .ok_or_else(|| Self::bad("reservation references missing flight"))??;

        tx.update_typed_by_pk::<Reservation, _, _, _>(
            Reservation::pk(&flight_id, &passenger_document_id),
            |reservation| {
                reservation.set_status(RESERVATION_STATUS_CANCELED);
            },
        )
        .map_err(Into::into)?
        .ok_or_else(|| Self::bad("reservation disappeared during cancel"))?;

        Self::emit_cancel_result(
            tx,
            flight_id,
            passenger_document_id,
            reservation.order_id,
            reservation.cabin_class,
            RESULT_ACCEPTED,
        );
        Ok(())
    }

    fn dispatch<Tx: TypedTxContext<Error: Into<FlightBookingError>> + ?Sized>(
        &self,
        tx: &mut Tx,
        command: &dyn RuntimeCommandEnvelope,
    ) -> Result<bool, FlightBookingError> {
        // command_dispatch! currently passes only the typed payload access. This
        // demo also needs ref_ext_time_us from the envelope for ReserveOrder.
        match command.command_kind() {
            AddFlight::KIND => {
                self.handle_add_flight(tx, AddFlight::wrap(command.payload()))?;
                Ok(true)
            }
            RetireFlight::KIND => {
                self.handle_retire_flight(tx, RetireFlight::wrap(command.payload()))?;
                Ok(true)
            }
            ReserveOrder::KIND => {
                self.handle_reserve_order(
                    tx,
                    ReserveOrder::wrap(command.payload()),
                    command.ref_ext_time_us(),
                )?;
                Ok(true)
            }
            CancelReservation::KIND => {
                self.handle_cancel_reservation(tx, CancelReservation::wrap(command.payload()))?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }
}

impl RuntimePlugin for FlightBookingRuntime {
    fn name(&self) -> &'static str {
        "flight-booking"
    }

    fn schema_registry(&self) -> statevec::SchemaRegistry {
        registry()
    }

    fn run_tx(
        &self,
        tx: &mut dyn RuntimeHostContext,
        command: &dyn RuntimeCommandEnvelope,
    ) -> Result<(), RuntimePluginError> {
        if self
            .dispatch(tx, command)
            .map_err(|e| RuntimePluginError::new(e.to_string()))?
        {
            return Ok(());
        }
        Err(RuntimePluginError::new(format!(
            "unknown command kind {}",
            command.command_kind()
        )))
    }

    fn validate_biz_invariants(&self, ctx: &dyn BizInvariantReadContext) -> Result<(), String> {
        let mut flight_keys = Vec::new();
        ctx.for_each_record_key_raw(Flight::KIND, &mut |key| flight_keys.push(key))
            .map_err(|e| e.to_string())?;

        let mut flight_counters = Vec::new();
        for key in flight_keys {
            let Some((
                flight_id,
                economy_total,
                economy_reserved,
                business_total,
                business_reserved,
                first_total,
                first_reserved,
            )) = ctx
                .with_read_typed::<Flight, _, _>(key.sys_id, |flight| {
                    (
                        flight.flight_id(),
                        flight.economy_total(),
                        flight.economy_reserved(),
                        flight.business_total(),
                        flight.business_reserved(),
                        flight.first_total(),
                        flight.first_reserved(),
                    )
                })
                .map_err(|e| e.to_string())?
            else {
                continue;
            };

            if economy_reserved > economy_total {
                return Err(format!(
                    "flight {:?} economy reserved exceeds total",
                    flight_id
                ));
            }
            if business_reserved > business_total {
                return Err(format!(
                    "flight {:?} business reserved exceeds total",
                    flight_id
                ));
            }
            if first_reserved > first_total {
                return Err(format!(
                    "flight {:?} first reserved exceeds total",
                    flight_id
                ));
            }
            flight_counters.push((
                flight_id,
                economy_reserved,
                business_reserved,
                first_reserved,
            ));
        }

        let mut reservation_keys = Vec::new();
        ctx.for_each_record_key_raw(Reservation::KIND, &mut |key| reservation_keys.push(key))
            .map_err(|e| e.to_string())?;

        let mut aggregates: Vec<(statevec::FixedBytes<16>, u8, u32)> = Vec::new();
        for key in reservation_keys {
            let Some((flight_id, passenger_document_id, cabin_class, status)) = ctx
                .with_read_typed::<Reservation, _, _>(key.sys_id, |reservation| {
                    (
                        reservation.flight_id(),
                        reservation.passenger_document_id(),
                        reservation.cabin_class(),
                        reservation.status(),
                    )
                })
                .map_err(|e| e.to_string())?
            else {
                continue;
            };

            if status != RESERVATION_STATUS_ACTIVE {
                continue;
            }
            if ctx
                .with_read_typed_by_pk::<Flight, _, _, _>(Flight::pk(&flight_id), |_| ())
                .map_err(|e| e.to_string())?
                .is_none()
            {
                return Err(format!(
                    "active reservation references missing flight {:?}",
                    flight_id
                ));
            }
            if ctx
                .with_read_typed_by_pk::<Passenger, _, _, _>(
                    Passenger::pk(&passenger_document_id),
                    |_| (),
                )
                .map_err(|e| e.to_string())?
                .is_none()
            {
                return Err(format!(
                    "active reservation references missing passenger {:?}",
                    passenger_document_id
                ));
            }

            if let Some((_, _, count)) = aggregates
                .iter_mut()
                .find(|(id, cabin, _)| *id == flight_id && *cabin == cabin_class)
            {
                *count = count
                    .checked_add(1)
                    .ok_or_else(|| "reservation aggregate overflow".to_string())?;
            } else {
                aggregates.push((flight_id, cabin_class, 1));
            }
        }

        for (flight_id, cabin_class, count) in &aggregates {
            let Some(expected) = ctx
                .with_read_typed_by_pk::<Flight, _, _, _>(Flight::pk(flight_id), |flight| {
                    match *cabin_class {
                        CABIN_ECONOMY => Some(flight.economy_reserved()),
                        CABIN_BUSINESS => Some(flight.business_reserved()),
                        CABIN_FIRST => Some(flight.first_reserved()),
                        _ => None,
                    }
                })
                .map_err(|e| e.to_string())?
                .flatten()
            else {
                return Err(format!(
                    "active reservation has invalid cabin class {cabin_class}"
                ));
            };

            if *count != expected {
                return Err(format!(
                    "flight {:?} cabin {cabin_class} reservation aggregate {count} != reserved counter {expected}",
                    flight_id
                ));
            }
        }

        for (flight_id, economy_reserved, business_reserved, first_reserved) in flight_counters {
            let economy_count = aggregates
                .iter()
                .find(|(id, cabin, _)| *id == flight_id && *cabin == CABIN_ECONOMY)
                .map(|(_, _, count)| *count)
                .unwrap_or(0);
            if economy_count != economy_reserved {
                return Err(format!(
                    "flight {:?} cabin {CABIN_ECONOMY} reservation aggregate {economy_count} != reserved counter {economy_reserved}",
                    flight_id
                ));
            }

            let business_count = aggregates
                .iter()
                .find(|(id, cabin, _)| *id == flight_id && *cabin == CABIN_BUSINESS)
                .map(|(_, _, count)| *count)
                .unwrap_or(0);
            if business_count != business_reserved {
                return Err(format!(
                    "flight {:?} cabin {CABIN_BUSINESS} reservation aggregate {business_count} != reserved counter {business_reserved}",
                    flight_id
                ));
            }

            let first_count = aggregates
                .iter()
                .find(|(id, cabin, _)| *id == flight_id && *cabin == CABIN_FIRST)
                .map(|(_, _, count)| *count)
                .unwrap_or(0);
            if first_count != first_reserved {
                return Err(format!(
                    "flight {:?} cabin {CABIN_FIRST} reservation aggregate {first_count} != reserved counter {first_reserved}",
                    flight_id
                ));
            }
        }

        Ok(())
    }

    fn on_unload(&mut self) -> Result<(), RuntimePluginUnloadError> {
        Ok(())
    }
}

struct FlightBookingRuntimeFactory;

impl RuntimePluginFactory for FlightBookingRuntimeFactory {
    fn plugin_name(&self) -> &'static str {
        "flight-booking"
    }

    fn schema_registry(&self) -> statevec::SchemaRegistry {
        registry()
    }

    fn create(
        &self,
        _plugin_config_text: &str,
    ) -> Result<Box<dyn RuntimePlugin>, RuntimePluginLoadError> {
        Ok(Box::new(FlightBookingRuntime))
    }
}

statevec::export_runtime_plugin!(Box::new(FlightBookingRuntimeFactory));

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    use statevec::{
        EventSchema, FixedBytes, GeneratedRecordAccess, PkCodec, RecordKey, RuntimeHostContextExt,
        RuntimeHostError,
    };

    #[derive(Default)]
    struct MockRuntimeHostContext {
        next_sys_id: u64,
        records: BTreeMap<(u8, u64), Vec<u8>>,
        events: Vec<(u8, Vec<u8>)>,
    }

    impl MockRuntimeHostContext {
        fn new() -> Self {
            Self {
                next_sys_id: 1,
                ..Self::default()
            }
        }

        fn data_len(record_kind: u8) -> Result<usize, RuntimeHostError> {
            match record_kind {
                Flight::KIND => Ok(Flight::DATA_LEN),
                Passenger::KIND => Ok(Passenger::DATA_LEN),
                Reservation::KIND => Ok(Reservation::DATA_LEN),
                _ => Err(RuntimeHostError::new("unexpected record kind")),
            }
        }

        fn encode_pk(record_kind: u8, data: &[u8]) -> Result<Vec<u8>, RuntimeHostError> {
            match record_kind {
                Flight::KIND => Ok(Flight::encode_pk_from_bytes(data).to_vec()),
                Passenger::KIND => Ok(Passenger::encode_pk_from_bytes(data).to_vec()),
                Reservation::KIND => Ok(Reservation::encode_pk_from_bytes(data).to_vec()),
                _ => Err(RuntimeHostError::new("unexpected record kind")),
            }
        }

        fn record_mut_by_pk(
            &mut self,
            record_kind: u8,
            pk: &[u8],
        ) -> Result<Option<&mut Vec<u8>>, RuntimeHostError> {
            let mut found = None;
            for ((kind, sys_id), data) in &self.records {
                if *kind == record_kind && Self::encode_pk(record_kind, data)? == pk {
                    found = Some((*kind, *sys_id));
                    break;
                }
            }
            Ok(found.and_then(|key| self.records.get_mut(&key)))
        }

        fn record_by_pk(
            &self,
            record_kind: u8,
            pk: &[u8],
        ) -> Result<Option<&Vec<u8>>, RuntimeHostError> {
            for ((kind, _), data) in &self.records {
                if *kind == record_kind && Self::encode_pk(record_kind, data)? == pk {
                    return Ok(Some(data));
                }
            }
            Ok(None)
        }

        fn last_reserve_result(&self) -> event::ReserveResultAccess<'_> {
            let (kind, payload) = self.events.last().expect("missing event");
            assert_eq!(*kind, ReserveResult::KIND);
            event::ReserveResultAccess::new(payload)
        }

        fn last_cancel_result(&self) -> event::CancelResultAccess<'_> {
            let (kind, payload) = self.events.last().expect("missing event");
            assert_eq!(*kind, CancelResult::KIND);
            event::CancelResultAccess::new(payload)
        }
    }

    impl RuntimeHostContext for MockRuntimeHostContext {
        fn with_read_typed_raw(
            &self,
            record_kind: u8,
            sys_id: u64,
            f: &mut dyn FnMut(&[u8]),
        ) -> Result<bool, RuntimeHostError> {
            let Some(data) = self.records.get(&(record_kind, sys_id)) else {
                return Ok(false);
            };
            f(data);
            Ok(true)
        }

        fn with_read_typed_by_pk_raw(
            &self,
            record_kind: u8,
            pk: &[u8],
            f: &mut dyn FnMut(&[u8]),
        ) -> Result<bool, RuntimeHostError> {
            let Some(data) = self.record_by_pk(record_kind, pk)? else {
                return Ok(false);
            };
            f(data);
            Ok(true)
        }

        fn create_typed_raw(
            &mut self,
            record_kind: u8,
            init: &mut dyn FnMut(&mut [u8]),
        ) -> Result<RecordKey, RuntimeHostError> {
            let sys_id = self.next_sys_id;
            self.next_sys_id += 1;

            let mut data = vec![0u8; Self::data_len(record_kind)?];
            init(&mut data);
            self.records.insert((record_kind, sys_id), data);

            Ok(RecordKey {
                kind: record_kind,
                sys_id,
            })
        }

        fn update_typed_by_pk_raw(
            &mut self,
            record_kind: u8,
            pk: &[u8],
            f: &mut dyn FnMut(&mut [u8]),
        ) -> Result<bool, RuntimeHostError> {
            let Some(data) = self.record_mut_by_pk(record_kind, pk)? else {
                return Ok(false);
            };
            f(data);
            Ok(true)
        }

        fn delete_by_pk_raw(
            &mut self,
            record_kind: u8,
            pk: &[u8],
        ) -> Result<bool, RuntimeHostError> {
            let Some((&key, _)) = self.records.iter().find(|((kind, _), data)| {
                *kind == record_kind
                    && Self::encode_pk(record_kind, data).is_ok_and(|encoded| encoded == pk)
            }) else {
                return Ok(false);
            };
            self.records.remove(&key);
            Ok(true)
        }

        fn emit_typed_event_raw(
            &mut self,
            event_kind: u8,
            payload: &[u8],
        ) -> Result<(), RuntimeHostError> {
            self.events.push((event_kind, payload.to_vec()));
            Ok(())
        }

        fn for_each_record_key_raw(
            &self,
            kind: u8,
            f: &mut dyn FnMut(RecordKey),
        ) -> Result<(), RuntimeHostError> {
            for (record_kind, sys_id) in self
                .records
                .keys()
                .filter(|(record_kind, _)| *record_kind == kind)
            {
                f(RecordKey {
                    kind: *record_kind,
                    sys_id: *sys_id,
                });
            }
            Ok(())
        }
    }

    impl BizInvariantReadContext for MockRuntimeHostContext {
        fn with_read_typed_raw(
            &self,
            record_kind: u8,
            sys_id: u64,
            f: &mut dyn FnMut(&[u8]),
        ) -> Result<bool, RuntimeHostError> {
            RuntimeHostContext::with_read_typed_raw(self, record_kind, sys_id, f)
        }

        fn with_read_typed_by_pk_raw(
            &self,
            record_kind: u8,
            pk: &[u8],
            f: &mut dyn FnMut(&[u8]),
        ) -> Result<bool, RuntimeHostError> {
            RuntimeHostContext::with_read_typed_by_pk_raw(self, record_kind, pk, f)
        }

        fn for_each_record_key_raw(
            &self,
            kind: u8,
            f: &mut dyn FnMut(RecordKey),
        ) -> Result<(), RuntimeHostError> {
            RuntimeHostContext::for_each_record_key_raw(self, kind, f)
        }
    }

    fn fb<const N: usize>(bytes: &[u8]) -> FixedBytes<N> {
        FixedBytes::<N>::new(bytes).unwrap()
    }

    fn flight_id() -> FixedBytes<16> {
        fb(b"JX100")
    }

    fn passenger_id() -> FixedBytes<32> {
        fb(b"P1234567")
    }

    fn order_id(bytes: &[u8]) -> FixedBytes<32> {
        fb(bytes)
    }

    fn add_flight_payload(economy_total: u32) -> Vec<u8> {
        AddFlight::builder()
            .set_flight_id(flight_id())
            .set_flight_date(20260503)
            .set_origin(fb(b"SIN"))
            .set_destination(fb(b"NRT"))
            .set_airline(fb(b"JX"))
            .set_aircraft_model(fb(b"A350"))
            .set_economy_total(economy_total)
            .set_business_total(1)
            .set_first_total(1)
            .build()
    }

    fn reserve_payload(order: &[u8], cabin_class: u8) -> Vec<u8> {
        ReserveOrder::builder()
            .set_flight_id(flight_id())
            .set_order_id(order_id(order))
            .set_passenger_document_id(passenger_id())
            .set_passenger_nationality(fb(b"SG"))
            .set_passenger_birth_date(19900101)
            .set_passenger_document_type(1)
            .set_cabin_class(cabin_class)
            .build()
    }

    fn cancel_payload(order: &[u8]) -> Vec<u8> {
        CancelReservation::builder()
            .set_flight_id(flight_id())
            .set_passenger_document_id(passenger_id())
            .set_order_id(order_id(order))
            .build()
    }

    fn add_flight(host: &mut MockRuntimeHostContext, economy_total: u32) {
        let runtime = FlightBookingRuntime;
        let payload = add_flight_payload(economy_total);
        let tx = host as &mut dyn RuntimeHostContext;
        runtime
            .handle_add_flight(tx, AddFlight::wrap(&payload))
            .expect("add flight should succeed");
    }

    fn reserve(host: &mut MockRuntimeHostContext, order: &[u8], cabin_class: u8) {
        let runtime = FlightBookingRuntime;
        let payload = reserve_payload(order, cabin_class);
        let tx = host as &mut dyn RuntimeHostContext;
        runtime
            .handle_reserve_order(tx, ReserveOrder::wrap(&payload), 1234)
            .expect("reserve command should not return plugin error");
    }

    fn cancel(host: &mut MockRuntimeHostContext, order: &[u8]) {
        let runtime = FlightBookingRuntime;
        let payload = cancel_payload(order);
        let tx = host as &mut dyn RuntimeHostContext;
        runtime
            .handle_cancel_reservation(tx, CancelReservation::wrap(&payload))
            .expect("cancel command should not return plugin error");
    }

    fn reserved_economy(host: &MockRuntimeHostContext) -> u32 {
        RuntimeHostContextExt::with_read_typed_by_pk::<Flight, _, _, _>(
            host,
            Flight::pk(&flight_id()),
            |flight| flight.economy_reserved(),
        )
        .unwrap()
        .unwrap()
    }

    #[test]
    fn add_flight_success_emits_event() {
        let mut host = MockRuntimeHostContext::new();
        add_flight(&mut host, 2);

        assert_eq!(host.events.len(), 1);
        assert_eq!(host.events[0].0, FlightAdded::KIND);
        assert_eq!(reserved_economy(&host), 0);
    }

    #[test]
    fn duplicate_add_flight_is_plugin_error() {
        let mut host = MockRuntimeHostContext::new();
        add_flight(&mut host, 2);

        let runtime = FlightBookingRuntime;
        let payload = add_flight_payload(2);
        let tx = &mut host as &mut dyn RuntimeHostContext;
        let err = runtime
            .handle_add_flight(tx, AddFlight::wrap(&payload))
            .expect_err("duplicate add should fail");
        assert_eq!(err.to_string(), "flight already exists");
    }

    #[test]
    fn reserve_success_updates_inventory_and_emits_accepted() {
        let mut host = MockRuntimeHostContext::new();
        add_flight(&mut host, 2);
        reserve(&mut host, b"ORD-1", CABIN_ECONOMY);

        assert_eq!(reserved_economy(&host), 1);
        assert_eq!(host.last_reserve_result().result_code(), RESULT_ACCEPTED);
    }

    #[test]
    fn duplicate_reserve_emits_failed_result_without_inventory_change() {
        let mut host = MockRuntimeHostContext::new();
        add_flight(&mut host, 2);
        reserve(&mut host, b"ORD-1", CABIN_ECONOMY);
        reserve(&mut host, b"ORD-2", CABIN_ECONOMY);

        assert_eq!(reserved_economy(&host), 1);
        assert_eq!(
            host.last_reserve_result().result_code(),
            RESULT_DUPLICATE_RESERVATION
        );
    }

    #[test]
    fn cabin_full_emits_failed_result() {
        let mut host = MockRuntimeHostContext::new();
        add_flight(&mut host, 1);
        reserve(&mut host, b"ORD-1", CABIN_ECONOMY);

        let other_passenger = fb(b"P7654321");
        let payload = ReserveOrder::builder()
            .set_flight_id(flight_id())
            .set_order_id(order_id(b"ORD-2"))
            .set_passenger_document_id(other_passenger)
            .set_passenger_nationality(fb(b"SG"))
            .set_passenger_birth_date(19900101)
            .set_passenger_document_type(1)
            .set_cabin_class(CABIN_ECONOMY)
            .build();
        let tx = &mut host as &mut dyn RuntimeHostContext;
        FlightBookingRuntime
            .handle_reserve_order(tx, ReserveOrder::wrap(&payload), 1234)
            .unwrap();

        assert_eq!(reserved_economy(&host), 1);
        assert_eq!(host.last_reserve_result().result_code(), RESULT_CABIN_FULL);
    }

    #[test]
    fn retire_flight_prevents_future_reserve() {
        let mut host = MockRuntimeHostContext::new();
        add_flight(&mut host, 2);

        let retire_payload = RetireFlight::builder().set_flight_id(flight_id()).build();
        let tx = &mut host as &mut dyn RuntimeHostContext;
        FlightBookingRuntime
            .handle_retire_flight(tx, RetireFlight::wrap(&retire_payload))
            .unwrap();

        reserve(&mut host, b"ORD-1", CABIN_ECONOMY);
        assert_eq!(reserved_economy(&host), 0);
        assert_eq!(
            host.last_reserve_result().result_code(),
            RESULT_FLIGHT_NOT_ACTIVE
        );
    }

    #[test]
    fn cancel_active_reservation_releases_inventory() {
        let mut host = MockRuntimeHostContext::new();
        add_flight(&mut host, 2);
        reserve(&mut host, b"ORD-1", CABIN_ECONOMY);
        cancel(&mut host, b"ORD-1");

        assert_eq!(reserved_economy(&host), 0);
        assert_eq!(host.last_cancel_result().result_code(), RESULT_ACCEPTED);
    }

    #[test]
    fn reserve_again_after_cancel_reuses_reservation() {
        let mut host = MockRuntimeHostContext::new();
        add_flight(&mut host, 2);
        reserve(&mut host, b"ORD-1", CABIN_ECONOMY);
        cancel(&mut host, b"ORD-1");
        reserve(&mut host, b"ORD-2", CABIN_ECONOMY);

        assert_eq!(reserved_economy(&host), 1);
        assert_eq!(host.last_reserve_result().result_code(), RESULT_ACCEPTED);
    }

    #[test]
    fn invariant_validation_catches_inventory_reservation_mismatch() {
        let mut host = MockRuntimeHostContext::new();
        add_flight(&mut host, 2);
        reserve(&mut host, b"ORD-1", CABIN_ECONOMY);

        RuntimeHostContextExt::update_typed_by_pk::<Flight, _, _, _>(
            &mut host,
            Flight::pk(&flight_id()),
            |flight| {
                flight.set_economy_reserved(0);
            },
        )
        .unwrap();

        let err = FlightBookingRuntime
            .validate_biz_invariants(&host)
            .expect_err("mismatch should be rejected");
        assert!(err.contains("reservation aggregate"));
    }
}
