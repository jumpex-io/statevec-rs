// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

use std::collections::BTreeMap;

use flight_booking::{
    AddFlight, CABIN_BUSINESS, CABIN_ECONOMY, CABIN_FIRST, CancelReservation, ReserveOrder,
    RetireFlight,
};
use statevec::{
    CommandSchema, FixedBytes, GeneratedCommandAccess, api::SubmitRequest, encode_submit_request,
};

fn main() {
    match run() {
        Ok(hex) => println!("{hex}"),
        Err(err) => {
            eprintln!("{err}");
            usage();
            std::process::exit(2);
        }
    }
}

fn run() -> Result<String, String> {
    let mut args = std::env::args().skip(1);
    let Some(command) = args.next() else {
        return Err("missing command name".to_string());
    };
    if command == "-h" || command == "--help" {
        usage();
        std::process::exit(0);
    }

    let fields = parse_fields(args)?;
    let (kind, payload) = match command.as_str() {
        "add-flight" => (AddFlight::KIND, add_flight(&fields)?),
        "retire-flight" => (RetireFlight::KIND, retire_flight(&fields)?),
        "reserve-order" => (ReserveOrder::KIND, reserve_order(&fields)?),
        "cancel-reservation" => (CancelReservation::KIND, cancel_reservation(&fields)?),
        other => return Err(format!("unknown command: {other}")),
    };

    let encoded = encode_submit_request(&SubmitRequest::new(kind, payload))
        .map_err(|err| format!("failed to encode submit request: {err}"))?;
    Ok(hex_encode(&encoded))
}

fn parse_fields(args: impl Iterator<Item = String>) -> Result<BTreeMap<String, String>, String> {
    let mut fields = BTreeMap::new();
    for arg in args {
        let Some((key, value)) = arg.split_once('=') else {
            return Err(format!("expected key=value argument, got: {arg}"));
        };
        if key.is_empty() {
            return Err(format!("empty key in argument: {arg}"));
        }
        fields.insert(key.to_string(), value.to_string());
    }
    Ok(fields)
}

fn add_flight(fields: &BTreeMap<String, String>) -> Result<Vec<u8>, String> {
    Ok(AddFlight::builder()
        .set_flight_id(fixed::<16>(fields, "flight_id")?)
        .set_flight_date(u32_field(fields, "flight_date")?)
        .set_origin(fixed::<4>(fields, "origin")?)
        .set_destination(fixed::<4>(fields, "destination")?)
        .set_airline(fixed::<8>(fields, "airline")?)
        .set_aircraft_model(fixed::<16>(fields, "aircraft_model")?)
        .set_economy_total(u32_field(fields, "economy_total")?)
        .set_business_total(u32_field(fields, "business_total")?)
        .set_first_total(u32_field(fields, "first_total")?)
        .build())
}

fn retire_flight(fields: &BTreeMap<String, String>) -> Result<Vec<u8>, String> {
    Ok(RetireFlight::builder()
        .set_flight_id(fixed::<16>(fields, "flight_id")?)
        .build())
}

fn reserve_order(fields: &BTreeMap<String, String>) -> Result<Vec<u8>, String> {
    Ok(ReserveOrder::builder()
        .set_flight_id(fixed::<16>(fields, "flight_id")?)
        .set_order_id(fixed::<32>(fields, "order_id")?)
        .set_passenger_document_id(fixed::<32>(fields, "passenger_document_id")?)
        .set_passenger_nationality(fixed::<4>(fields, "nationality")?)
        .set_passenger_birth_date(u32_field(fields, "birth_date")?)
        .set_passenger_document_type(u8_field(fields, "document_type")?)
        .set_cabin_class(cabin_class(fields)?)
        .build())
}

fn cancel_reservation(fields: &BTreeMap<String, String>) -> Result<Vec<u8>, String> {
    Ok(CancelReservation::builder()
        .set_flight_id(fixed::<16>(fields, "flight_id")?)
        .set_passenger_document_id(fixed::<32>(fields, "passenger_document_id")?)
        .set_order_id(fixed::<32>(fields, "order_id")?)
        .build())
}

fn field<'a>(fields: &'a BTreeMap<String, String>, key: &str) -> Result<&'a str, String> {
    fields
        .get(key)
        .map(String::as_str)
        .ok_or_else(|| format!("missing required field: {key}"))
}

fn fixed<const N: usize>(
    fields: &BTreeMap<String, String>,
    key: &str,
) -> Result<FixedBytes<N>, String> {
    let value = field(fields, key)?;
    FixedBytes::<N>::new(value.as_bytes())
        .map_err(|_| format!("{key} exceeds FixedBytes<{N}> capacity"))
}

fn u32_field(fields: &BTreeMap<String, String>, key: &str) -> Result<u32, String> {
    field(fields, key)?
        .parse::<u32>()
        .map_err(|err| format!("invalid u32 field {key}: {err}"))
}

fn u8_field(fields: &BTreeMap<String, String>, key: &str) -> Result<u8, String> {
    field(fields, key)?
        .parse::<u8>()
        .map_err(|err| format!("invalid u8 field {key}: {err}"))
}

fn cabin_class(fields: &BTreeMap<String, String>) -> Result<u8, String> {
    match field(fields, "cabin")? {
        "economy" | "Economy" | "1" => Ok(CABIN_ECONOMY),
        "business" | "Business" | "2" => Ok(CABIN_BUSINESS),
        "first" | "First" | "1st" | "3" => Ok(CABIN_FIRST),
        value => Err(format!("invalid cabin: {value}")),
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn usage() {
    eprintln!(
        "usage:
  flight-booking-command add-flight flight_id=JX100 flight_date=20260503 origin=SIN destination=NRT airline=JX aircraft_model=A350 economy_total=2 business_total=1 first_total=1
  flight-booking-command reserve-order flight_id=JX100 order_id=ORD-1 passenger_document_id=P1234567 nationality=SG birth_date=19900101 document_type=1 cabin=economy
  flight-booking-command cancel-reservation flight_id=JX100 passenger_document_id=P1234567 order_id=ORD-1
  flight-booking-command retire-flight flight_id=JX100"
    );
}
