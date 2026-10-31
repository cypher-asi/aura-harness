//! Loading and displaying historical records from the store.

use aura_core_types::{
    ActionResultPayload, AgentId, EffectStatus, Identity, RecordEntry, ToolExecution,
    ToolGateVerdict, ToolProposal, Transaction, TransactionType,
};
use aura_store_db::{ReadStore, RocksStore};
use aura_terminal::{
    events::{AgentSummary, RecordStatus, RecordSummary},
    UiCommand,
};
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::warn;

/// Upper bound on the summary string; the panel truncates to its own
/// width, this only keeps huge prompts from being copied around.
const MAX_INFO_CHARS: usize = 240;

/// Load existing records from the store and send to UI.
pub fn load_existing_records(
    store: &Arc<RocksStore>,
    agent_id: AgentId,
    commands: &mpsc::Sender<UiCommand>,
) {
    let head_seq = match store.get_head_seq(agent_id) {
        Ok(seq) => seq,
        Err(e) => {
            warn!(error = %e, "Failed to get head sequence");
            let _ = commands.try_send(UiCommand::ShowWarning(format!(
                "Could not load record history: {e}"
            )));
            return;
        }
    };

    if head_seq == 0 {
        return;
    }

    let from_seq = head_seq.saturating_sub(99).max(1);
    let records = match store.scan_record(agent_id, from_seq, 100) {
        Ok(entries) => entries,
        Err(e) => {
            warn!(from_seq, head_seq, error = %e, "Failed to load records");
            let _ = commands.try_send(UiCommand::ShowWarning(format!(
                "Could not load {head_seq} historical records: {e}"
            )));
            return;
        }
    };

    for entry in records {
        let timestamp = chrono::DateTime::from_timestamp_millis(entry.tx.ts_ms as i64)
            .map(|dt| dt.format("%H:%M:%S").to_string())
            .unwrap_or_else(|| "??:??:??".to_string());
        let record_summary = build_record_summary(entry.seq, &entry.tx, &entry, timestamp);
        let _ = commands.try_send(UiCommand::NewRecord(record_summary));
    }
}

/// Build the UI summary for a record entry. Shared by the history
/// loader and the live record feed so both render identically.
pub fn build_record_summary(
    seq: u64,
    tx: &Transaction,
    entry: &RecordEntry,
    timestamp: String,
) -> RecordSummary {
    let (tx_kind, sender) = tx_type_label(tx.tx_type);

    let message = truncate_chars(&String::from_utf8_lossy(&tx.payload), 200);

    let effect_count = entry.effects.len();
    let ok_count = entry
        .effects
        .iter()
        .filter(|e| matches!(e.status, EffectStatus::Committed))
        .count();
    let pending_count = entry
        .effects
        .iter()
        .filter(|e| matches!(e.status, EffectStatus::Pending))
        .count();
    let err_count = effect_count - ok_count - pending_count;

    let effect_status = if effect_count == 0 {
        "-".to_string()
    } else if err_count == 0 {
        format!("{ok_count} ok")
    } else {
        format!("{ok_count} ok, {err_count} err")
    };

    let status = if err_count > 0 {
        RecordStatus::Error
    } else if pending_count > 0 {
        RecordStatus::Pending
    } else {
        RecordStatus::Ok
    };

    let error_details: String = entry
        .effects
        .iter()
        .filter(|e| matches!(e.status, EffectStatus::Failed))
        .filter_map(|e| String::from_utf8(e.payload.to_vec()).ok())
        .collect::<Vec<_>>()
        .join("; ");

    let full_hash = entry.context_hash.as_hex();
    let hash_suffix = full_hash[full_hash.len().saturating_sub(4)..].to_string();

    RecordSummary {
        seq,
        timestamp,
        full_hash,
        hash_suffix,
        tx_kind: tx_kind.to_string(),
        sender: sender.to_string(),
        message,
        action_count: entry.actions.len(),
        effect_status,
        status,
        info: summarize_record(tx),
        error_details,
        tx_id: hex::encode(tx.hash.as_bytes()),
        agent_id: hex::encode(tx.agent_id.as_bytes()),
        ts_ms: tx.ts_ms,
    }
}

