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

/// Format a tool summary given parsed JSON args.
fn format_tool_summary_from_value(tool_name: &str, args: &serde_json::Value) -> String {
    match tool_name {
        "run_command" => {
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
