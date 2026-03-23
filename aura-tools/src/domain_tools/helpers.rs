//! Shared utility helpers for domain tool handlers.

use serde_json::Value;

/// Extract a string field from a JSON value.
pub fn str_field(input: &Value, key: &str) -> Option<String> {
    input
        .get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// Extract and parse a required string field into a target type.
pub fn parse_id(input: &Value, key: &str) -> Result<String, String> {
    str_field(input, key).ok_or_else(|| format!("Missing required field: {key}"))
}

/// Extract a required string field, returning an error message on absence.
pub fn require_str(input: &Value, key: &str) -> Result<String, String> {
    str_field(input, key).ok_or_else(|| format!("Missing required field: {key}"))
}

/// Extract an optional list of strings from a JSON array field.
pub fn str_array(input: &Value, key: &str) -> Vec<String> {
    input
        .get(key)
        .and_then(|v| serde_json::from_value::<Vec<String>>(v.clone()).ok())
        .unwrap_or_default()
}
