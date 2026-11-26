//! Chat transcripts rebuilt from model messages and record entries.

use super::*;

/// A chat transcript of `messages`: their text, and each tool call with
/// its result.
#[must_use]
pub fn chat_from_messages(messages: &[Message]) -> Vec<MessageData> {
    use aura_model::{ContentBlock, Role, ToolResultContent};

    let mut chat: Vec<MessageData> = Vec::new();
    let mut tool_calls: HashMap<String, usize> = HashMap::new();
    for message in messages {
        let role = match message.role {
            Role::User => MessageRole::User,
            Role::Assistant => MessageRole::Assistant,
        };
        for block in &message.content {
            match block {
                ContentBlock::Text { text } if !text.trim().is_empty() => {
                    chat.push(MessageData::new(role, text));
                }
                ContentBlock::ToolUse { id, name, input } => {
                    tool_calls.insert(id.clone(), chat.len());
                    chat.push(tool_use_message(id, name, input));
                }
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } => {
                    let Some(&index) = tool_calls.get(tool_use_id) else {
                        continue;
                    };
                    let text = match content {
                        ToolResultContent::Text(text) => text.clone(),
                        ToolResultContent::Json(value) => value.to_string(),
                    };
                    set_tool_result(&mut chat[index], text, *is_error);
                }
                _ => {}
            }
        }
    }
    let skip = chat.len().saturating_sub(HISTORY_MESSAGE_LIMIT);
    chat.into_iter().skip(skip).collect()
}

/// The chat message of a tool call, before its result is known.
fn tool_use_message(id: &str, name: &str, input: &Value) -> MessageData {
    MessageData {
        activity: Some(ActivityData::Tool {
            id: id.to_owned(),
            name: name.to_owned(),
            args: input.to_string(),
            result: None,
            is_error: false,
            duration_ms: None,
            policy: None,
        }),
        ..MessageData::new(MessageRole::Assistant, "")
    }
}

/// Put a tool result on the tool call message it answers.
fn set_tool_result(message: &mut MessageData, text: String, is_error: bool) {
    if let Some(ActivityData::Tool {
        result,
        is_error: failed,
        ..
    }) = &mut message.activity
    {
        *result = Some(text);
        *failed = is_error;
    }
}

/// Rebuild an agent's chat transcript from its record, oldest first.
///
/// # Errors
/// Returns the store error if the head sequence or scan fails.
pub fn conversation_history(
    store: &dyn ReadStore,
    agent_id: AgentId,
) -> Result<Vec<MessageData>, StoreError> {
    Ok(conversation_from(&scan_tail(
        store,
        agent_id,
        HISTORY_SCAN_LIMIT,
    )?))
}

/// [`conversation_history`] from `entries`, the newest of the record,
/// oldest first.
#[must_use]
pub fn conversation_from(entries: &[RecordEntry]) -> Vec<MessageData> {
    let mut history: Vec<MessageData> = Vec::new();
    // Tool use id -> (index in `history`, proposal time)
    let mut tool_calls: HashMap<String, (usize, u64)> = HashMap::new();
    // The models of the current turn, and where its replies are in `history`
    let mut calls = TurnCalls::default();
    let mut replies: Vec<usize> = Vec::new();
    for entry in entries {
        let tx = &entry.tx;
        track_turn(&mut calls, &mut replies, &mut history, tx);
        match tx.tx_type {
            TransactionType::UserPrompt => {
                history.extend(history_message(tx));
            }
            TransactionType::Reasoning => {
                history.extend(thinking_message(tx));
            }
            TransactionType::AgentMsg => {
                if let Some(mut message) = history_message(tx) {
                    set_footer(&mut message, calls.footer());
                    replies.push(history.len());
                    history.push(message);
                }
            }
            TransactionType::ToolProposal if !is_user_proposal(tx) => {
                push_proposal(&mut history, &mut tool_calls, tx);
            }
            TransactionType::PolicyDecision => apply_policy(&mut history, &tool_calls, tx),
            TransactionType::ToolExecution => apply_execution(&mut history, &tool_calls, entry),
            _ => history.extend(history_message(tx)),
        }
    }
    let skip = history.len().saturating_sub(HISTORY_MESSAGE_LIMIT);
    history.into_iter().skip(skip).collect()
}

/// Follow the turn `tx` belongs to, and refresh the footers of its replies
/// when it adds a model call.
fn track_turn(
    calls: &mut TurnCalls,
    replies: &mut Vec<usize>,
    history: &mut [MessageData],
    tx: &Transaction,
) {
    match calls.see(tx) {
        Seen::NewTurn => replies.clear(),
        Seen::Call => {
            let footer = calls.footer();
            for &at in replies.iter() {
                set_footer(&mut history[at], footer.clone());
            }
        }
        Seen::Other => {}
    }
}

/// Add a tool proposal to `history` as a tool call, or as plain text when
/// it does not decode.
fn push_proposal(
    history: &mut Vec<MessageData>,
    tool_calls: &mut HashMap<String, (usize, u64)>,
    tx: &Transaction,
) {
    match serde_json::from_slice::<ToolProposal>(&tx.payload) {
        Ok(proposal) => {
            tool_calls.insert(proposal.tool_use_id.clone(), (history.len(), tx.ts_ms));
            history.push(tool_call_message(&proposal, tx.ts_ms));
        }
        Err(_) => history.extend(history_message(tx)),
    }
}

