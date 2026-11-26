//! Record panel summaries, columns and policy details.

use super::*;

pub(super) fn history_message(tx: &Transaction) -> Option<MessageData> {
    if is_user_proposal(tx) {
        return user_command_message(tx);
    }
    let (role, content) = history_content(tx)?;
    if content.trim().is_empty() {
        return None;
    }
    Some(MessageData {
        role,
        content,
        is_streaming: false,
        timestamp_ms: Some(tx.ts_ms),
        command: None,
        activity: None,
        model: None,
        usage: None,
        sources: Vec::new(),
        boot: None,
    })
}

/// The chat message of a command the user ran, if it has one.
fn user_command_message(tx: &Transaction) -> Option<MessageData> {
    let command = extract_tool_info(tx);
    (!command.is_empty()).then(|| MessageData {
        role: MessageRole::User,
        content: command.clone(),
        is_streaming: false,
        timestamp_ms: Some(tx.ts_ms),
        command: Some(command),
        activity: None,
        model: None,
        usage: None,
        sources: Vec::new(),
        boot: None,
    })
}

/// The role and text `tx` shows in the chat history, if any.
fn history_content(tx: &Transaction) -> Option<(MessageRole, String)> {
    let content = match tx.tx_type {
        TransactionType::UserPrompt => (MessageRole::User, prompt_text(tx)),
        TransactionType::AgentMsg => (
            MessageRole::Assistant,
            String::from_utf8_lossy(&tx.payload).into_owned(),
        ),
        TransactionType::ToolProposal => {
            let info = extract_tool_info(tx);
            if info.is_empty() {
                return None;
            }
            (MessageRole::System, format!("$ {info}"))
        }
        TransactionType::PolicyDecision => (
            MessageRole::System,
            serde_json::from_slice::<PolicyDecision>(&tx.payload)
                .ok()
                .and_then(|decision| policy_chat_line(&decision))?,
        ),
        TransactionType::SessionStart => (MessageRole::System, "── new session ──".to_string()),
        _ => return None,
    };
    Some(content)
}

/// Build the UI summary for a record entry. Shared by the history
/// loader and the live record feed so both render identically.
pub fn build_record_summary(
    seq: u64,
    tx: &Transaction,
    entry: &RecordEntry,
    timestamp: String,
) -> RecordSummary {
    let (tx_kind, mut sender) = tx_type_label(tx.tx_type);
    if is_user_proposal(tx) {
        sender = "USER";
    }

    let message = record_message(&tx.payload);
    let (effect_status, status) = effect_status(entry);
    let error_details = error_details(entry);

    let full_hash = entry.context_hash.as_hex();
    let hash_suffix = full_hash[full_hash.len().saturating_sub(4)..].to_string();
    let columns = record_columns(tx, entry, sender).unwrap_or_default();

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
        info: summarize_entry(tx, entry),
        what: columns.what,
        group: columns.group,
        detail: truncate_chars(&collapse_whitespace(&columns.detail), MAX_INFO_CHARS),
        error_details,
        tx_id: hex::encode(tx.hash.as_bytes()),
        agent_id: hex::encode(tx.agent_id.as_bytes()),
        ts_ms: tx.ts_ms,
        policy: policy_detail(tx),
    }
}

/// The effect column ("-", "N ok", "N ok, M err") and the status of `entry`.
fn effect_status(entry: &RecordEntry) -> (String, RecordStatus) {
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
    (effect_status, status)
}

/// The payloads of `entry`'s failed effects, joined with "; ".
fn error_details(entry: &RecordEntry) -> String {
    entry
        .effects
        .iter()
        .filter(|e| matches!(e.status, EffectStatus::Failed))
        .filter_map(|e| String::from_utf8(e.payload.to_vec()).ok())
        .collect::<Vec<_>>()
        .join("; ")
}

/// The payload as shown in the record detail view: JSON objects and
/// arrays pretty-printed, anything else truncated.
pub(super) fn record_message(payload: &[u8]) -> String {
    match serde_json::from_slice::<Value>(payload) {
        Ok(value @ (Value::Object(_) | Value::Array(_))) => truncate_chars(
            &serde_json::to_string_pretty(&value).unwrap_or_default(),
            MAX_JSON_MESSAGE_CHARS,
        ),
        _ => truncate_chars(&String::from_utf8_lossy(payload), MAX_MESSAGE_CHARS),
    }
}

