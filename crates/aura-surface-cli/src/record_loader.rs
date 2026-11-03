//! Loading and displaying historical records from the store.

use crate::agent_registry::AgentRegistry;
use crate::event_loop::agent_events::call_usage;
use crate::event_loop::run_command::is_user_tool_use;
use aura_agent_kernel::decode_tool_effect;
use aura_core_types::{
    ActionResultPayload, AgentId, EffectStatus, PolicyApprover, PolicyCheckOutcome, PolicyDecision,
    RecordEntry, ToolExecution, ToolGateVerdict, ToolProposal, Transaction, TransactionType,
};
use aura_model_reasoner::{Message, Usage};
use aura_store_db::{ReadStore, StoreError};
use aura_terminal::{
    events::{
        ActivityData, AgentSummary, CallUsage, MessageData, MessageRole, PolicyDetail,
        RecordStatus, RecordSummary,
    },
    UiCommand,
};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use tokio::sync::mpsc;
use tracing::warn;

/// Number of entries shown in the Record panel.
const RECORD_PANEL_LIMIT: usize = 100;

/// Number of trailing entries scanned when rebuilding an agent's chat.
const HISTORY_SCAN_LIMIT: usize = 1000;

/// Number of chat messages kept after rebuilding, activity blocks included
/// (matches the UI cap).
const HISTORY_MESSAGE_LIMIT: usize = 300;

/// Upper bound on the summary string; the panel truncates to its own
/// width, this only keeps huge prompts from being copied around.
const MAX_INFO_CHARS: usize = 240;

/// Load the most recent record entries for an agent as UI summaries,
/// oldest first.
///
/// # Errors
/// Returns the store error if the head sequence or scan fails.
pub fn recent_record_summaries(
    store: &dyn ReadStore,
    agent_id: AgentId,
) -> Result<Vec<RecordSummary>, StoreError> {
    Ok(scan_tail(store, agent_id, RECORD_PANEL_LIMIT)?
        .iter()
        .map(|entry| {
            build_record_summary(
                entry.seq,
                &entry.tx,
                entry,
                format_local_time(entry.tx.ts_ms),
            )
        })
        .collect())
}

/// Load existing records from the store and send to UI.
///
/// Uses `try_send`, so it is only suitable before the UI starts draining
/// the channel (startup); async callers should send summaries themselves.
pub fn load_existing_records(
    store: &dyn ReadStore,
    agent_id: AgentId,
    commands: &mpsc::Sender<UiCommand>,
) {
    match recent_record_summaries(store, agent_id) {
        Ok(summaries) => {
            for summary in summaries {
                let _ = commands.try_send(UiCommand::NewRecord(summary));
            }
        }
        Err(e) => {
            warn!(error = %e, "Failed to load records");
            let _ = commands.try_send(UiCommand::ShowWarning(format!(
                "Could not load record history: {e}"
            )));
        }
    }
}

/// Rebuild the conversation the model sees from an agent's record: its
/// prompts and replies since the last session start, oldest first. Tool
/// calls are left out; their `tool_use`/`tool_result` pairs can't be
/// replayed safely, and the replies already describe what they did.
///
/// # Errors
/// Returns the store error if the head sequence or scan fails.
pub fn model_history(store: &dyn ReadStore, agent_id: AgentId) -> Result<Vec<Message>, StoreError> {
    let mut messages = Vec::new();
    for entry in scan_tail(store, agent_id, HISTORY_SCAN_LIMIT)? {
        let tx = &entry.tx;
        let text = || String::from_utf8_lossy(&tx.payload).trim().to_string();
        match tx.tx_type {
            TransactionType::SessionStart => messages.clear(),
            TransactionType::UserPrompt => {
                let text = text();
                if !text.is_empty() {
                    messages.push(Message::user(text));
                }
            }
            TransactionType::AgentMsg => {
                let text = text();
                if !text.is_empty() {
                    messages.push(Message::assistant(text));
                }
            }
            _ => {}
        }
    }
    Ok(messages)
}

