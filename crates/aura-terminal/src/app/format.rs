//! Tool summary formatting helpers.

/// What a tool call did, as a short phrase: `Read main.rs L1-80`,
/// `Grepped TODO in src`, `Ran cargo test`. Empty while the args are not
/// yet valid JSON (they stream in piece by piece).
pub(crate) fn format_tool_label(tool_name: &str, args_json: &str) -> String {
    let Ok(args) = serde_json::from_str::<serde_json::Value>(args_json) else {
        return String::new();
    };
    let text = |key: &str| {
        args.get(key)
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.is_empty())
    };
    let file = |verb: &str| match text("path") {
        Some(path) => format!("{verb} {}", file_name(path)),
        None => verb.to_string(),
    };
    let in_path = |label: String| match text("path") {
        Some(path) => format!("{label} in {path}"),
        None => label,
    };
    match tool_name {
        "read_file" => {
            let line = |key: &str| args.get(key).and_then(serde_json::Value::as_u64);
            match (line("start_line"), line("end_line")) {
                (Some(start), Some(end)) => format!("{} L{start}-{end}", file("Read")),
                (Some(start), None) => format!("{} from L{start}", file("Read")),
                (None, Some(end)) => format!("{} L1-{end}", file("Read")),
                (None, None) => file("Read"),
            }
        }
        "write_file" => file("Wrote"),
        "edit_file" => file("Edited"),
        "delete_file" => file("Deleted"),
        "stat_file" => file("Checked"),
        "list_files" => format!("Listed {}", text("path").unwrap_or(".")),
        "search_code" => {
            let label = in_path(format!("Grepped {}", text("pattern").unwrap_or_default()));
            match text("include") {
                Some(include) => format!("{label} ({include})"),
                None => label,
            }
        }
        "find_files" => in_path(format!("Found {}", text("pattern").unwrap_or_default())),
        "run_command" => format!("Ran {}", format_tool_summary_from_value(tool_name, &args)),
        _ => {
            let summary = format_tool_summary_from_value(tool_name, &args);
            if summary.is_empty() {
                tool_name.to_string()
            } else {
                format!("{tool_name} {summary}")
            }
        }
    }
}

/// Last component of `path`.
pub(crate) fn file_name(path: &str) -> &str {
    path.trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or(path)
}

/// Format a tool result for display.
///
/// `run_command` results arrive as the `{ ok, exit_code, stdout, stderr }`
/// envelope; they are shown as the command's output text. Anything else
/// (or an envelope that fails to parse) is returned unchanged.
pub(crate) fn format_tool_result(tool_name: &str, result: &str) -> String {
    if tool_name != "run_command" {
        return result.to_string();
    }
    let Ok(envelope) = serde_json::from_str::<serde_json::Value>(result) else {
        return result.to_string();
    };
    let field = |key: &str| {
        envelope
            .get(key)
            .and_then(serde_json::Value::as_str)
            .map_or("", str::trim_end)
    };
    let (stdout, stderr) = (field("stdout"), field("stderr"));
    let ok = envelope
        .get("ok")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(true);

    let mut parts = Vec::new();
    if !ok {
        if let Some(code) = envelope
            .get("exit_code")
            .and_then(serde_json::Value::as_i64)
        {
            parts.push(format!("exit {code}"));
        }
    }
    if !stdout.is_empty() {
        parts.push(stdout.to_string());
    }
    if !stderr.is_empty() {
        parts.push(format!("stderr:\n{stderr}"));
    }
    if stdout.is_empty() && stderr.is_empty() {
        parts.push("(no output)".to_string());
    }
    parts.join("\n")
}

/// Format a tool summary given parsed JSON args.
fn format_tool_summary_from_value(tool_name: &str, args: &serde_json::Value) -> String {
    match tool_name {
        "run_command" => {
            if let Some(script) = ["shell_script", "command"]
                .iter()
                .find_map(|key| args.get(*key).and_then(|v| v.as_str()))
            {
                return script.to_string();
            }
            let program = args.get("program").and_then(|v| v.as_str()).unwrap_or("");
            let cmd_args = args
                .get("args")
                .and_then(|v| {
                    v.as_array().map(|arr| {
                        arr.iter()
                            .filter_map(|a| a.as_str())
                            .collect::<Vec<_>>()
                            .join(" ")
                    })
                })
                .unwrap_or_default();
            if cmd_args.is_empty() {
                program.to_string()
            } else {
                format!("{program} {cmd_args}")
            }
        }
        _ => {
            let summary: Vec<String> = args
                .as_object()
                .map(|obj| {
                    obj.iter()
                        .take(3)
                        .map(|(k, v)| {
                            let val_str = match v {
                                serde_json::Value::String(s) => {
                                    if s.chars().count() > 30 {
                                        format!("{}...", s.chars().take(27).collect::<String>())
                                    } else {
                                        s.clone()
                                    }
                                }
                                _ => v.to_string(),
                            };
                            format!("{k}={val_str}")
                        })
                        .collect()
                })
                .unwrap_or_default();
            summary.join(", ")
        }
    }
}

#[cfg(test)]
mod tests;
