use statevec_projector_api::plugin_v1::{ProjectorFailure, RecordChangeView};
use std::collections::{BTreeMap, btree_map::Entry};

pub fn failure(code: u32) -> ProjectorFailure { ProjectorFailure::Application { code } }

/// Private application model, not host evidence or a persisted consume offset.
#[derive(Default)]
pub struct Model {
    pub records: BTreeMap<(u16, u64), (u64, Vec<u8>)>,
}
impl Model {
    pub fn insert(&mut self, kind: u16, id: u64, revision: u64, bytes: &[u8]) -> Result<(), ProjectorFailure> {
        match self.records.entry((kind, id)) {
            Entry::Vacant(slot) => { slot.insert((revision, bytes.to_vec())); Ok(()) }
            Entry::Occupied(_) => Err(failure(1201)),
        }
    }
    pub fn change(&mut self, change: RecordChangeView<'_>) -> Result<serde_json::Value, ProjectorFailure> {
        let (operation, key, revision, before, after) = match change {
            RecordChangeView::Insert { key, revision, after } => ("insert", key, revision, None, Some(after)),
            RecordChangeView::Update { key, revision, before, after } => ("update", key, revision, Some(before), Some(after)),
            RecordChangeView::Delete { key, revision, before } => ("delete", key, revision, Some(before), None),
        };
        if let Some(before) = before {
            let previous = self.records.get(&(key.kind, key.sys_id)).ok_or_else(|| failure(1202))?;
            if previous.1 != before || revision <= previous.0 { return Err(failure(1203)); }
            if let Some(after) = after {
                *self.records.get_mut(&(key.kind, key.sys_id)).expect("checked record") = (revision, after.to_vec());
            } else {
                self.records.remove(&(key.kind, key.sys_id));
            }
        } else {
            self.insert(key.kind, key.sys_id, revision, after.expect("insert after"))?;
        }
        // A websocket implementation can encode its schema-specific field diff
        // here. This generic example sends only changed records, not the model.
        Ok(serde_json::json!({ "operation": operation, "kind": key.kind,
            "sys_id": key.sys_id.to_string(), "revision": revision.to_string(),
            "before": before, "after": after }))
    }
    pub fn reset(&self, index: u64) -> serde_json::Value {
        let records: Vec<_> = self.records.iter().map(|((kind, id), (revision, bytes))|
            serde_json::json!({ "kind": kind, "sys_id": id.to_string(), "revision": revision.to_string(), "bytes": bytes })
        ).collect();
        serde_json::json!({ "type": "reset", "raft_index": index.to_string(), "records": records })
    }
}
