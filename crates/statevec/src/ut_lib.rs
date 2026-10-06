use super::*;

#[derive(EnumU8)]
#[repr(u8)]
enum Side {
    Buy = 1,
    Sell = 2,
}

#[schema_module(version = "1.0")]
mod schema {
    use super::*;

    #[record(kind = 1, record_len = 64, uk(id = 0, fields = [id]))]
    pub struct Order {
        #[field(index = 1, immutable)]
        pub id: u64,
        #[field(index = 2, enum_u8)]
        pub side: Side,
    }

    #[command(kind = 1)]
    pub struct PlaceOrder {
        #[field(index = 1)]
        pub id: u64,
    }

    #[event(kind = 1)]
    pub struct OrderPlaced {
        #[field(index = 1)]
        pub id: u64,
    }
}

#[test]
fn prelude_exposes_public_model_api_and_macros() {
    assert_eq!(schema::SCHEMA_VERSION, Version::new(1, 0));
    let identity: SchemaIdentity = schema::schema_identity();
    assert_eq!(identity.schema_version, schema::SCHEMA_VERSION);
}
