//! Argument bindings: URL templating and query strings, JSON bodies and
//! form fields built from a spec's [`TrustedIntegrationArgBinding`]s.

use super::super::json_paths::{
    insert_json_path, optional_boolean_from_names, optional_boolean_from_names_map,
    optional_json_from_names, optional_json_from_names_map, optional_positive_number_from_names,
    optional_positive_number_from_names_map, optional_string_from_names,
    optional_string_from_names_map, optional_string_list_from_names,
    optional_string_list_from_names_map, required_string,
};
use super::{
    TrustedIntegrationArgBinding, TrustedIntegrationArgSource, TrustedIntegrationArgValueType,
};
use crate::error::ToolError;
use aura_core_types::{InstalledToolRuntimeIntegration, InstalledToolRuntimeProviderExecution};
use reqwest::Url;
use serde_json::{json, Value};
use std::collections::HashMap;

pub(super) fn build_runtime_url(
    provider: &InstalledToolRuntimeProviderExecution,
    integration: &InstalledToolRuntimeIntegration,
    path: &str,
    query_bindings: &[TrustedIntegrationArgBinding],
    args: &Value,
) -> Result<String, ToolError> {
    let expanded_path = expand_path_template(path, args)?;
    let resolved_base_url = integration
        .base_url
        .as_deref()
        .unwrap_or(&provider.base_url);
    let base = format!(
        "{}{}",
        resolved_base_url.trim_end_matches('/'),
        expanded_path
    );
    let mut url = Url::parse(&base)
        .map_err(|e| ToolError::ExternalToolError(format!("invalid trusted runtime url: {e}")))?;
    for binding in query_bindings {
        if let Some(value) = resolve_binding_value(args, &integration.provider_config, binding)? {
            append_query_value(&mut url, &binding.target, value);
        }
    }
    Ok(url.to_string())
}

fn expand_path_template(path: &str, args: &Value) -> Result<String, ToolError> {
    let mut expanded = String::new();
    let mut chars = path.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '{' {
            let mut key = String::new();
            for next in chars.by_ref() {
                if next == '}' {
                    break;
                }
                key.push(next);
            }
            let value = required_string(args, &[key.as_str()])?;
            expanded.push_str(&value);
        } else {
            expanded.push(ch);
        }
    }
    Ok(expanded)
}

fn append_query_value(url: &mut Url, key: &str, value: Value) {
    let mut pairs = url.query_pairs_mut();
    match value {
        Value::Array(items) => {
            for item in items {
                if let Some(value) = item.as_str() {
                    pairs.append_pair(key, value);
                } else {
                    pairs.append_pair(key, &item.to_string());
                }
            }
        }
        Value::String(value) => {
            pairs.append_pair(key, &value);
        }
        other => {
            pairs.append_pair(key, &other.to_string());
        }
    }
}

pub(super) fn build_object_from_bindings(
    bindings: &[TrustedIntegrationArgBinding],
    args: &Value,
    provider_config: &HashMap<String, Value>,
) -> Result<Option<Value>, ToolError> {
    if bindings.is_empty() {
        return Ok(None);
    }

    if bindings.len() == 1 && bindings[0].target == "$" {
        return resolve_binding_value(args, provider_config, &bindings[0]);
    }

    let mut body = json!({});
    let mut inserted = false;
    for binding in bindings {
        if binding.target == "$" {
            return Err(ToolError::ExternalToolError(
                "trusted integration metadata cannot mix root body bindings with object bindings"
                    .into(),
            ));
        }
        if let Some(value) = resolve_binding_value(args, provider_config, binding)? {
            insert_json_path(&mut body, &binding.target, value)?;
            inserted = true;
        }
    }
    Ok(inserted.then_some(body))
}

pub(super) fn build_form_fields_from_bindings(
    bindings: &[TrustedIntegrationArgBinding],
    args: &Value,
    provider_config: &HashMap<String, Value>,
) -> Result<Vec<(String, String)>, ToolError> {
    let mut fields = Vec::new();
    for binding in bindings {
        if let Some(value) = resolve_binding_value(args, provider_config, binding)? {
            match value {
                Value::Array(items) => {
                    for item in items {
                        fields.push((binding.target.clone(), form_field_value(item)));
                    }
                }
                other => fields.push((binding.target.clone(), form_field_value(other))),
            }
        }
    }
    Ok(fields)
}

fn form_field_value(value: Value) -> String {
    match value {
        Value::String(value) => value,
        other => other.to_string(),
    }
}

fn resolve_binding_value(
    args: &Value,
    provider_config: &HashMap<String, Value>,
    binding: &TrustedIntegrationArgBinding,
) -> Result<Option<Value>, ToolError> {
    if binding.arg_names.is_empty() {
        return Ok(binding.default_value.clone());
    }

    let resolved = match binding.source {
        TrustedIntegrationArgSource::InputArgs => resolve_from_input_args(args, binding),
        TrustedIntegrationArgSource::ProviderConfig => {
            resolve_from_provider_config(provider_config, binding)
        }
    };

    if let Some(value) = resolved {
        return Ok(Some(value));
    }
    if let Some(default) = &binding.default_value {
        return Ok(Some(default.clone()));
    }
    if binding.required {
        return Err(ToolError::ExternalToolError(format!(
            "missing required field `{}`",
            binding.arg_names.first().map_or("", String::as_str)
        )));
    }
    Ok(None)
}

fn resolve_from_input_args(args: &Value, binding: &TrustedIntegrationArgBinding) -> Option<Value> {
    match binding.value_type {
        TrustedIntegrationArgValueType::String => {
            optional_string_from_names(args, &binding.arg_names).map(Value::String)
        }
        TrustedIntegrationArgValueType::StringList => {
            optional_string_list_from_names(args, &binding.arg_names).map(|items| json!(items))
        }
        TrustedIntegrationArgValueType::PositiveNumber => {
            optional_positive_number_from_names(args, &binding.arg_names).map(|value| json!(value))
        }
        TrustedIntegrationArgValueType::Boolean => {
            optional_boolean_from_names(args, &binding.arg_names).map(|value| json!(value))
        }
        TrustedIntegrationArgValueType::Json => optional_json_from_names(args, &binding.arg_names),
    }
}

fn resolve_from_provider_config(
    provider_config: &HashMap<String, Value>,
    binding: &TrustedIntegrationArgBinding,
) -> Option<Value> {
    match binding.value_type {
        TrustedIntegrationArgValueType::String => {
            optional_string_from_names_map(provider_config, &binding.arg_names).map(Value::String)
        }
        TrustedIntegrationArgValueType::StringList => {
            optional_string_list_from_names_map(provider_config, &binding.arg_names)
                .map(|items| json!(items))
        }
        TrustedIntegrationArgValueType::PositiveNumber => {
            optional_positive_number_from_names_map(provider_config, &binding.arg_names)
                .map(|value| json!(value))
        }
        TrustedIntegrationArgValueType::Boolean => {
            optional_boolean_from_names_map(provider_config, &binding.arg_names)
                .map(|value| json!(value))
        }
        TrustedIntegrationArgValueType::Json => {
            optional_json_from_names_map(provider_config, &binding.arg_names)
        }
    }
}