/// Rebuild an agent's chat transcript from its record, oldest first.
///
/// # Errors
/// Returns the store error if the head sequence or scan fails.
pub fn conversation_history(
    store: &dyn ReadStore,
    agent_id: AgentId,
) -> Result<Vec<MessageData>, StoreError> {
    let mut history: Vec<MessageData> = Vec::new();
    // Tool use id -> (index in `history`, proposal time)
    let mut tool_calls: HashMap<String, (usize, u64)> = HashMap::new();
    // Model, tokens and cost of the latest model call in the current turn
    let mut reply_model: Option<String> = None;
    let mut reply_usage: Option<CallUsage> = None;
    for entry in scan_tail(store, agent_id, HISTORY_SCAN_LIMIT)? {
        let tx = &entry.tx;
        match tx.tx_type {
            TransactionType::UserPrompt => {
                reply_model = None;
                reply_usage = None;
                history.extend(history_message(tx));
            }
            TransactionType::Reasoning => {
                if let Some(model) = reasoning_model(tx) {
                    reply_usage = reasoning_usage(tx, &model);
                    reply_model = Some(model);
                }
                history.extend(thinking_message(tx));
            }
            TransactionType::AgentMsg => {
                history.extend(history_message(tx).map(|message| MessageData {
                    model: reply_model.clone(),
                    usage: reply_usage,
                    ..message
                }));
            }
            TransactionType::ToolProposal if !is_user_proposal(tx) => {
                match serde_json::from_slice::<ToolProposal>(&tx.payload) {
                    Ok(proposal) => {
                        tool_calls.insert(proposal.tool_use_id.clone(), (history.len(), tx.ts_ms));
                        history.push(tool_call_message(&proposal, tx.ts_ms));
                    }
                    Err(_) => history.extend(history_message(tx)),
                }
            }
            TransactionType::ToolExecution => {
                let Some(execution) = tool_execution(&entry) else {
                    continue;
                };
                if let Some(&(index, proposed_at)) = tool_calls.get(&execution.tool_use_id) {
                    finish_tool_call(
                        &mut history[index],
                        &execution,
                        tx.ts_ms.saturating_sub(proposed_at),
                    );
                }
            }
            _ => history.extend(history_message(tx)),
        }
    }
    let skip = history.len().saturating_sub(HISTORY_MESSAGE_LIMIT);
    Ok(history.into_iter().skip(skip).collect())
}

/// The model that answered the call a `Reasoning` entry records.
fn reasoning_model(tx: &Transaction) -> Option<String> {
    json_payload(tx)?
        .get("model")
        .and_then(Value::as_str)
        .filter(|model| !model.is_empty() && *model != "unknown")
        .map(str::to_string)
}

/// Tokens and cost of the call a `Reasoning` entry records, answered by `model`.
fn reasoning_usage(tx: &Transaction, model: &str) -> Option<CallUsage> {
    let payload = json_payload(tx)?;
    let tokens = |key: &str| payload.get(key).and_then(Value::as_u64);
    let usage = Usage {
        input_tokens: tokens("input_tokens").unwrap_or(0),
        output_tokens: tokens("output_tokens").unwrap_or(0),
        cache_creation_input_tokens: tokens("cache_creation_input_tokens"),
        cache_read_input_tokens: tokens("cache_read_input_tokens"),
    };
    call_usage(model, &usage)
}

/// The model's thinking from a `Reasoning` entry, when it thought.
fn thinking_message(tx: &Transaction) -> Option<MessageData> {
    let payload = json_payload(tx)?;
    let text = payload.get("thinking").and_then(Value::as_str)?;
    if text.trim().is_empty() {
        return None;
    }
    Some(MessageData {
        timestamp_ms: Some(tx.ts_ms),
        activity: Some(ActivityData::Thinking {
            text: text.to_string(),
            duration_ms: payload.get("thinking_ms").and_then(Value::as_u64),
        }),
        ..MessageData::new(MessageRole::Assistant, "")
    })
}

/// A tool call the model proposed, without its outcome yet.
fn tool_call_message(proposal: &ToolProposal, ts_ms: u64) -> MessageData {
    MessageData {
        timestamp_ms: Some(ts_ms),
        activity: Some(ActivityData::Tool {
            id: proposal.tool_use_id.clone(),
            name: proposal.tool.clone(),
            args: proposal.args.to_string(),
            result: None,
            is_error: false,
            duration_ms: None,
        }),
        ..MessageData::new(MessageRole::Assistant, "")
    }
}

/// Fill in a tool call's outcome from its execution entry.
fn finish_tool_call(message: &mut MessageData, execution: &ToolExecution, took_ms: u64) {
    let Some(ActivityData::Tool {
        result,
        is_error,
        duration_ms,
        ..
    }) = &mut message.activity
    else {
        return;
    };
    *is_error = execution.is_error || execution.decision != ToolGateVerdict::Approved;
    *result = match execution.decision {
        ToolGateVerdict::Approved => execution.result.clone(),
        ToolGateVerdict::Denied => Some(execution.reason.as_deref().map_or_else(
            || "denied".to_string(),
            |reason| format!("denied: {reason}"),
        )),
        ToolGateVerdict::PendingApproval => Some("awaiting approval".to_string()),
    };
    *duration_ms = Some(took_ms);
}

