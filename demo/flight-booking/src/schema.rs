// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

#[allow(unused_imports)]
use super::*;
use statevec::schema_module;
#[allow(unused_imports)]
use statevec::{statevec_api, statevec_model};

mod flight_booking {
    use super::*;
    use statevec::{FixedBytes, command, event, record};

    #[schema_module(version = "1.0")]
    pub mod v1_0 {
        use super::*;

        #[record(kind = 1, record_len = 128, uk(id = 0, fields = [flight_id]))]
        pub struct Flight {
            #[field(index = 1, immutable = true)]
            pub flight_id: FixedBytes<16>,
            #[field(index = 2)]
            pub flight_date: u32,
            #[field(index = 3)]
            pub origin: FixedBytes<4>,
            #[field(index = 4)]
            pub destination: FixedBytes<4>,
            #[field(index = 5)]
            pub airline: FixedBytes<8>,
            #[field(index = 6)]
            pub aircraft_model: FixedBytes<16>,
            #[field(index = 7)]
            pub status: u8,
            #[field(index = 8)]
            pub economy_total: u32,
            #[field(index = 9)]
            pub economy_reserved: u32,
            #[field(index = 10)]
            pub business_total: u32,
            #[field(index = 11)]
            pub business_reserved: u32,
            #[field(index = 12)]
            pub first_total: u32,
            #[field(index = 13)]
            pub first_reserved: u32,
        }

        #[record(kind = 2, record_len = 128, uk(id = 0, fields = [document_id]))]
        pub struct Passenger {
            #[field(index = 1, immutable = true)]
            pub document_id: FixedBytes<32>,
            #[field(index = 2)]
            pub nationality: FixedBytes<4>,
            #[field(index = 3)]
            pub birth_date: u32,
            #[field(index = 4)]
            pub document_type: u8,
        }

        #[record(
            kind = 3,
            record_len = 128,
            uk(id = 0, fields = [flight_id, passenger_document_id])
        )]
        pub struct Reservation {
            #[field(index = 1, immutable = true)]
            pub flight_id: FixedBytes<16>,
            #[field(index = 2, immutable = true)]
            pub passenger_document_id: FixedBytes<32>,
            #[field(index = 3)]
            pub order_id: FixedBytes<32>,
            #[field(index = 4)]
            pub cabin_class: u8,
            #[field(index = 5)]
            pub status: u8,
            #[field(index = 6)]
            pub reserved_at_ref_time_us: u64,
        }

        #[command(kind = 1)]
        pub struct AddFlight {
            #[field(index = 1)]
            pub flight_id: FixedBytes<16>,
            #[field(index = 2)]
            pub flight_date: u32,
            #[field(index = 3)]
            pub origin: FixedBytes<4>,
            #[field(index = 4)]
            pub destination: FixedBytes<4>,
            #[field(index = 5)]
            pub airline: FixedBytes<8>,
            #[field(index = 6)]
            pub aircraft_model: FixedBytes<16>,
            #[field(index = 7)]
            pub economy_total: u32,
            #[field(index = 8)]
            pub business_total: u32,
            #[field(index = 9)]
            pub first_total: u32,
        }

        #[command(kind = 2)]
        pub struct RetireFlight {
            #[field(index = 1)]
            pub flight_id: FixedBytes<16>,
        }

        #[command(kind = 3)]
        pub struct ReserveOrder {
            #[field(index = 1)]
            pub flight_id: FixedBytes<16>,
            #[field(index = 2)]
            pub order_id: FixedBytes<32>,
            #[field(index = 3)]
            pub passenger_document_id: FixedBytes<32>,
            #[field(index = 4)]
            pub passenger_nationality: FixedBytes<4>,
            #[field(index = 5)]
            pub passenger_birth_date: u32,
            #[field(index = 6)]
            pub passenger_document_type: u8,
            #[field(index = 7)]
            pub cabin_class: u8,
        }

        #[command(kind = 4)]
        pub struct CancelReservation {
            #[field(index = 1)]
            pub flight_id: FixedBytes<16>,
            #[field(index = 2)]
            pub passenger_document_id: FixedBytes<32>,
            #[field(index = 3)]
            pub order_id: FixedBytes<32>,
        }

        #[event(kind = 1)]
        pub struct FlightAdded {
            #[field(index = 1)]
            pub flight_id: FixedBytes<16>,
        }

        #[event(kind = 2)]
        pub struct FlightRemoved {
            #[field(index = 1)]
            pub flight_id: FixedBytes<16>,
        }

        #[event(kind = 3)]
        pub struct ReserveResult {
            #[field(index = 1)]
            pub flight_id: FixedBytes<16>,
            #[field(index = 2)]
            pub passenger_document_id: FixedBytes<32>,
            #[field(index = 3)]
            pub order_id: FixedBytes<32>,
            #[field(index = 4)]
            pub cabin_class: u8,
            #[field(index = 5)]
            pub result_code: u8,
        }

        #[event(kind = 4)]
        pub struct CancelResult {
            #[field(index = 1)]
            pub flight_id: FixedBytes<16>,
            #[field(index = 2)]
            pub passenger_document_id: FixedBytes<32>,
            #[field(index = 3)]
            pub order_id: FixedBytes<32>,
            #[field(index = 4)]
            pub cabin_class: u8,
            #[field(index = 5)]
            pub result_code: u8,
        }
    }
}

pub use flight_booking::v1_0::*;
