//! Concise per-tool argument hints for status logging.

use crate::types::ToolCallInfo;

/// Format a concise hint for a tool call's arguments (for status logging).
pub(crate) fn format_tool_arg_hint(tc: &ToolCallInfo) -> String {
    match tc.name.as_str() {
        "read_file" => read_file_hint(tc),
        "write_file" | "edit_file" | "delete_file" => str_field(tc, "path").to_string(),
        "list_files" => str_field(tc, "directory").to_string(),
        "search_code" => search_code_hint(tc),
        "run_command" => str_field(tc, "command").to_string(),
        _ => String::new(),
    }
}

fn str_field<'a>(tc: &'a ToolCallInfo, key: &str) -> &'a str {
    tc.input.get(key).and_then(|v| v.as_str()).unwrap_or("")
}

fn u64_field(tc: &ToolCallInfo, key: &str) -> Option<u64> {
    tc.input.get(key).and_then(serde_json::Value::as_u64)
}

fn read_file_hint(tc: &ToolCallInfo) -> String {
    let path = str_field(tc, "path");
    let start = u64_field(tc, "start_line");
    let end = u64_field(tc, "end_line");
    match (start, end) {
        (Some(s), Some(e)) => format!("{path}:{s}-{e}"),
        (Some(s), None) => format!("{path}:{s}-end"),
        (None, Some(e)) => format!("{path}:1-{e}"),
        (None, None) => path.to_string(),
    }
}

fn search_code_hint(tc: &ToolCallInfo) -> String {
    let pattern = str_field(tc, "pattern");
    if let Some(c) = u64_field(tc, "context_lines") {
        format!("{pattern}, context={c}")
    } else {
        pattern.to_string()
    }
}
