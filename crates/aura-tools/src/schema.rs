//! Claude-style tool JSON ↔ [`ToolDefinition`] converter.
//!
//! The in-process super-agent registers tool schemas in Claude's native
//! shape (`{name, description, input_schema, eager_input_streaming?}`)
//! while the harness-side model provider consumes
//! [`aura_core::ToolDefinition`]. Phase 2 of the super-agent / harness
//! unification plan needs a single-file converter so the portable
//! super-agent profile (shipped as JSON) can be translated into harness
//! tools at session boot.

use aura_core::ToolDefinition;
use serde_json::Value;
use thiserror::Error;

/// Errors produced by [`from_claude_json`].
#[derive(Debug, Error)]
pub enum SchemaError {
    #[error("missing required field: {0}")]
    MissingField(&'static str),
    #[error("field {0} must be a {1}")]
    InvalidType(&'static str, &'static str),
}

/// Convert a Claude-shaped tool JSON into an [`aura_core::ToolDefinition`].
///
/// Expected shape:
/// ```json
/// {
///   "name": "...",
///   "description": "...",
///   "input_schema": { ... },
///   "eager_input_streaming": true?
/// }
/// ```
pub fn from_claude_json(value: &Value) -> Result<ToolDefinition, SchemaError> {
    let obj = value
        .as_object()
        .ok_or(SchemaError::InvalidType("root", "object"))?;

    let name = obj
        .get("name")
        .ok_or(SchemaError::MissingField("name"))?
        .as_str()
        .ok_or(SchemaError::InvalidType("name", "string"))?;

    let description = obj
        .get("description")
        .ok_or(SchemaError::MissingField("description"))?
        .as_str()
        .ok_or(SchemaError::InvalidType("description", "string"))?;

    let input_schema = obj
        .get("input_schema")
        .cloned()
        .ok_or(SchemaError::MissingField("input_schema"))?;
    if !input_schema.is_object() {
        return Err(SchemaError::InvalidType("input_schema", "object"));
    }

    let mut def = ToolDefinition::new(name, description, input_schema);
    if matches!(obj.get("eager_input_streaming"), Some(Value::Bool(true))) {
        def.eager_input_streaming = Some(true);
    }
    Ok(def)
}

/// Reverse of [`from_claude_json`]. Produces the exact wire shape the
/// super-agent crate emits so round-tripping is loss-free.
#[must_use]
pub fn to_claude_json(def: &ToolDefinition) -> Value {
    let mut obj = serde_json::Map::new();
    obj.insert("name".to_string(), Value::String(def.name.clone()));
    obj.insert(
        "description".to_string(),
        Value::String(def.description.clone()),
    );
    obj.insert("input_schema".to_string(), def.input_schema.clone());
    if def.eager_input_streaming == Some(true) {
        obj.insert("eager_input_streaming".to_string(), Value::Bool(true));
    }
    Value::Object(obj)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn converts_minimal_claude_tool_to_tool_definition() {
        let v = json!({
            "name": "create_project",
            "description": "Create a new project in the org.",
            "input_schema": {
                "type": "object",
                "properties": {"name": {"type": "string"}},
                "required": ["name"]
            }
        });
        let def = from_claude_json(&v).unwrap();
        assert_eq!(def.name, "create_project");
        assert_eq!(def.description, "Create a new project in the org.");
        assert_eq!(
            def.input_schema["required"][0].as_str(),
            Some("name")
        );
        assert_eq!(def.eager_input_streaming, None);
    }

    #[test]
    fn propagates_eager_input_streaming_flag() {
        let v = json!({
            "name": "create_spec",
            "description": "Draft a spec.",
            "input_schema": {"type": "object"},
            "eager_input_streaming": true
        });
        let def = from_claude_json(&v).unwrap();
        assert_eq!(def.eager_input_streaming, Some(true));
    }

    #[test]
    fn rejects_missing_required_fields() {
        let cases = [
            json!({"description": "x", "input_schema": {"type": "object"}}),
            json!({"name": "x", "input_schema": {"type": "object"}}),
            json!({"name": "x", "description": "x"}),
        ];
        for v in cases {
            assert!(from_claude_json(&v).is_err());
        }
    }

    #[test]
    fn rejects_wrong_types() {
        let v = json!({"name": 1, "description": "x", "input_schema": {}});
        assert!(matches!(
            from_claude_json(&v),
            Err(SchemaError::InvalidType("name", "string"))
        ));

        let v = json!({"name": "x", "description": "x", "input_schema": []});
        assert!(matches!(
            from_claude_json(&v),
            Err(SchemaError::InvalidType("input_schema", "object"))
        ));
    }

    #[test]
    fn roundtrip_is_lossless_for_all_fields() {
        let original = json!({
            "name": "update_spec",
            "description": "Persist a spec revision.",
            "input_schema": {
                "type": "object",
                "properties": {"spec_id": {"type": "string"}, "body": {"type": "string"}},
                "required": ["spec_id", "body"]
            },
            "eager_input_streaming": true
        });
        let def = from_claude_json(&original).unwrap();
        let back = to_claude_json(&def);
        assert_eq!(back, original);
    }

    #[test]
    fn roundtrip_omits_eager_flag_when_not_set() {
        let v = json!({
            "name": "list_projects",
            "description": "List projects in the org.",
            "input_schema": {"type": "object"}
        });
        let def = from_claude_json(&v).unwrap();
        let back = to_claude_json(&def);
        assert!(back.get("eager_input_streaming").is_none());
    }
}
