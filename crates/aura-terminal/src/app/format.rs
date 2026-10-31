//! Tool summary formatting helpers.

/// Format a tool summary from tool name and JSON args for display.
pub(super) fn format_tool_summary(tool_name: &str, args_json: &str) -> String {
    serde_json::from_str::<serde_json::Value>(args_json).map_or_else(
        |_| {
            if args_json.len() > 50 {
                format!("{}...", &args_json[..47])
            } else {
                args_json.to_string()
            }
        },
        |args| format_tool_summary_from_value(tool_name, &args),
    )
}

/// Format a tool result for display.
///
/// `run_command` results arrive as the `{ ok, exit_code, stdout, stderr }`
/// envelope; they are shown as the command's output text. Anything else
/// (or an envelope that fails to parse) is returned unchanged.
pub(super) fn format_tool_result(tool_name: &str, result: &str) -> String {
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
        if let Some(code) = envelope.get("exit_code").and_then(serde_json::Value::as_i64) {
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
        "read_file" => args
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        "write_file" => args
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        "fs_list" | "list_dir" => args
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or(".")
            .to_string(),
        "fs_search" | "search" | "glob" => args
            .get("pattern")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        _ => {
            let summary: Vec<String> = args
                .as_object()
                .map(|obj| {
                    obj.iter()
                        .take(3)
                        .map(|(k, v)| {
                            let val_str = match v {
                                serde_json::Value::String(s) => {
                                    if s.len() > 30 {
                                        format!("{}...", &s[..27])
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
