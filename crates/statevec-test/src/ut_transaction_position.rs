use super::*;
use statevec_api::{RuntimeCommandEnvelope, TypedTxContext};

fn position<Tx: TypedTxContext + ?Sized>(tx: &Tx) -> Result<u64, TxPositionUnavailable> {
    tx.tx_seq()
}

/// Records the observed position, then refuses when the payload asks it to.
struct ObservePosition;

impl RuntimePlugin for ObservePosition {
    fn name(&self) -> &'static str {
        "transaction-position"
    }

    fn schema_registry(&self) -> SchemaRegistry {
        SchemaRegistry::with_records(statevec_model::Version::new(1, 0), &[])
    }

    fn run_tx(
        &self,
        tx: &mut dyn RuntimeHostContext,
        command: &dyn RuntimeCommandEnvelope,
    ) -> Result<(), RuntimePluginError> {
        let assigned = position(tx).map_err(|error| RuntimePluginError::new(error.to_string()))?;
        tx.emit_typed_event_raw(1, &assigned.to_le_bytes())
            .map_err(|error| RuntimePluginError::new(error.to_string()))?;
        if command.payload() == b"refuse" {
            return Err(RuntimePluginError::new("refused"));
        }
        Ok(())
    }
}

fn observed(host: &TestHost) -> Vec<u64> {
    host.events()
        .iter()
        .map(|event| u64::from_le_bytes(event.payload[..8].try_into().unwrap()))
        .collect()
}

#[test]
fn position_exists_only_inside_a_transaction() {
    let mut host = TestHost::new(ObservePosition.schema_registry());
    assert_eq!(position(&host), Err(TxPositionUnavailable));
    assert_eq!(position(&host as &dyn RuntimeHostContext), Err(TxPositionUnavailable));
    let inside = host.transaction(|tx| Ok::<_, ()>(position(&*tx))).unwrap();
    assert_eq!(inside, Ok(1));
    assert_eq!(position(&host), Err(TxPositionUnavailable), "no position leaks after commit");
}

#[test]
fn every_transaction_including_refusals_consumes_one_increasing_position() {
    let mut host = TestHost::new(ObservePosition.schema_registry());
    host.set_next_tx_seq(41);
    for payload in [&b"accept"[..], b"refuse", b"accept"] {
        let command = RuntimeCommandRef::new(9, 1, 0, payload);
        let _ = host.run_tx(&ObservePosition, &command);
    }
    assert_eq!(observed(&host), [41, 43], "the refused transaction rolled back yet consumed 42");
    assert_eq!(host.transaction(|tx| Ok::<_, ()>(position(&*tx))).unwrap(), Ok(44));
}
