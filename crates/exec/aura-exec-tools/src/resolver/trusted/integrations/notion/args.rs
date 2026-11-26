//! Reading tool arguments for the Notion handlers.

use crate::error::ToolError;
use crate::resolver::json_paths::{optional_positive_number, optional_string, required_string};
use reqwest::Url;
use serde_json::{Map, Value};

const MAX_PAGE_SIZE: u64 = 100;

pub(super) fn notion_content_mode(args: &Value) -> String {
    optional_string(args, &["content_mode", "contentMode"])
        .unwrap_or_else(|| "metadata".to_string())
        .to_ascii_lowercase()
}

pub(super) fn required_notion_id(args: &Value, keys: &[&str]) -> Result<String, ToolError> {
    required_string(args, keys).map(|value| notion_id_from_ref(&value))
}

pub(super) fn optional_notion_id(args: &Value, keys: &[&str]) -> Option<String> {
    optional_string(args, keys).map(|value| notion_id_from_ref(&value))
}

pub(super) fn notion_id_from_ref(value: &str) -> String {
    let trimmed = value.trim();
    if let Ok(url) = Url::parse(trimmed) {
        if let Some(segment) = url
            .path_segments()
            .and_then(|segments| segments.filter(|segment| !segment.is_empty()).next_back())
        {
            let candidate = segment.rsplit('-').next().unwrap_or(segment);
            if is_notion_id(candidate) {
                return candidate.to_string();
            }
        }
    }
    trimmed.to_string()
}

fn is_notion_id(value: &str) -> bool {
    let normalized = value.replace('-', "");
    normalized.len() == 32 && normalized.chars().all(|ch| ch.is_ascii_hexdigit())
}

pub(super) fn insert_optional_object(
    body: &mut Map<String, Value>,
    args: &Value,
    keys: &[&str],
    field: &str,
) -> Result<(), ToolError> {
    if let Some(value) = optional_object(args, keys)? {
        body.insert(field.to_string(), value);
    }
    Ok(())
}

pub(super) fn insert_optional_array(
    body: &mut Map<String, Value>,
    args: &Value,
    keys: &[&str],
    field: &str,
) -> Result<(), ToolError> {
    if let Some(value) = optional_array(args, keys)? {
        body.insert(field.to_string(), Value::Array(value));
    }
    Ok(())
}

pub(super) fn insert_optional_bool(
    body: &mut Map<String, Value>,
    args: &Value,
    keys: &[&str],
    field: &str,
) {
    if let Some(value) = optional_bool(args, keys) {
        body.insert(field.to_string(), Value::Bool(value));
    }
}

pub(super) fn insert_optional_string(
    body: &mut Map<String, Value>,
    args: &Value,
    keys: &[&str],
    field: &str,
) {
    if let Some(value) = optional_string(args, keys) {
        body.insert(field.to_string(), Value::String(value));
    }
}

pub(super) fn insert_optional_page_size(body: &mut Map<String, Value>, args: &Value) {
    if let Some(page_size) = notion_page_size(args) {
        body.insert("page_size".to_string(), Value::Number(page_size.into()));
    }
}

pub(super) fn optional_object(args: &Value, keys: &[&str]) -> Result<Option<Value>, ToolError> {
    optional_typed_value(args, keys, Value::is_object, "object")
}

pub(super) fn optional_array(args: &Value, keys: &[&str]) -> Result<Option<Vec<Value>>, ToolError> {
    match optional_typed_value(args, keys, Value::is_array, "array")? {
        Some(Value::Array(values)) => Ok(Some(values)),
        _ => Ok(None),
    }
}

pub(super) fn optional_bool(args: &Value, keys: &[&str]) -> Option<bool> {
    keys.iter()
        .find_map(|key| args.get(*key).and_then(Value::as_bool))
}

fn optional_typed_value(
    args: &Value,
    keys: &[&str],
    predicate: fn(&Value) -> bool,
    expected_type: &str,
) -> Result<Option<Value>, ToolError> {
    for key in keys {
        if let Some(value) = args.get(*key) {
            if value.is_null() {
                return Ok(None);
            }
            if predicate(value) {
                return Ok(Some(value.clone()));
            }
            return Err(ToolError::ExternalToolError(format!(
                "field `{key}` must be an {expected_type}"
            )));
        }
    }
    Ok(None)
}

pub(super) fn notion_page_size(args: &Value) -> Option<u64> {
    optional_positive_number(args, &["page_size", "pageSize"])
        .map(|page_size| page_size.clamp(1, MAX_PAGE_SIZE))
}

pub(super) fn has_any(args: &Value, keys: &[&str]) -> bool {
    keys.iter().any(|key| args.get(*key).is_some())
}
