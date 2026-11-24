//! Preserve all execution schema bytes and change one display-only field tag.
pub fn tagged(idl: &str, family: &str) -> Option<String> {
    if !matches!(family, "records" | "commands" | "events") {
        return None;
    }
    let mut document: serde_json::Value = serde_json::from_str(idl).ok()?;
    let definition = document
        .get_mut(family)?
        .as_array_mut()?
        .iter_mut()
        .find(|definition| definition["kind"] == 1)?;
    // The account ID is a u64 in each of these three Bank definitions.
    let field = definition.get_mut("fields")?.as_array_mut()?.first_mut()?;
    field["semantic"] = "timestampMicros".into();
    serde_json::to_string(&document).ok()
}
