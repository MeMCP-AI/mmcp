//! Shared permissive `output_schema` attached to every registered tool.

/// Every registered tool receives a permissive object
/// `output_schema` so MCP clients can validate that responses are
/// JSON objects (with optional `notes` channel) and surface the
/// shape in autocomplete UIs.
///
/// Cached behind a `OnceLock` so the same `Arc<JsonObject>` reaches
/// every tool. Cheap to clone; cheaper than rebuilding the map per
/// tool on every `tools/list` round-trip.
pub(crate) fn shared_output_schema() -> std::sync::Arc<rmcp::model::JsonObject> {
    use std::sync::OnceLock;
    static SCHEMA: OnceLock<std::sync::Arc<rmcp::model::JsonObject>> = OnceLock::new();
    SCHEMA
        .get_or_init(|| {
            let mut obj = serde_json::Map::new();
            obj.insert(
                "type".to_string(),
                serde_json::Value::String("object".to_string()),
            );
            obj.insert(
                "additionalProperties".to_string(),
                serde_json::Value::Bool(true),
            );
            // Surface the shared `notes` field shape so harnesses
            // know to look there for dangling-ref / parse-warning
            // notes; absent on tools that never emit any.
            let mut props = serde_json::Map::new();
            let mut notes_schema = serde_json::Map::new();
            notes_schema.insert(
                "type".to_string(),
                serde_json::Value::String("array".to_string()),
            );
            notes_schema.insert(
                "description".to_string(),
                serde_json::Value::String(
                    "Standard notes channel. Optional warnings emitted alongside the \
                     tool's primary payload."
                        .to_string(),
                ),
            );
            props.insert("notes".to_string(), serde_json::Value::Object(notes_schema));
            obj.insert("properties".to_string(), serde_json::Value::Object(props));
            obj.insert(
                "description".to_string(),
                serde_json::Value::String(
                    "Tool response. Permissive object shape; per-tool typed schemas \
                     are a candidate future refinement."
                        .to_string(),
                ),
            );
            std::sync::Arc::new(obj)
        })
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The schema is shared (same `Arc`) across all tools so
    /// the per-tool patch is cheap.
    /// Cloning the Arc bumps the reference count rather than
    /// rebuilding the JsonObject.
    #[test]
    fn shared_output_schema_returns_same_arc() {
        let a = shared_output_schema();
        let b = shared_output_schema();
        assert!(
            std::sync::Arc::ptr_eq(&a, &b),
            "shared_output_schema must hand out the same Arc on repeat calls",
        );
    }
}
