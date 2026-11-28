//! Reading and validating tool arguments.

use crate::runpod::RunpodError;
use serde_json::Value;

/// Longest id accepted for a pod, endpoint or job.
const MAX_ID_LEN: usize = 128;

/// A required, non-empty string argument.
pub(super) fn string(tool: &str, args: &Value, name: &str) -> Result<String, RunpodError> {
    optional_string(tool, args, name)?
        .ok_or_else(|| RunpodError::invalid(tool, format!("`{name}` is required")))
}

/// An optional string argument; empty counts as absent.
pub(super) fn optional_string(
    tool: &str,
    args: &Value,
    name: &str,
) -> Result<Option<String>, RunpodError> {
    match args.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => {
            let s = s.trim();
            Ok((!s.is_empty()).then(|| s.to_string()))
        }
        Some(_) => Err(RunpodError::invalid(
            tool,
            format!("`{name}` must be a string"),
        )),
    }
}

/// An optional non-negative integer argument.
pub(super) fn optional_u64(
    tool: &str,
    args: &Value,
    name: &str,
) -> Result<Option<u64>, RunpodError> {
    match args.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v.as_u64().map(Some).ok_or_else(|| {
            RunpodError::invalid(tool, format!("`{name}` must be a whole number, 0 or more"))
        }),
    }
}

/// An optional boolean argument.
pub(super) fn optional_bool(
    tool: &str,
    args: &Value,
    name: &str,
) -> Result<Option<bool>, RunpodError> {
    match args.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_bool()
            .map(Some)
            .ok_or_else(|| RunpodError::invalid(tool, format!("`{name}` must be true or false"))),
    }
}

/// A required id that goes into a URL path: letters, digits, `-` and `_`
/// only, so it can't reach another path.
pub(super) fn id(tool: &str, args: &Value, name: &str) -> Result<String, RunpodError> {
    let value = string(tool, args, name)?;
    let valid = value.len() <= MAX_ID_LEN
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if !valid {
        return Err(RunpodError::invalid(
            tool,
            format!("`{name}` must be a Runpod id (letters, digits, - and _)"),
        ));
    }
    Ok(value)
}

/// An optional string argument that must be one of `allowed`.
pub(super) fn optional_choice(
    tool: &str,
    args: &Value,
    name: &str,
    allowed: &[&str],
) -> Result<Option<String>, RunpodError> {
    let Some(value) = optional_string(tool, args, name)? else {
        return Ok(None);
    };
    allowed
        .iter()
        .find(|a| a.eq_ignore_ascii_case(&value))
        .map(|a| Some((*a).to_string()))
        .ok_or_else(|| {
            RunpodError::invalid(
                tool,
                format!("`{name}` must be one of {}", allowed.join(", ")),
            )
        })
}
