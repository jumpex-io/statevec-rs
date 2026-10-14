#[allow(dead_code)]
#[path = "../examples/projector_v1_latest/author.rs"]
mod author;
use author::model::Model;
use statevec_model::RecordKey;
use statevec_projector_api::plugin_v1::*;

#[test]
fn rebuild_then_deltas_preserve_record_kinds_and_before_images() {
    let mut model = Model::default();
    for kind in [1, 2, u16::MAX] { model.insert(kind, 1, 4, &[kind as u8]).unwrap(); }
    let key = RecordKey { kind: 2, sys_id: 1 };
    let delta = model.change(RecordChangeView::Update { key, revision: 5, before: &[2], after: &[10] }).unwrap();
    assert_eq!(delta["before"], serde_json::json!([2]));
    assert_eq!(delta["after"], serde_json::json!([10]));
    model.change(RecordChangeView::Delete { key, revision: 6, before: &[10] }).unwrap();
    model.change(RecordChangeView::Insert { key: RecordKey { kind: 2, sys_id: 2 }, revision: 1, after: &[20] }).unwrap();
    assert!(!model.records.contains_key(&(2, 1)));
    assert_eq!(model.records[&(2, 2)], (1, vec![20]));
    assert_eq!(model.records[&(1, 1)], (4, vec![1]));
    assert_eq!(model.records[&(u16::MAX, 1)], (4, vec![255]));
    assert_eq!(model.reset(u64::MAX)["raft_index"], u64::MAX.to_string());
}
#[test]
fn duplicate_and_inconsistent_delta_leave_the_model_unchanged() {
    let mut model = Model::default();
    model.insert(2, 1, 4, b"original").unwrap();
    assert!(model.insert(2, 1, 5, b"wrong").is_err());
    let key = RecordKey { kind: 2, sys_id: 1 };
    assert!(model.change(RecordChangeView::Update { key, revision: 5, before: b"wrong", after: b"next" }).is_err());
    assert!(model.change(RecordChangeView::Delete { key, revision: 4, before: b"original" }).is_err());
    assert_eq!(model.records[&(2, 1)], (4, b"original".to_vec()));
}
#[test]
fn configuration_is_strict_and_never_opens_the_report() {
    let directory = tempfile::tempdir().unwrap();
    let configure = |bytes| author::Latest.configure(ProjectorConfig { bytes, base_dir: directory.path() });
    assert!(configure(br#"{"report_path":"output"}"#).is_ok());
    for bytes in [br#"["output"]"#.as_slice(), br#"{"report_path":""}"#, br#"{"report_path":"x","offset_path":"y"}"#, br#"{"report_path":"x","report_path":"y"}"#] {
        assert!(configure(bytes).is_err(), "{bytes:?}");
    }
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
}