/// The tool execution in `entry`, with its output read from the entry's
/// effects (the kernel no longer copies it into the payload).
fn tool_execution(entry: &RecordEntry) -> Option<ToolExecution> {
    let mut execution = serde_json::from_slice::<ToolExecution>(&entry.tx.payload).ok()?;
    if execution.result.is_none() {
        if let Some(effect) = entry.effects.first() {
            let decoded = decode_tool_effect(effect);
            execution.is_error |= decoded.is_error;
            execution.result = Some(decoded.content);
        }
    }
    Some(execution)
}

/// Build the agents-column rows for every registered agent, each followed
/// by the agents it spawned. `running` holds the agents currently working
/// on a task for their parent.
#[must_use]
pub fn agent_summaries(
    registry: &AgentRegistry,
    store: &dyn ReadStore,
    active: AgentId,
    running: &HashSet<AgentId>,
) -> Vec<AgentSummary> {
    registry
        .tree()
        .into_iter()
        .map(|(agent, depth)| {
            let agent_id = agent.agent_id();
            let record_count = store.get_head_seq(agent_id).unwrap_or(0);
            let last_active_ms = (record_count > 0)
                .then(|| store.get_record_entry(agent_id, record_count).ok())
                .flatten()
                .map(|entry| entry.tx.ts_ms);
            AgentSummary {
                id: agent_id.to_hex(),
                name: agent.name.clone(),
                zns_id: agent.zns_id.clone(),
                is_active: agent_id == active,
                record_count,
                last_active: last_active_ms.map_or_else(|| "-".to_string(), format_local_time),
                last_active_ms,
                definition: agent.definition.clone(),
                depth,
                running: running.contains(&agent_id),
                project: agent.project.clone(),
            }
        })
        .collect()
}

fn scan_tail(
    store: &dyn ReadStore,
    agent_id: AgentId,
    limit: usize,
) -> Result<Vec<RecordEntry>, StoreError> {
    let head_seq = store.get_head_seq(agent_id)?;
    if head_seq == 0 {
        return Ok(Vec::new());
    }
    let window = u64::try_from(limit).unwrap_or(u64::MAX);
    let from_seq = head_seq.saturating_sub(window.saturating_sub(1)).max(1);
    store.scan_record(agent_id, from_seq, limit)
}

fn history_message(tx: &Transaction) -> Option<MessageData> {
    if is_user_proposal(tx) {
        let command = extract_tool_info(tx);
        return (!command.is_empty()).then(|| MessageData {
            role: MessageRole::User,
            content: command.clone(),
            is_streaming: false,
            timestamp_ms: Some(tx.ts_ms),
            command: Some(command),
            activity: None,
            model: None,
            usage: None,
        });
    }
    let (role, content) = match tx.tx_type {
        TransactionType::UserPrompt => (
            MessageRole::User,
            String::from_utf8_lossy(&tx.payload).into_owned(),
        ),
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
    })
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

/// One-line chat rendering of a policy decision, e.g.
/// `policy: read_file allowed by user_default.full_access`.
///
/// `None` when the user's own command was allowed: its output already
/// shows that it ran.
#[must_use]
pub fn policy_chat_line(decision: &PolicyDecision) -> Option<String> {
    let outcome = match decision.verdict {
        ToolGateVerdict::Approved if is_user_tool_use(&decision.tool_use_id) => return None,
        ToolGateVerdict::Approved => "allowed",
        ToolGateVerdict::Denied => "denied",
        ToolGateVerdict::PendingApproval => "needs approval",
    };
    Some(format!(
        "policy: {} {outcome} by {}",
        decision.tool, decision.decided_by
    ))
}

