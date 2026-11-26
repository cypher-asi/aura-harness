//! One-line text summaries and formatting helpers.

use super::*;

/// Map a transaction type to its display label and sender name.
///
/// Labels are kept to at most 8 characters so they fit the Record
/// panel's fixed-width kind column.
pub fn tx_type_label(tx_type: TransactionType) -> (&'static str, &'static str) {
    match tx_type {
        TransactionType::UserPrompt => ("Prompt", "USER"),
        TransactionType::ActionResult => ("Action", "SYSTEM"),
        TransactionType::System => ("System", "SYSTEM"),
        TransactionType::AgentMsg => ("Reply", "AURA"),
        TransactionType::Trigger => ("Trigger", "SYSTEM"),
        TransactionType::SessionStart => ("Session", "SYSTEM"),
        TransactionType::ToolProposal => ("ToolCall", "LLM"),
        TransactionType::PolicyDecision => ("Policy", "KERNEL"),
        TransactionType::ToolExecution => ("ToolRes", "KERNEL"),
        TransactionType::ProcessComplete => ("Complete", "SYSTEM"),
        TransactionType::Reasoning => ("LLM", "KERNEL"),
        TransactionType::SubagentSpawn => ("Spawn", "AGENT"),
        TransactionType::Outcome => ("Outcome", "GATE"),
    }
}

/// One-line, human-readable summary of what a transaction contains.
pub fn summarize_record(tx: &Transaction) -> String {
    let summary = match tx.tx_type {
        TransactionType::UserPrompt => prompt_text(tx),
        TransactionType::AgentMsg | TransactionType::Trigger => payload_text(tx),
        TransactionType::Outcome => tx.outcome_payload().map_or_else(String::new, |outcome| {
            std::iter::once(outcome.signal.as_str().to_string())
                .chain(outcome_detail(&outcome))
                .collect::<Vec<_>>()
                .join(" · ")
        }),
        TransactionType::ToolProposal => serde_json::from_slice::<ToolProposal>(&tx.payload)
            .map_or_else(
                |_| payload_text(tx),
                |p| summarize_tool_call(&p.tool, &p.args),
            ),
        TransactionType::ToolExecution => serde_json::from_slice::<ToolExecution>(&tx.payload)
            .map_or_else(|_| payload_text(tx), |e| summarize_tool_execution(&e)),
        TransactionType::PolicyDecision => serde_json::from_slice::<PolicyDecision>(&tx.payload)
            .map_or_else(|_| payload_text(tx), |d| summarize_policy_decision(&d)),
        TransactionType::ActionResult | TransactionType::ProcessComplete => {
            serde_json::from_slice::<ActionResultPayload>(&tx.payload)
                .map_or_else(|_| payload_text(tx), |r| summarize_action_result(&r))
        }
        TransactionType::Reasoning => {
            json_payload(tx).map_or_else(|| payload_text(tx), |v| summarize_reasoning(&v))
        }
        TransactionType::System => {
            json_payload(tx).map_or_else(|| payload_text(tx), |v| summarize_system(&v))
        }
        TransactionType::SessionStart => "new session".to_string(),
        TransactionType::SubagentSpawn => {
            json_payload(tx).map_or_else(String::new, |v| summarize_spawn(&v))
        }
    };
    truncate_chars(&collapse_whitespace(&summary), MAX_INFO_CHARS)
}

pub(crate) fn summarize_tool_call(tool: &str, args: &Value) -> String {
    let target = tool_target(tool, args);
    if target.is_empty() {
        tool.to_string()
    } else {
        format!("{tool} {target}")
    }
}

/// What a tool call acts on: the command line for `run_command`, else
/// its primary argument.
pub(crate) fn tool_target(tool: &str, args: &Value) -> String {
    if tool == "run_command" {
        extract_cmd_run_command(args)
    } else {
        primary_arg(args).unwrap_or_default()
    }
}

pub(super) fn summarize_tool_execution(execution: &ToolExecution) -> String {
    format!(
        "{} · {}",
        summarize_tool_call(&execution.tool, &execution.args),
        tool_outcome(execution)
    )
}

/// How a tool call ended, e.g. `ok 5B` or `denied: reason`.
pub(crate) fn tool_outcome(execution: &ToolExecution) -> String {
    match execution.decision {
        ToolGateVerdict::Denied => match execution.reason.as_deref() {
            Some(reason) if !reason.is_empty() => format!("denied: {reason}"),
            _ => "denied".to_string(),
        },
        ToolGateVerdict::PendingApproval => "awaiting approval".to_string(),
        ToolGateVerdict::Approved if execution.is_error => {
            let first_line = execution
                .result
                .as_deref()
                .and_then(|r| r.lines().find(|l| !l.trim().is_empty()))
                .unwrap_or("");
            if first_line.is_empty() {
                "err".to_string()
            } else {
                format!("err: {first_line}")
            }
        }
        ToolGateVerdict::Approved => match execution.result.as_deref() {
            Some(result) => format!("ok {}", format_bytes(result.len())),
            None => "ok".to_string(),
        },
    }
}