/// Put a policy decision on its tool call, or add it as a message.
fn apply_policy(
    history: &mut Vec<MessageData>,
    tool_calls: &HashMap<String, (usize, u64)>,
    tx: &Transaction,
) {
    let decision = serde_json::from_slice::<PolicyDecision>(&tx.payload).ok();
    let call = decision
        .as_ref()
        .and_then(|decision| tool_calls.get(&decision.tool_use_id));
    match (call, decision.as_ref().and_then(policy_outcome)) {
        (Some(&(index, _)), Some(outcome)) => {
            set_tool_policy(&mut history[index], outcome);
        }
        _ => history.extend(history_message(tx)),
    }
}

/// Finish the tool call a tool execution answers.
fn apply_execution(
    history: &mut [MessageData],
    tool_calls: &HashMap<String, (usize, u64)>,
    entry: &RecordEntry,
) {
    let Some(execution) = tool_execution(entry) else {
        return;
    };
    if let Some(&(index, proposed_at)) = tool_calls.get(&execution.tool_use_id) {
        finish_tool_call(
            &mut history[index],
            &execution,
            entry.tx.ts_ms.saturating_sub(proposed_at),
        );
    }
}

/// Put the footer of a reply's turn, its models and their usage, on `reply`.
pub(super) fn set_footer(reply: &mut MessageData, footer: Option<crate::turn_calls::Footer>) {
    match footer {
        Some(footer) => {
            reply.model = Some(footer.models);
            reply.usage = footer.usage;
            reply.sources = footer.sources;
        }
        None => {
            reply.model = None;
            reply.usage = None;
            reply.sources = Vec::new();
        }
    }
}

/// The model that answered the call a `Reasoning` entry records.
pub(super) fn reasoning_model(payload: &Value) -> Option<String> {
    recorded_model(payload, "model")
}

/// The model id named under `key` in a `Reasoning` payload, unless it is
/// empty or `unknown`.
pub(super) fn recorded_model(payload: &Value, key: &str) -> Option<String> {
    payload
        .get(key)
        .and_then(Value::as_str)
        .filter(|model| !model.is_empty() && *model != "unknown")
        .map(str::to_string)
}

pub(super) fn call_tokens(payload: &Value) -> Usage {
    let tokens = |key: &str| payload.get(key).and_then(Value::as_u64);
    Usage {
        input_tokens: tokens("input_tokens").unwrap_or(0),
        output_tokens: tokens("output_tokens").unwrap_or(0),
        cache_creation_input_tokens: tokens("cache_creation_input_tokens"),
        cache_read_input_tokens: tokens("cache_read_input_tokens"),
    }
}

/// The call `tx` records, when it is a model call that got an answer:
/// the model that answered, the id it was asked for (when the record
/// kept it), what it was made for and its tokens.
pub(crate) fn model_call(tx: &Transaction) -> Option<ModelCall> {
    if tx.tx_type != TransactionType::Reasoning {
        return None;
    }
    let payload = json_payload(tx)?;
    Some(ModelCall {
        model: reasoning_model(&payload)?,
        route: recorded_model(&payload, "route"),
        purpose: recorded_purpose(&payload),
        usage: call_tokens(&payload),
    })
}

/// What the model call `tx` records was made for, when it was made
/// beside the agent's own work.
pub(crate) fn call_purpose(tx: &Transaction) -> Option<ModelCallPurpose> {
    if tx.tx_type != TransactionType::Reasoning {
        return None;
    }
    recorded_purpose(&json_payload(tx)?)
}

/// The purpose a `Reasoning` payload names, unless it names none this
/// build knows.
pub(super) fn recorded_purpose(payload: &Value) -> Option<ModelCallPurpose> {
    payload
        .get("purpose")
        .cloned()
        .and_then(|purpose| serde_json::from_value(purpose).ok())
}

/// The plan usage the call `tx` records, when it is a model call whose
/// provider reported any; a payload that does not read is none.
pub(crate) fn plan_usage(tx: &Transaction) -> Vec<PlanUsage> {
    if tx.tx_type != TransactionType::Reasoning {
        return Vec::new();
    }
    json_payload(tx)
        .and_then(|payload| payload.get("plan_usage").cloned())
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default()
}

/// The model's thinking from a `Reasoning` entry, when it thought.
pub(super) fn thinking_message(tx: &Transaction) -> Option<MessageData> {
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
pub(super) fn tool_call_message(proposal: &ToolProposal, ts_ms: u64) -> MessageData {
    MessageData {
        timestamp_ms: Some(ts_ms),
        activity: Some(ActivityData::Tool {
            id: proposal.tool_use_id.clone(),
            name: proposal.tool.clone(),
            args: proposal.args.to_string(),
            result: None,
            is_error: false,
            duration_ms: None,
            policy: None,
        }),
        ..MessageData::new(MessageRole::Assistant, "")
    }
}

/// Record the policy decision on the tool call `message` shows.
pub(super) fn set_tool_policy(message: &mut MessageData, decision: String) {
    if let Some(ActivityData::Tool { policy, .. }) = &mut message.activity {
        *policy = Some(decision);
    }
}

/// Fill in a tool call's outcome from its execution entry.
pub(super) fn finish_tool_call(message: &mut MessageData, execution: &ToolExecution, took_ms: u64) {
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
    *duration_ms = Some(execution.duration_ms.unwrap_or(took_ms));
}

/// The tool execution in `entry`, with its output read from the entry's
/// effects (the kernel no longer copies it into the payload).
pub(crate) fn tool_execution(entry: &RecordEntry) -> Option<ToolExecution> {
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
