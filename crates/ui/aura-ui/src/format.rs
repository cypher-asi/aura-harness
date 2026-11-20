//! Tool summary formatting helpers.

/// What a tool call did, as a short phrase: `Read main.rs L1-80`,
/// `Grepped TODO in src`, `Ran cargo test`. While the args are still
/// streaming in, the label is built from the fields that have arrived;
/// it is empty until one has.
pub fn format_tool_label(tool_name: &str, args_json: &str) -> String {
    let args = parse_tool_args(args_json);
    if args.as_object().is_some_and(serde_json::Map::is_empty) && !is_complete_json(args_json) {
        return String::new();
    }
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
        "task" => {
            let agent = match text("subagent_type") {
                Some(kind) => format!("Delegated to @{kind}"),
                None => "Delegated".to_string(),
            };
            match text("prompt").and_then(|p| p.lines().map(str::trim).find(|l| !l.is_empty())) {
                Some(line) => format!("{agent}: {line}"),
                None => agent,
            }
        }
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

/// Tool arguments as JSON. Arguments that are still streaming give an
/// object of the top-level string fields that have fully arrived.
pub fn parse_tool_args(args_json: &str) -> serde_json::Value {
    serde_json::from_str(args_json)
        .unwrap_or_else(|_| serde_json::Value::Object(complete_string_fields(args_json)))
}

/// Whether `args_json` is a whole JSON value rather than a streaming prefix.
pub fn is_complete_json(args_json: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(args_json).is_ok()
}

/// The top-level string fields of a JSON object whose values have fully
/// arrived, from a buffer that may stop anywhere.
fn complete_string_fields(buf: &str) -> serde_json::Map<String, serde_json::Value> {
    let mut fields = serde_json::Map::new();
    let bytes = buf.as_bytes();
    let mut depth = 0usize;
    let mut key: Option<String> = None;
    let mut in_value = false;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => {
                let Some(end) = string_end(bytes, i) else {
                    break;
                };
                if depth == 1 {
                    let Ok(text) = serde_json::from_str::<String>(&buf[i..=end]) else {
                        break;
                    };
                    if in_value {
                        if let Some(key) = key.take() {
                            fields.insert(key, serde_json::Value::String(text));
                        }
                        in_value = false;
                    } else {
                        key = Some(text);
                    }
                }
                i = end;
            }
            b':' if depth == 1 => in_value = true,
            b',' if depth == 1 => {
                key = None;
                in_value = false;
            }
            b'{' | b'[' => depth += 1,
            b'}' | b']' => depth = depth.saturating_sub(1),
            _ => {}
        }
        i += 1;
    }
    fields
}

/// Index of the quote closing the JSON string that opens at `start`.
fn string_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut i = start + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'"' => return Some(i),
            _ => i += 1,
        }
    }
    None
}

/// Last component of `path`.
pub fn file_name(path: &str) -> &str {
    path.trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or(path)
}

/// Format a tool result for display.
///
/// `run_command` results arrive as the `{ ok, exit_code, stdout, stderr }`
/// envelope; they are shown as the command's output text. `task` results
/// are shown as the subagent's reply, after why it stopped if it didn't
/// complete. Anything else (or an envelope that fails to parse) is
/// returned unchanged.
pub fn format_tool_result(tool_name: &str, result: &str) -> String {
    match tool_name {
        "run_command" => format_command_result(result),
        "task" => format_task_result(result).unwrap_or_else(|| result.to_string()),
        _ => result.to_string(),
    }
}

/// A `SubagentResult`: why the subagent stopped unless it completed, its
/// reply, and the files it changed.
fn format_task_result(result: &str) -> Option<String> {
    let outcome = serde_json::from_str::<serde_json::Value>(result).ok()?;
    let reply = outcome.get("final_message")?.as_str()?.trim();
    let exit = outcome.get("exit")?;
    let reason = exit
        .get("reason")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let stopped = match exit.get("kind")?.as_str()? {
        "completed" => None,
        "cancelled" => Some("cancelled".to_string()),
        "timeout" => Some("timed out".to_string()),
        kind if reason.is_empty() => Some(kind.to_string()),
        kind => Some(format!("{kind}: {reason}")),
    };

    let mut parts: Vec<String> = stopped.into_iter().collect();
    if !reply.is_empty() {
        parts.push(reply.to_string());
    } else if parts.is_empty() {
        parts.push("(no reply)".to_string());
    }
    let files: Vec<&str> = outcome
        .get("files_changed")
        .and_then(serde_json::Value::as_array)
        .map(|files| files.iter().filter_map(serde_json::Value::as_str).collect())
        .unwrap_or_default();
    if !files.is_empty() {
        parts.push(format!("files changed: {}", files.join(", ")));
    }
    Some(parts.join("\n\n"))
}

fn format_command_result(result: &str) -> String {
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

/// `950`, `12.4k`, `124k`, `1.2M`.
#[allow(clippy::cast_precision_loss)]
pub fn format_tokens(tokens: u64) -> String {
    match tokens {
        0..1_000 => tokens.to_string(),
        1_000..100_000 => format!("{:.1}k", tokens as f64 / 1_000.0),
        100_000..1_000_000 => format!("{}k", tokens / 1_000),
        _ => format!("{:.1}M", tokens as f64 / 1_000_000.0),
    }
}

/// `<$0.01`, `$0.042`, `$3.50`.
pub fn format_cost(usd: f64) -> String {
    if usd < 0.01 {
        "<$0.01".to_string()
    } else if usd < 1.0 {
        format!("${usd:.3}")
    } else {
        format!("${usd:.2}")
    }
}

#[cfg(test)]
mod tests;
