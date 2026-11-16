//! Shared utility helpers for domain tool handlers.

use serde_json::Value;

/// Extract a string field from a JSON value.
pub(crate) fn str_field(input: &Value, key: &str) -> Option<String> {
    input
        .get(key)
        .and_then(|v| v.as_str())
        .map(ToString::to_string)
}

/// Extract a required string field, returning an error message on absence.
pub(crate) fn require_str(input: &Value, key: &str) -> Result<String, String> {
    str_field(input, key).ok_or_else(|| format!("Missing required field: {key}"))
}

/// Wrap a successful domain tool result into the standard JSON envelope.
/// Merges `payload` fields into `{"ok": true, ...}`.
pub(crate) fn domain_ok(payload: serde_json::Value) -> String {
    let mut envelope = serde_json::json!({ "ok": true });
    if let Value::Object(map) = payload {
        if let Value::Object(ref mut env_map) = envelope {
            env_map.extend(map);
        }
    }
    envelope.to_string()
}

/// Wrap an error into the standard JSON envelope.
pub(crate) fn domain_err(error: impl std::fmt::Display) -> String {
    serde_json::json!({ "ok": false, "error": error.to_string() }).to_string()
}

/// Extract an optional list of strings from a JSON array field.
pub(crate) fn str_array(input: &Value, key: &str) -> Vec<String> {
    input
        .get(key)
        .and_then(|v| serde_json::from_value::<Vec<String>>(v.clone()).ok())
        .unwrap_or_default()
}

/// Extract an optional list of strings, distinguishing "field absent"
/// (`None`, leave unchanged) from "field present" (`Some`, replace) so
/// partial updates don't clobber an existing list with an empty one.
pub(crate) fn opt_str_array(input: &Value, key: &str) -> Option<Vec<String>> {
    input
        .get(key)
        .map(|v| serde_json::from_value::<Vec<String>>(v.clone()).unwrap_or_default())
}

/// Extract an optional `u32` field (accepts JSON numbers).
pub(crate) fn u32_field(input: &Value, key: &str) -> Option<u32> {
    input
        .get(key)
        .and_then(serde_json::Value::as_u64)
        .and_then(|n| u32::try_from(n).ok())
}