pub(crate) fn summarize_policy_decision(decision: &PolicyDecision) -> String {
    format!(
        "{} · {} · {}",
        decision.tool,
        policy_verdict(decision.verdict),
        decision.decided_by
    )
}

pub(super) const fn policy_verdict(verdict: ToolGateVerdict) -> &'static str {
    match verdict {
        ToolGateVerdict::Approved => "allow",
        ToolGateVerdict::Denied => "deny",
        ToolGateVerdict::PendingApproval => "ask",
    }
}

pub(crate) fn summarize_action_result(result: &ActionResultPayload) -> String {
    let outcome = if result.success { "ok" } else { "failed" };
    let duration = format_duration_ms(result.duration_ms);
    match result.exit_code {
        Some(code) => format!("{outcome} · exit {code} · {duration}"),
        None => format!("{outcome} · {duration}"),
    }
}

pub(super) fn summarize_reasoning(payload: &Value) -> String {
    if let Some(error) = reasoning_error(payload) {
        return format!("failed: {error}");
    }

    let mut parts = Vec::new();
    if let Some(model) = payload.get("model").and_then(Value::as_str) {
        parts.push(model.to_string());
    }
    parts.extend(reasoning_tokens(payload));
    if let Some(stop) = payload.get("stop_reason").and_then(Value::as_str) {
        parts.push(stop.to_string());
    }
    parts.extend(reasoning_tools(payload));
    parts.join(" · ")
}

/// Input/output token counts of a model call, e.g. `1.2k/300 tok`.
pub(crate) fn reasoning_tokens(payload: &Value) -> Option<String> {
    let input = payload.get("input_tokens").and_then(Value::as_u64);
    let output = payload.get("output_tokens").and_then(Value::as_u64);
    (input.is_some() || output.is_some()).then(|| {
        format!(
            "{}/{} tok",
            input.map_or_else(|| "?".to_string(), format_tokens),
            output.map_or_else(|| "?".to_string(), format_tokens),
        )
    })
}

/// Tools a model call asked for, e.g. `tools: read_file, run_command`.
pub(crate) fn reasoning_tools(payload: &Value) -> Option<String> {
    let tools: Vec<&str> = payload
        .get("tool_uses")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    (!tools.is_empty()).then(|| format!("tools: {}", tools.join(", ")))
}

pub(crate) fn summarize_system(payload: &Value) -> String {
    let kind = system_kind(payload);
    let mode = |key: &str| payload.get(key).and_then(Value::as_str);
    if let (Some(from), Some(to)) = (mode("from"), mode("to")) {
        return format!("{kind} · {from} → {to}");
    }
    let detail = system_detail(payload);
    if detail.is_empty() {
        kind.to_string()
    } else {
        format!("{kind} · {}", detail.join(" "))
    }
}

pub(super) fn system_kind(payload: &Value) -> &str {
    payload
        .get("system_kind")
        .and_then(Value::as_str)
        .unwrap_or("system")
}

pub(super) fn system_detail(payload: &Value) -> Vec<&str> {
    ["method", "event", "action", "scope", "phase"]
        .iter()
        .filter_map(|key| payload.get(*key).and_then(Value::as_str))
        .collect()
}

pub(super) fn summarize_spawn(payload: &Value) -> String {
    let mut parts = Vec::new();
    if let Some(mode) = payload.get("spawn_mode").and_then(Value::as_str) {
        parts.push(mode.to_string());
    }
    parts.extend(spawn_detail(payload));
    parts.join(" · ")
}

/// Child agent, the permanent agent it is an instance of, the todo it
/// does and the override count of a spawn, without the spawn mode.
pub(super) fn spawn_detail(payload: &Value) -> Vec<String> {
    let mut parts = Vec::new();
    let text = |key| payload.get(key).and_then(Value::as_str);
    if let Some(name) = text("child_name").or_else(|| text("subagent_type")) {
        parts.push(name.to_string());
    }
    if let Some(todo) = text("todo_id") {
        parts.push(format!("todo {todo}"));
    }
    if let Some(child) = payload.get("child_agent_id").and_then(Value::as_str) {
        let short: String = child.chars().take(8).collect();
        parts.push(format!("child {short}"));
    }
    let overrides = payload
        .pointer("/override_manifest/applied")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    if overrides > 0 {
        parts.push(format!("{overrides} overrides"));
    }
    parts
}

/// Pick the argument that best identifies what a tool call targets.
pub(super) fn primary_arg(args: &Value) -> Option<String> {
    const KEYS: &[&str] = &[
        "path",
        "file_path",
        "pattern",
        "query",
        "url",
        "command",
        "name",
    ];
    KEYS.iter()
        .find_map(|key| args.get(*key).and_then(Value::as_str))
        .map(str::to_string)
}