/// Policy provenance for the record detail view; `None` for entries that
/// are not policy decisions.
fn policy_detail(tx: &Transaction) -> Option<PolicyDetail> {
    if tx.tx_type != TransactionType::PolicyDecision {
        return None;
    }
    let decision = serde_json::from_slice::<PolicyDecision>(&tx.payload).ok()?;
    let approval = decision
        .approval
        .as_ref()
        .map_or_else(String::new, |approval| {
            let approver = match &approval.approver {
                PolicyApprover::Hook => "hook".to_string(),
                PolicyApprover::User { user_id: Some(id) } => format!("user {id}"),
                PolicyApprover::User { user_id: None } => "user".to_string(),
            };
            match approval.remember.as_deref() {
                Some(remember) => format!("{approver} ({remember})"),
                None => approver,
            }
        });
    Some(PolicyDetail {
        tool: decision.tool.clone(),
        verdict: verdict_label(decision.verdict).to_string(),
        decided_by: decision.decided_by.to_string(),
        reason: decision.reason.clone().unwrap_or_default(),
        trace: decision
            .trace
            .iter()
            .map(|check| {
                let outcome = match check.outcome {
                    PolicyCheckOutcome::Allow => "allow",
                    PolicyCheckOutcome::Deny => "deny ",
                    PolicyCheckOutcome::Ask => "ask  ",
                };
                format!("{outcome} {}", check.rule)
            })
            .collect(),
        approval,
        mode: decision.mode,
        policy_hash: decision.policy_hash,
        proposal_tx: tx
            .reference_tx_hash
            .map(|hash| hash.to_hex())
            .unwrap_or_default(),
    })
}

const fn verdict_label(verdict: ToolGateVerdict) -> &'static str {
    match verdict {
        ToolGateVerdict::Approved => "approved",
        ToolGateVerdict::Denied => "denied",
        ToolGateVerdict::PendingApproval => "pending_approval",
    }
}