/// Send initial agent info to the UI.
pub fn send_initial_agent(
    identity: &Identity,
    store: &Arc<RocksStore>,
    commands: &mpsc::Sender<UiCommand>,
) {
    let record_count = store.get_head_seq(identity.agent_id).unwrap_or(0);
    let last_active = chrono::Local::now().format("%H:%M:%S").to_string();

    let agent = AgentSummary {
        id: hex::encode(identity.agent_id.as_bytes()),
        name: identity.name.clone(),
        zns_id: identity.zns_id.clone(),
        is_active: true,
        record_count,
        last_active,
    };

    let _ = commands.try_send(UiCommand::SetAgents(vec![agent]));
    let _ = commands.try_send(UiCommand::SetActiveAgent(hex::encode(
        identity.agent_id.as_bytes(),
    )));
}

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
        TransactionType::ToolExecution => ("ToolRes", "KERNEL"),
        TransactionType::ProcessComplete => ("Complete", "SYSTEM"),
        TransactionType::Reasoning => ("LLM", "KERNEL"),
        TransactionType::SubagentSpawn => ("Spawn", "FLEET"),
    }
}

/// One-line, human-readable summary of what a transaction contains.
pub fn summarize_record(tx: &Transaction) -> String {
    let summary = match tx.tx_type {
        TransactionType::UserPrompt | TransactionType::AgentMsg | TransactionType::Trigger => {
            payload_text(tx)
        }
        TransactionType::ToolProposal => serde_json::from_slice::<ToolProposal>(&tx.payload)
            .map_or_else(
                |_| payload_text(tx),
                |p| summarize_tool_call(&p.tool, &p.args),
            ),
        TransactionType::ToolExecution => serde_json::from_slice::<ToolExecution>(&tx.payload)
            .map_or_else(|_| payload_text(tx), |e| summarize_tool_execution(&e)),
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

fn summarize_tool_call(tool: &str, args: &Value) -> String {
    let detail = if tool == "run_command" {
        extract_cmd_run_command(args)
    } else {
        primary_arg(args).unwrap_or_default()
    };
    if detail.is_empty() {
        tool.to_string()
    } else {
        format!("{tool} {detail}")
    }
}

fn summarize_tool_execution(execution: &ToolExecution) -> String {
    let call = summarize_tool_call(&execution.tool, &execution.args);
    match execution.decision {
        ToolGateVerdict::Denied => match execution.reason.as_deref() {
            Some(reason) if !reason.is_empty() => format!("{call} · denied: {reason}"),
            _ => format!("{call} · denied"),
        },
        ToolGateVerdict::PendingApproval => format!("{call} · awaiting approval"),
        ToolGateVerdict::Approved if execution.is_error => {
            let first_line = execution
                .result
                .as_deref()
                .and_then(|r| r.lines().find(|l| !l.trim().is_empty()))
                .unwrap_or("");
            if first_line.is_empty() {
                format!("{call} · err")
            } else {
                format!("{call} · err: {first_line}")
            }
        }
        ToolGateVerdict::Approved => match execution.result.as_deref() {
            Some(result) => format!("{call} · ok {}", format_bytes(result.len())),
            None => format!("{call} · ok"),
        },
    }
}

fn summarize_action_result(result: &ActionResultPayload) -> String {
    let outcome = if result.success { "ok" } else { "failed" };
    let duration = format_duration_ms(result.duration_ms);
    match result.exit_code {
        Some(code) => format!("{outcome} · exit {code} · {duration}"),
        None => format!("{outcome} · {duration}"),
    }
}

fn summarize_reasoning(payload: &Value) -> String {
    if payload.get("status").and_then(Value::as_str) == Some("failed") {
        let error = payload.get("error").and_then(Value::as_str).unwrap_or("");
        return format!("failed: {error}");
    }

    let mut parts = Vec::new();
    if let Some(model) = payload.get("model").and_then(Value::as_str) {
        parts.push(model.to_string());
    }
    let input = payload.get("input_tokens").and_then(Value::as_u64);
    let output = payload.get("output_tokens").and_then(Value::as_u64);
    if input.is_some() || output.is_some() {
        parts.push(format!(
            "{}/{} tok",
            input.map_or_else(|| "?".to_string(), format_tokens),
            output.map_or_else(|| "?".to_string(), format_tokens),
        ));
    }
    if let Some(stop) = payload.get("stop_reason").and_then(Value::as_str) {
        parts.push(stop.to_string());
    }
    let tools: Vec<&str> = payload
        .get("tool_uses")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    if !tools.is_empty() {
        parts.push(format!("tools: {}", tools.join(", ")));
    }
    parts.join(" · ")
}

fn summarize_system(payload: &Value) -> String {
    let kind = payload
        .get("system_kind")
        .and_then(Value::as_str)
        .unwrap_or("system");
    let detail: Vec<&str> = ["method", "event", "action", "scope", "phase"]
        .iter()
        .filter_map(|key| payload.get(*key).and_then(Value::as_str))
        .collect();
    if detail.is_empty() {
        kind.to_string()
    } else {
        format!("{kind} · {}", detail.join(" "))
    }
}

fn summarize_spawn(payload: &Value) -> String {
    let mut parts = Vec::new();
    if let Some(mode) = payload.get("spawn_mode").and_then(Value::as_str) {
        parts.push(mode.to_string());
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
    parts.join(" · ")
}

/// Pick the argument that best identifies what a tool call targets.
fn primary_arg(args: &Value) -> Option<String> {
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

/// Extract the command string from cmd_run tool arguments.
fn extract_cmd_run_command(args: &Value) -> String {
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

fn json_payload(tx: &Transaction) -> Option<Value> {
    serde_json::from_slice::<Value>(&tx.payload)
        .ok()
        .filter(Value::is_object)
}

fn payload_text(tx: &Transaction) -> String {
    std::str::from_utf8(&tx.payload)
        .map(str::to_string)
        .unwrap_or_default()
}

fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let kept: String = text.chars().take(max_chars.saturating_sub(3)).collect();
    format!("{kept}...")
}

/// `n / unit` rendered with one truncated decimal, e.g. `1234 / 1000` -> `1.2`.
fn one_decimal(n: u64, unit: u64) -> String {
    let tenths = n.saturating_mul(10) / unit;
    format!("{}.{}", tenths / 10, tenths % 10)
}

fn format_tokens(n: u64) -> String {
    if n < 1000 {
        n.to_string()
    } else {
        format!("{}k", one_decimal(n, 1000))
    }
}

fn format_bytes(n: usize) -> String {
    let n = n as u64;
    if n < 1024 {
        format!("{n}B")
    } else if n < 1024 * 1024 {
        format!("{}KB", one_decimal(n, 1024))
    } else {
        format!("{}MB", one_decimal(n, 1024 * 1024))
    }
}

fn format_duration_ms(ms: u64) -> String {
    if ms < 1000 {
        format!("{ms}ms")
    } else {
        format!("{}s", one_decimal(ms, 1000))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tx(tx_type: TransactionType, payload: impl Into<Vec<u8>>) -> Transaction {
        Transaction::new_chained(AgentId::generate(), tx_type, payload.into(), None)
    }

    fn json_tx(tx_type: TransactionType, payload: &Value) -> Transaction {
        tx(tx_type, serde_json::to_vec(payload).unwrap())
    }

    #[test]
    fn labels_fit_kind_column() {
        for t in [
            TransactionType::UserPrompt,
            TransactionType::AgentMsg,
            TransactionType::Trigger,
            TransactionType::ActionResult,
            TransactionType::System,
            TransactionType::SessionStart,
            TransactionType::ToolProposal,
            TransactionType::ToolExecution,
            TransactionType::ProcessComplete,
            TransactionType::Reasoning,
            TransactionType::SubagentSpawn,
        ] {
            assert!(tx_type_label(t).0.len() <= 8, "{t:?} label too long");
        }
    }

    #[test]
    fn prompt_collapses_to_one_line() {
        let t = tx(
            TransactionType::UserPrompt,
            "fix the login\n\n  redirect bug\tplease",
        );
        assert_eq!(summarize_record(&t), "fix the login redirect bug please");
    }

    #[test]
    fn long_prompt_is_capped_char_safely() {
        let t = tx(TransactionType::UserPrompt, "修".repeat(500));
        let out = summarize_record(&t);
        assert_eq!(out.chars().count(), MAX_INFO_CHARS);
        assert!(out.ends_with("..."));
    }

    #[test]
    fn tool_proposal_shows_primary_arg() {
        let t = json_tx(
            TransactionType::ToolProposal,
            &json!({"tool_use_id": "t1", "tool": "read_file", "args": {"path": "src/main.rs"}}),
        );
        assert_eq!(summarize_record(&t), "read_file src/main.rs");
    }

    #[test]
    fn tool_proposal_run_command() {
        let t = json_tx(
            TransactionType::ToolProposal,
            &json!({"tool_use_id": "t1", "tool": "run_command",
                    "args": {"program": "cargo", "args": ["test", "-p", "x"]}}),
        );
        assert_eq!(summarize_record(&t), "run_command cargo test -p x");
    }

    fn execution(decision: ToolGateVerdict, result: Option<&str>, is_error: bool) -> Transaction {
        let exec = ToolExecution {
            tool_use_id: "t1".into(),
            tool: "read_file".into(),
            args: json!({"path": "a.rs"}),
            decision,
            reason: Some("policy says no".into()),
            result: result.map(str::to_string),
            is_error,
            parent_agent_id: AgentId::generate(),
            originating_user_id: "u".into(),
        };
        tx(
            TransactionType::ToolExecution,
            serde_json::to_vec(&exec).unwrap(),
        )
    }

    #[test]
    fn tool_execution_outcomes() {
        assert_eq!(
            summarize_record(&execution(ToolGateVerdict::Approved, Some("hello"), false)),
            "read_file a.rs · ok 5B"
        );
        assert_eq!(
            summarize_record(&execution(
                ToolGateVerdict::Approved,
                Some("\nNo such file\nmore"),
                true
            )),
            "read_file a.rs · err: No such file"
        );
        assert_eq!(
            summarize_record(&execution(ToolGateVerdict::Denied, None, false)),
            "read_file a.rs · denied: policy says no"
        );
    }

    #[test]
    fn reasoning_completed_and_failed() {
        let ok = json_tx(
            TransactionType::Reasoning,
            &json!({"model": "claude-opus-4-6", "stop_reason": "ToolUse",
                    "input_tokens": 1234, "output_tokens": 340,
                    "tool_uses": ["read_file", "grep"]}),
        );
        assert_eq!(
            summarize_record(&ok),
            "claude-opus-4-6 · 1.2k/340 tok · ToolUse · tools: read_file, grep"
        );
        let failed = json_tx(
            TransactionType::Reasoning,
            &json!({"status": "failed", "error": "rate limited"}),
        );
        assert_eq!(summarize_record(&failed), "failed: rate limited");
    }

    #[test]
    fn system_kind_and_detail() {
        let t = json_tx(
            TransactionType::System,
            &json!({"system_kind": "domain_mutation", "phase": "request", "method": "create_spec"}),
        );
        assert_eq!(
            summarize_record(&t),
            "domain_mutation · create_spec request"
        );
    }

    #[test]
    fn session_start() {
        let t = tx(TransactionType::SessionStart, "session_start");
        assert_eq!(summarize_record(&t), "new session");
    }

    #[test]
    fn unparseable_json_falls_back_to_text() {
        let t = tx(TransactionType::Reasoning, "not json");
        assert_eq!(summarize_record(&t), "not json");
        let binary = tx(TransactionType::ToolProposal, vec![0xff, 0xfe]);
        assert_eq!(summarize_record(&binary), "");
    }
}