/// Whether `tx` is a tool proposal the user made directly.
pub(super) fn is_user_proposal(tx: &Transaction) -> bool {
    tx.tx_type == TransactionType::ToolProposal
        && serde_json::from_slice::<ToolProposal>(&tx.payload)
            .is_ok_and(|p| p.source.as_deref() == Some(crate::event_loop::run_command::USER_SOURCE))
}

/// Extract the command string from cmd_run tool arguments.
pub(super) fn extract_cmd_run_command(args: &Value) -> String {
    if let Some(script) = ["shell_script", "command"]
        .iter()
        .find_map(|key| args.get(*key).and_then(Value::as_str))
    {
        return script.to_string();
    }
    let program = args["program"].as_str().unwrap_or("");
    let cmd_args: Vec<&str> = args["args"]
        .as_array()
        .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();

    if cmd_args.is_empty() {
        program.to_string()
    } else {
        format!("{program} {}", cmd_args.join(" "))
    }
}

/// Tool name (or the command line for `run_command`) of a tool proposal
/// or execution; empty for other transaction types.
pub fn extract_tool_info(tx: &Transaction) -> String {
    let call = match tx.tx_type {
        TransactionType::ToolProposal => serde_json::from_slice::<ToolProposal>(&tx.payload)
            .ok()
            .map(|p| (p.tool, p.args)),
        TransactionType::ToolExecution => serde_json::from_slice::<ToolExecution>(&tx.payload)
            .ok()
            .map(|e| (e.tool, e.args)),
        _ => None,
    };
    match call {
        Some((tool, args)) if tool == "run_command" => extract_cmd_run_command(&args),
        Some((tool, _)) => tool,
        None => String::new(),
    }
}

pub(crate) fn json_payload(tx: &Transaction) -> Option<Value> {
    serde_json::from_slice::<Value>(&tx.payload)
        .ok()
        .filter(Value::is_object)
}

/// The text of a prompt entry; empty when its payload is not a prompt.
pub(super) fn prompt_text(tx: &Transaction) -> String {
    tx.prompt_payload()
        .map(|prompt| prompt.text)
        .unwrap_or_default()
}

/// `ok`, `failed`, or `-` when an outcome says neither.
pub(super) const fn outcome_verdict(success: Option<bool>) -> &'static str {
    match success {
        Some(true) => "ok",
        Some(false) => "failed",
        None => "-",
    }
}

/// Category, model, cost and wall time of an outcome's work.
pub(super) fn outcome_detail(outcome: &aura_core_types::OutcomePayload) -> Vec<String> {
    vec![
        outcome.category.to_string(),
        outcome.model.clone(),
        format!("${:.4}", outcome.cost_usd),
        format_duration_ms(outcome.latency_ms),
    ]
}

pub(crate) fn payload_text(tx: &Transaction) -> String {
    std::str::from_utf8(&tx.payload)
        .map(str::to_string)
        .unwrap_or_default()
}

pub(crate) fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(crate) fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let kept: String = text.chars().take(max_chars.saturating_sub(3)).collect();
    format!("{kept}...")
}

/// `n / unit` rendered with one truncated decimal, e.g. `1234 / 1000` -> `1.2`.
pub(super) fn one_decimal(n: u64, unit: u64) -> String {
    let tenths = n.saturating_mul(10) / unit;
    format!("{}.{}", tenths / 10, tenths % 10)
}

pub(crate) fn format_tokens(n: u64) -> String {
    if n < 1000 {
        n.to_string()
    } else {
        format!("{}k", one_decimal(n, 1000))
    }
}

pub(super) fn format_bytes(n: usize) -> String {
    let n = n as u64;
    if n < 1024 {
        format!("{n}B")
    } else if n < 1024 * 1024 {
        format!("{}KB", one_decimal(n, 1024))
    } else {
        format!("{}MB", one_decimal(n, 1024 * 1024))
    }
}

/// Why a recorded call didn't go to the tool executor, for the TOOLCALL
/// row. `None` for calls the panel already marks some other way (the
/// sender column shows user commands).
pub(super) fn proposal_source_label(source: &str) -> Option<String> {
    let label = match source {
        crate::event_loop::run_command::USER_SOURCE => return None,
        "cache" => "cached result",
        "steering_block" => "blocked by steering",
        "invalid_tool_use" => "invalid call",
        "task_guard" => "blocked by task guard",
        "task_executor" => "handled by task runner",
        other => other,
    };
    Some(label.to_string())
}

pub(crate) fn format_duration_ms(ms: u64) -> String {
    if ms < 1000 {
        format!("{ms}ms")
    } else {
        format!("{}s", one_decimal(ms, 1000))
    }
}