/// Like [`summarize_record`], but tool executions read their output from
/// the entry's effects (the kernel no longer copies it into the payload).
pub(super) fn summarize_entry(tx: &Transaction, entry: &RecordEntry) -> String {
    if tx.tx_type == TransactionType::ToolExecution {
        if let Some(execution) = tool_execution(entry) {
            return truncate_chars(
                &collapse_whitespace(&summarize_tool_execution(&execution)),
                MAX_INFO_CHARS,
            );
        }
    }
    summarize_record(tx)
}

/// The Record panel's fixed columns for one entry.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct RecordColumns {
    /// What the entry is about: tool name, LLM stop reason, sender.
    pub(super) what: String,
    /// Category of `what`: tool group, LLM error class, model family.
    pub(super) group: String,
    /// The rest of the summary, without what the columns already show.
    pub(super) detail: String,
}

impl RecordColumns {
    fn new(what: impl Into<String>, group: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            what: what.into(),
            group: group.into(),
            detail: detail.into(),
        }
    }
}

/// Split an entry into the Record panel's columns; `None` when the
/// payload doesn't parse, so the panel falls back to the summary.
pub(super) fn record_columns(
    tx: &Transaction,
    entry: &RecordEntry,
    sender: &str,
) -> Option<RecordColumns> {
    let columns = match tx.tx_type {
        TransactionType::UserPrompt => RecordColumns::new(sender, "text", prompt_text(tx)),
        TransactionType::AgentMsg | TransactionType::Trigger => {
            RecordColumns::new(sender, "text", payload_text(tx))
        }
        TransactionType::Outcome => {
            let outcome = tx.outcome_payload()?;
            RecordColumns::new(
                outcome.signal.as_str(),
                outcome_verdict(outcome.success),
                outcome_detail(&outcome).join(" · "),
            )
        }
        TransactionType::ToolProposal => proposal_columns(tx)?,
        TransactionType::ToolExecution => execution_columns(entry)?,
        TransactionType::PolicyDecision => decision_columns(tx)?,
        TransactionType::ActionResult | TransactionType::ProcessComplete => {
            action_result_columns(tx)?
        }
        TransactionType::Reasoning => reasoning_columns(&json_payload(tx)?),
        TransactionType::System => {
            let payload = json_payload(tx)?;
            RecordColumns::new(system_kind(&payload), "", system_detail(&payload).join(" "))
        }
        TransactionType::SessionStart => RecordColumns::new("start", "", ""),
        TransactionType::SubagentSpawn => {
            let payload = json_payload(tx)?;
            let mode = payload
                .get("spawn_mode")
                .and_then(Value::as_str)
                .unwrap_or("spawn");
            RecordColumns::new(mode, "agent", spawn_detail(&payload).join(" · "))
        }
    };
    Some(columns)
}

/// Columns for a tool proposal: tool, tool group, target and source.
fn proposal_columns(tx: &Transaction) -> Option<RecordColumns> {
    let proposal = serde_json::from_slice::<ToolProposal>(&tx.payload).ok()?;
    let detail: Vec<String> = [
        Some(tool_target(&proposal.tool, &proposal.args)),
        proposal.source.as_deref().and_then(proposal_source_label),
    ]
    .into_iter()
    .flatten()
    .filter(|part| !part.is_empty())
    .collect();
    Some(RecordColumns::new(
        proposal.tool.as_str(),
        tool_group(&proposal.tool),
        detail.join(" · "),
    ))
}

/// Columns for a tool execution: tool, tool group, target, outcome and time.
fn execution_columns(entry: &RecordEntry) -> Option<RecordColumns> {
    let execution = tool_execution(entry)?;
    let target = tool_target(&execution.tool, &execution.args);
    let detail: Vec<String> = [
        Some(target).filter(|target| !target.is_empty()),
        Some(tool_outcome(&execution)),
        execution.duration_ms.map(format_duration_ms),
    ]
    .into_iter()
    .flatten()
    .collect();
    let detail = detail.join(" · ");
    Some(RecordColumns::new(
        execution.tool.as_str(),
        tool_group(&execution.tool),
        detail,
    ))
}

/// Columns for a policy decision: tool, tool group, verdict and decider.
fn decision_columns(tx: &Transaction) -> Option<RecordColumns> {
    let decision = serde_json::from_slice::<PolicyDecision>(&tx.payload).ok()?;
    Some(RecordColumns::new(
        decision.tool.as_str(),
        tool_group(&decision.tool),
        format!(
            "{} · {}",
            policy_verdict(decision.verdict),
            decision.decided_by
        ),
    ))
}

