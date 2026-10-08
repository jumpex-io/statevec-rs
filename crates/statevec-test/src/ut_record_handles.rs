//! A handler resolves a record's key once and keeps acting on its system id.
use super::*;
use statevec_api::{RuntimeHostContextExt, TypedTxContext};
use statevec_macros::{record, schema_module};
use statevec_model::RecordSchema;

#[schema_module(version = "1.0")]
mod schema {
    use super::*;
    #[record(kind = 1, record_len = 64, uk(id = 0, fields = [asset_id]))]
    pub struct Asset {
        #[field(index = 1, immutable)]
        pub asset_id: u64,
        #[field(index = 2)]
        pub precision: u8,
    }
}
use schema::*;

#[test]
fn one_resolved_handle_reads_updates_and_deletes_the_same_record() {
    let mut host = TestHost::new(SchemaRegistry::with_records(
        statevec_model::Version::new(1, 0),
        &[*Asset::definition()],
    ));
    let created = RuntimeHostContextExt::create_typed::<Asset, _>(&mut host, |asset| {
        asset.init_asset_id(1).set_precision(2);
    })
    .unwrap();
    host.transaction(|tx| -> Result<(), RuntimeHostError> {
        assert_eq!(tx.resolve_typed_uk::<Asset, _>(Asset::uk(9))?, None);
        let id = tx
            .resolve_typed_uk::<Asset, _>(Asset::uk(1))?
            .expect("existing asset");
        assert_eq!(id, created.sys_id);
        assert_eq!(
            tx.update_typed::<Asset, _, _>(id, |asset| asset.set_precision(4).precision())?,
            Some(4)
        );
        assert_eq!(
            TypedTxContext::with_read_typed::<Asset, _, _>(tx, id, |asset| asset.precision())?,
            Some(4)
        );
        assert!(tx.delete_typed::<Asset>(id)?);
        assert!(
            !tx.delete_typed::<Asset>(id)?,
            "a deleted handle names no record"
        );
        assert_eq!(
            tx.update_typed::<Asset, _, _>(id, |asset| asset.set_precision(5).precision())?,
            None
        );
        assert_eq!(
            tx.resolve_typed_uk::<Asset, _>(Asset::uk(1))?,
            None,
            "deletion also releases the key"
        );
        Ok(())
    })
    .unwrap();
    assert_eq!(
        host.record_count(),
        0,
        "the committed transaction removed the record"
    );
}

#[test]
fn hosts_without_record_handles_refuse_them_explicitly() {
    let mut host = TestHost::new(SchemaRegistry::with_records(
        statevec_model::Version::new(1, 0),
        &[*Asset::definition()],
    ));
    let created = RuntimeHostContextExt::create_typed::<Asset, _>(&mut host, |asset| {
        asset.init_asset_id(1).set_precision(2);
    })
    .unwrap();
    let plugin = &mut host as &mut dyn RuntimeHostContext;
    assert!(plugin.resolve_typed_uk::<Asset, _>(Asset::uk(1)).is_err());
    assert!(
        plugin
            .update_typed::<Asset, _, _>(created.sys_id, |asset| asset.set_precision(4).precision())
            .is_err()
    );
    assert!(plugin.delete_typed::<Asset>(created.sys_id).is_err());
    assert_eq!(
        RuntimeHostContextExt::with_read_typed::<Asset, _, _>(&host, created.sys_id, |asset| asset
            .precision())
        .unwrap(),
        Some(2),
        "a refused handle changes nothing"
    );
}
