//! Structured redaction markers and write-input summaries.

use serde_json::Value;

use super::summary::SummaryOutput;

/// Compatibility marker names used by Phase 1 redaction summaries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedactionMarker {
    /// `write_file.content` was summarized.
    WriteContent,
    /// `edit_file.old_*` was summarized.
    EditOld,
    /// `edit_file.new_*` was summarized.
    EditNew,
    /// A stored tool blob exceeded the storage cap.
    StorageBlob,
}

impl RedactionMarker {
    /// Return a copy of `input` with `field` removed and structured redaction metadata added.
    #[must_use]
    pub fn mark(input: &Value, field: &str, bytes: usize) -> Value {
        let Value::Object(source) = input else {
            return input.clone();
        };

        let mut marked = source.clone();
        marked.remove(field);
        let entry = serde_json::json!({
            "kind": "aura_compaction_redaction",
            "version": 1,
            "field": field,
            "bytes": bytes,
        });

        match marked.remove("_redacted") {
            Some(Value::Object(mut existing)) => {
                if let Some(Value::Array(mut fields)) = existing.remove("fields") {
                    fields.push(entry_without_kind(field, bytes));
                    marked.insert(
                        "_redacted".to_string(),
                        serde_json::json!({
                            "kind": "aura_compaction_redaction",
                            "version": 1,
                            "fields": fields,
                        }),
                    );
                } else if let (Some(existing_field), Some(existing_bytes)) = (
                    existing.get("field").and_then(Value::as_str),
                    existing.get("bytes").and_then(Value::as_u64),
                ) {
                    marked.insert(
                        "_redacted".to_string(),
                        serde_json::json!({
                            "kind": "aura_compaction_redaction",
                            "version": 1,
                            "fields": [
                                { "field": existing_field, "bytes": existing_bytes },
                                { "field": field, "bytes": bytes },
                            ],
                        }),
                    );
                } else {
                    marked.insert("_redacted".to_string(), entry);
                }
            }
            Some(other) => {
                marked.insert("_redacted".to_string(), other);
            }
            None => {
                marked.insert("_redacted".to_string(), entry);
            }
        }

        Value::Object(marked)
    }

    /// Detect the structured redaction marker convention.
    #[must_use]
    pub fn is_marker(value: &Value) -> bool {
        let Value::Object(map) = value else {
            return false;
        };
        if let Some(marker) = map.get("_redacted") {
            return Self::is_marker(marker);
        }
        if map
            .get("kind")
            .and_then(Value::as_str)
            .is_some_and(|kind| kind == "aura_compaction_redaction")
        {
            return map.get("field").and_then(Value::as_str).is_some()
                || map.get("fields").and_then(Value::as_array).is_some();
        }
        false
    }
}

fn entry_without_kind(field: &str, bytes: usize) -> Value {
    serde_json::json!({ "field": field, "bytes": bytes })
}

/// Input to a write-input or cached-result summary operation.
#[derive(Debug, Clone, Copy)]
pub struct ToolSummaryInput<'a> {
    /// Tool name associated with the payload.
    pub tool_name: &'a str,
    /// Tool JSON input.
    pub input: &'a Value,
    /// Optional tool-result text when summarizing cached reads.
    pub content: Option<&'a str>,
}

/// Summarize write tool inputs to save context tokens.
#[must_use]
pub fn summarize_write_input(tool_name: &str, input: &Value) -> Option<Value> {
    match tool_name {
        "write_file" => {
            let content_len = input
                .get("content")
                .and_then(Value::as_str)
                .map_or(0, str::len);
            if input.get("content").is_some() {
                Some(RedactionMarker::mark(input, "content", content_len))
            } else {
                None
            }
        }
        "edit_file" => {
            let old_key = if input.get("old_string").is_some() {
                "old_string"
            } else {
                "old_text"
            };
            let new_key = if input.get("new_string").is_some() {
                "new_string"
            } else {
                "new_text"
            };
            if input.get(old_key).is_none() || input.get(new_key).is_none() {
                return None;
            }
            let old_len = input
                .get(old_key)
                .and_then(Value::as_str)
                .map_or(0, str::len);
            let new_len = input
                .get(new_key)
                .and_then(Value::as_str)
                .map_or(0, str::len);
            let redacted = RedactionMarker::mark(input, old_key, old_len);
            Some(RedactionMarker::mark(&redacted, new_key, new_len))
        }
        _ => None,
    }
}

/// Apply a write-input redaction summary.
///
/// Cached-result-shaping has been removed: tool results either fit
/// verbatim or get a visible `truncate_content` marker via
/// `compact_older_messages`. This entry point now only redacts oversized
/// write-tool inputs that the storage / request-build path hands us.
#[must_use]
pub fn apply_summary(input: ToolSummaryInput<'_>) -> Option<SummaryOutput> {
    summarize_write_input(input.tool_name, input.input).map(SummaryOutput::Input)
}