/// Columns for an action result: success, exit code and duration.
fn action_result_columns(tx: &Transaction) -> Option<RecordColumns> {
    let result = serde_json::from_slice::<ActionResultPayload>(&tx.payload).ok()?;
    Some(RecordColumns::new(
        if result.success { "ok" } else { "failed" },
        result
            .exit_code
            .map_or_else(String::new, |code| format!("exit {code}")),
        format_duration_ms(result.duration_ms),
    ))
}

/// Columns for a model call: stop reason and model family, or the class
/// of error when the call failed.
pub(super) fn reasoning_columns(payload: &Value) -> RecordColumns {
    if let Some(error) = reasoning_error(payload) {
        return RecordColumns::new("error", llm_error_class(error), error);
    }
    let stop = payload
        .get("stop_reason")
        .and_then(Value::as_str)
        .unwrap_or("");
    let family = payload
        .get("model")
        .and_then(Value::as_str)
        .map_or("", model_family);
    let took = payload
        .get("duration_ms")
        .and_then(Value::as_u64)
        .map(format_duration_ms);
    let detail: Vec<String> = [reasoning_tokens(payload), reasoning_tools(payload), took]
        .into_iter()
        .flatten()
        .collect();
    RecordColumns::new(stop, family, detail.join(" · "))
}

/// The error of a failed model call; `None` when the call succeeded. A
/// stream that broke says `status: failed`; a call that got no answer at
/// all (an error or timeout before any stream) says `stop_reason: Error`,
/// which no answer has.
pub(crate) fn reasoning_error(payload: &Value) -> Option<&str> {
    let text = |key: &str| payload.get(key).and_then(Value::as_str);
    (text("status") == Some("failed") || text("stop_reason") == Some("Error"))
        .then(|| text("error").unwrap_or(""))
}

/// Short class of a model call error, at most 7 characters.
pub(super) fn llm_error_class(error: &str) -> &'static str {
    let error = error.to_ascii_lowercase();
    let mentions = |needles: &[&str]| needles.iter().any(|needle| error.contains(needle));
    if mentions(&["429", "rate limit", "rate_limit"]) {
        "rate"
    } else if mentions(&["529", "overloaded"]) {
        "busy"
    } else if mentions(&["timeout", "timed out"]) {
        "timeout"
    } else if mentions(&[
        "401",
        "403",
        "unauthorized",
        "forbidden",
        "api key",
        "authentication",
    ]) {
        "auth"
    } else if mentions(&["stream"]) {
        "stream"
    } else {
        "other"
    }
}

/// Model family shown in the group column, e.g. `claude-sonnet-4-5` -> `sonnet`.
pub(super) fn model_family(model: &str) -> &str {
    const FAMILIES: &[&str] = &["opus", "sonnet", "haiku", "gpt", "gemini", "grok"];
    let lower = model.to_ascii_lowercase();
    if let Some(family) = FAMILIES.iter().find(|family| lower.contains(*family)) {
        return family;
    }
    let name = model.rsplit('/').next().unwrap_or(model);
    name.split(['-', '_', ':', '.']).next().unwrap_or(name)
}

/// Group a tool belongs to, at most 7 characters.
pub(super) fn tool_group(tool: &str) -> &'static str {
    match tool {
        "read_file" | "write_file" | "edit_file" | "delete_file" | "list_files" | "find_files"
        | "stat_file" => "file",
        "run_command" => "shell",
        "search_code" => "search",
        _ if tool.starts_with("git_") => "git",
        _ if tool.starts_with("web_") || tool.contains("http") || tool.contains("fetch") => "web",
        _ if tool.contains("spec") => "spec",
        _ if tool.contains("task") => "task",
        _ if tool.contains("agent") => "agent",
        _ => "other",
    }
}

pub(crate) fn format_local_time(ts_ms: u64) -> String {
    i64::try_from(ts_ms)
        .ok()
        .and_then(chrono::DateTime::from_timestamp_millis)
        .map_or_else(
            || "??:??:??".to_string(),
            |dt| {
                dt.with_timezone(&chrono::Local)
                    .format("%H:%M:%S")
                    .to_string()
            },
        )
}