/// Like [`summarize_record`], but tool executions read their output from
/// the entry's effects (the kernel no longer copies it into the payload).
fn summarize_entry(tx: &Transaction, entry: &RecordEntry) -> String {
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
struct RecordColumns {
    /// What the entry is about: tool name, LLM stop reason, sender.
    what: String,
    /// Category of `what`: tool group, LLM error class, model family.
    group: String,
    /// The rest of the summary, without what the columns already show.
    detail: String,
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
fn record_columns(tx: &Transaction, entry: &RecordEntry, sender: &str) -> Option<RecordColumns> {
    let columns = match tx.tx_type {
        TransactionType::UserPrompt | TransactionType::AgentMsg | TransactionType::Trigger => {
            RecordColumns::new(sender, "text", payload_text(tx))
        }
        TransactionType::ToolProposal => {
            let proposal = serde_json::from_slice::<ToolProposal>(&tx.payload).ok()?;
            RecordColumns::new(
                proposal.tool.as_str(),
                tool_group(&proposal.tool),
                tool_target(&proposal.tool, &proposal.args),
            )
        }
        TransactionType::ToolExecution => {
            let execution = tool_execution(entry)?;
            let target = tool_target(&execution.tool, &execution.args);
            let outcome = tool_outcome(&execution);
            let detail = if target.is_empty() {
                outcome
            } else {
                format!("{target} · {outcome}")
            };
            RecordColumns::new(execution.tool.as_str(), tool_group(&execution.tool), detail)
        }
        TransactionType::PolicyDecision => {
            let decision = serde_json::from_slice::<PolicyDecision>(&tx.payload).ok()?;
            RecordColumns::new(
                decision.tool.as_str(),
                tool_group(&decision.tool),
                format!(
                    "{} · {}",
                    policy_verdict(decision.verdict),
                    decision.decided_by
                ),
            )
        }
        TransactionType::ActionResult | TransactionType::ProcessComplete => {
            let result = serde_json::from_slice::<ActionResultPayload>(&tx.payload).ok()?;
            RecordColumns::new(
                if result.success { "ok" } else { "failed" },
                result
                    .exit_code
                    .map_or_else(String::new, |code| format!("exit {code}")),
                format_duration_ms(result.duration_ms),
            )
        }
        TransactionType::Reasoning => reasoning_columns(&json_payload(tx)?),
        TransactionType::System => {
            let payload = json_payload(tx)?;
            RecordColumns::new(
                system_kind(&payload),
                "",
                system_detail(&payload).join(" "),
            )
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

/// Columns for a model call: stop reason and model family, or the class
/// of error when the call failed.
fn reasoning_columns(payload: &Value) -> RecordColumns {
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
    let detail: Vec<String> = [reasoning_tokens(payload), reasoning_tools(payload)]
        .into_iter()
        .flatten()
        .collect();
    RecordColumns::new(stop, family, detail.join(" · "))
}

/// The error of a failed model call; `None` when the call succeeded.
fn reasoning_error(payload: &Value) -> Option<&str> {
    (payload.get("status").and_then(Value::as_str) == Some("failed"))
        .then(|| payload.get("error").and_then(Value::as_str).unwrap_or(""))
}

/// Short class of a model call error, at most 7 characters.
fn llm_error_class(error: &str) -> &'static str {
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
fn model_family(model: &str) -> &str {
    const FAMILIES: &[&str] = &["opus", "sonnet", "haiku", "gpt", "gemini", "grok"];
    let lower = model.to_ascii_lowercase();
    if let Some(family) = FAMILIES.iter().find(|family| lower.contains(*family)) {
        return *family;
    }
    let name = model.rsplit('/').next().unwrap_or(model);
    name.split(['-', '_', ':', '.']).next().unwrap_or(name)
}

/// Group a tool belongs to, at most 7 characters.
fn tool_group(tool: &str) -> &'static str {
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
        .map(|dt| {
            dt.with_timezone(&chrono::Local)
                .format("%H:%M:%S")
                .to_string()
        })
        .unwrap_or_else(|| "??:??:??".to_string())
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
        TransactionType::PolicyDecision => ("Policy", "KERNEL"),
        TransactionType::ToolExecution => ("ToolRes", "KERNEL"),
        TransactionType::ProcessComplete => ("Complete", "SYSTEM"),
        TransactionType::Reasoning => ("LLM", "KERNEL"),
        TransactionType::SubagentSpawn => ("Spawn", "AGENT"),
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

fn summarize_tool_call(tool: &str, args: &Value) -> String {
    let target = tool_target(tool, args);
    if target.is_empty() {
        tool.to_string()
    } else {
        format!("{tool} {target}")
    }
}

/// What a tool call acts on: the command line for `run_command`, else
/// its primary argument.
fn tool_target(tool: &str, args: &Value) -> String {
    if tool == "run_command" {
        extract_cmd_run_command(args)
    } else {
        primary_arg(args).unwrap_or_default()
    }
}

fn summarize_tool_execution(execution: &ToolExecution) -> String {
    format!(
        "{} · {}",
        summarize_tool_call(&execution.tool, &execution.args),
        tool_outcome(execution)
    )
}

/// How a tool call ended, e.g. `ok 5B` or `denied: reason`.
fn tool_outcome(execution: &ToolExecution) -> String {
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

fn summarize_policy_decision(decision: &PolicyDecision) -> String {
    format!(
        "{} · {} · {}",
        decision.tool,
        policy_verdict(decision.verdict),
        decision.decided_by
    )
}

const fn policy_verdict(verdict: ToolGateVerdict) -> &'static str {
    match verdict {
        ToolGateVerdict::Approved => "allow",
        ToolGateVerdict::Denied => "deny",
        ToolGateVerdict::PendingApproval => "ask",
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
fn reasoning_tokens(payload: &Value) -> Option<String> {
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
fn reasoning_tools(payload: &Value) -> Option<String> {
    let tools: Vec<&str> = payload
        .get("tool_uses")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    (!tools.is_empty()).then(|| format!("tools: {}", tools.join(", ")))
}

fn summarize_system(payload: &Value) -> String {
    let kind = system_kind(payload);
    let detail = system_detail(payload);
    if detail.is_empty() {
        kind.to_string()
    } else {
        format!("{kind} · {}", detail.join(" "))
    }
}

fn system_kind(payload: &Value) -> &str {
    payload
        .get("system_kind")
        .and_then(Value::as_str)
        .unwrap_or("system")
}

fn system_detail(payload: &Value) -> Vec<&str> {
    ["method", "event", "action", "scope", "phase"]
        .iter()
        .filter_map(|key| payload.get(*key).and_then(Value::as_str))
        .collect()
}

fn summarize_spawn(payload: &Value) -> String {
    let mut parts = Vec::new();
    if let Some(mode) = payload.get("spawn_mode").and_then(Value::as_str) {
        parts.push(mode.to_string());
    }
    parts.extend(spawn_detail(payload));
    parts.join(" · ")
}

/// Child agent and override count of a spawn, without the spawn mode.
fn spawn_detail(payload: &Value) -> Vec<String> {
    let mut parts = Vec::new();
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

/// Whether `tx` is a tool proposal the user made directly.
fn is_user_proposal(tx: &Transaction) -> bool {
    tx.tx_type == TransactionType::ToolProposal
        && serde_json::from_slice::<ToolProposal>(&tx.payload)
            .is_ok_and(|p| p.source.as_deref() == Some(crate::event_loop::run_command::USER_SOURCE))
}

/// Extract the command string from cmd_run tool arguments.
fn extract_cmd_run_command(args: &Value) -> String {
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
mod tests;
