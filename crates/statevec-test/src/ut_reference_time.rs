use super::*;
use statevec_api::{RuntimeCommandEnvelope, TypedTxContext};

fn business_time<Tx: TypedTxContext + ?Sized>(tx: &Tx) -> Result<u64, ReferenceTimeUnavailable> {
    tx.ref_tx_time_ns()
}

struct ObserveTime;

impl RuntimePlugin for ObserveTime {
    fn name(&self) -> &'static str {
        "reference-time"
    }

    fn schema_registry(&self) -> SchemaRegistry {
        SchemaRegistry::with_records(statevec_model::Version::new(1, 0), &[])
    }

    fn run_tx(
        &self,
        tx: &mut dyn RuntimeHostContext,
        command: &dyn RuntimeCommandEnvelope,
    ) -> Result<(), RuntimePluginError> {
        let assigned = business_time(tx).map_err(|error| RuntimePluginError::new(error.to_string()))?;
        let mut payload = assigned.to_le_bytes().to_vec();
        payload.extend_from_slice(&command.ref_ext_time_us().to_le_bytes());
        tx.emit_typed_event_raw(1, &payload).map_err(|error| RuntimePluginError::new(error.to_string()))
    }
}

#[test]
fn unconfigured_host_does_not_invent_epoch_zero() {
    let host = TestHost::new(ObserveTime.schema_registry());
    assert_eq!(business_time(&host), Err(ReferenceTimeUnavailable));
    assert_eq!(business_time(&host as &dyn RuntimeHostContext), Err(ReferenceTimeUnavailable));
}

#[test]
fn explicit_time_survives_typed_plugin_dispatch_and_external_time_changes() {
    let mut host = TestHost::for_plugin(ObserveTime).with_ref_tx_time_ns(u64::MAX);
    assert_eq!(business_time(host.inner()), Ok(u64::MAX), "builder supplies explicit fixture time");
    let payload = 7u64.to_le_bytes();
    for (assigned, external_us) in [(u64::MAX, 17), (0, 999), (42, u64::MAX)] {
        host.set_ref_tx_time_ns(assigned);
        assert_eq!(business_time(host.inner()), Ok(assigned), "typed native fixture forwarding");
        let command = RuntimeCommandRef::new(9, 1, external_us, &payload);
        host.inner_mut().run_tx(&ObserveTime, &command).unwrap();
        let event = host.events().last().unwrap();
        assert_eq!(&event.payload[..8], assigned.to_le_bytes(), "execution time must retain ns precision");
        assert_eq!(&event.payload[8..], external_us.to_le_bytes(), "external us remain a separate input");
    }
}
