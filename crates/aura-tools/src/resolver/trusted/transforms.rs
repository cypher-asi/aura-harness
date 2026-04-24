//! Result-shape transforms applied to trusted-provider responses.
//!
//! `apply_result_transform` interprets a [`TrustedIntegrationResultTransform`]
//! against the provider's raw JSON response and produces the canonical
//! envelope the agent sees (`{ key: ... }` for `WrapPointer` /
//! `ProjectArray` / `ProjectObject`, `{ query, results, more_results_available }`
//! for `BraveSearch`).

use super::super::json_paths::required_string;
use super::{TrustedIntegrationResultField, TrustedIntegrationResultTransform};
use crate::error::ToolError;
use serde_json::{json, Value};

pub(super) fn apply_result_transform(
    response: &Value,
    transform: &TrustedIntegrationResultTransform,
    args: &Value,
) -> Result<Value, ToolError> {
    match transform {
        TrustedIntegrationResultTransform::WrapPointer { key, pointer } => Ok(object_with_entry(
            key,
            response
                .pointer(pointer)
                .cloned()
                .unwrap_or_else(|| json!({})),
        )),
        TrustedIntegrationResultTransform::ProjectArray {
            key,
            pointer,
            fields,
            extras,
        } => {
            let source = pointer
                .as_deref()
                .map_or(Some(response), |path| response.pointer(path));
            let items = source
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|item| project_fields(&item, fields))
                .collect::<Vec<_>>();
            let mut result = object_with_entry(key, Value::Array(items));
            for extra in extras {
                let value = response
                    .pointer(&extra.pointer)
                    .cloned()
                    .or_else(|| extra.default_value.clone())
                    .unwrap_or(Value::Null);
                result[&extra.output] = value;
            }
            Ok(result)
        }
        TrustedIntegrationResultTransform::ProjectObject {
            key,
            pointer,
            fields,
        } => {
            let source = pointer
                .as_deref()
                .map_or(Some(response), |path| response.pointer(path))
                .cloned()
                .unwrap_or_else(|| json!({}));
            Ok(object_with_entry(key, project_fields(&source, fields)))
        }
        TrustedIntegrationResultTransform::BraveSearch { vertical } => {
            brave_search_results(response, vertical, args)
        }
    }
}

/// Shape Brave Search responses into the canonical
/// `{ query, results: [...], more_results_available }` envelope.
///
/// Used both by the in-line `brave_search` runtime path and by the
/// declarative `apply_result_transform::BraveSearch` transform — keeping
/// one implementation guarantees both code paths produce bit-identical
/// output for the same Brave response.
pub(super) fn brave_search_results(
    response: &Value,
    vertical: &str,
    args: &Value,
) -> Result<Value, ToolError> {
    let query = required_string(args, &["query", "q"])?;
    let items = response
        .pointer(&format!("/{vertical}/results"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|item| {
            json!({
                "title": item.get("title").and_then(Value::as_str).unwrap_or_default(),
                "url": item
                    .get("url")
                    .or_else(|| item.get("profile"))
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                "description": item
                    .get("description")
                    .or_else(|| item.get("snippet"))
                    .and_then(Value::as_str),
                "age": item.get("age").and_then(Value::as_str),
                "source": item.get("source").and_then(Value::as_str),
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({
        "query": query,
        "results": items,
        "more_results_available": response
            .pointer("/query/more_results_available")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    }))
}

fn object_with_entry(key: &str, value: Value) -> Value {
    let mut map = serde_json::Map::new();
    map.insert(key.to_string(), value);
    Value::Object(map)
}

fn project_fields(source: &Value, fields: &[TrustedIntegrationResultField]) -> Value {
    let mut result = json!({});
    for field in fields {
        result[&field.output] = source
            .pointer(&field.pointer)
            .cloned()
            .unwrap_or(Value::Null);
    }
    result
}
