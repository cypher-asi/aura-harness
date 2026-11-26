//! Node construction and label helpers.

use super::*;

/// A node for `entry` labelled `label`, cut to one line.
pub(super) fn new_node(entry: &RecordEntry, kind: FlowKind, label: &str) -> FlowNode {
    let agent = entry.tx.agent_id.to_hex();
    let label = short(label, MAX_LABEL_CHARS);
    FlowNode {
        id: format!(
            "{}:{}",
            &agent[..NODE_ID_AGENT_CHARS.min(agent.len())],
            entry.seq
        ),
        agent_id: agent,
        kind,
        label: if label.is_empty() {
            tx_type_label(entry.tx.tx_type).0.to_lowercase()
        } else {
            label
        },
        seq: entry.seq,
        at_ms: entry.tx.ts_ms,
        meta: vec![("tx".to_string(), entry.tx.hash.to_hex())],
        ..FlowNode::default()
    }
}

pub(super) fn set_meta(node: &mut FlowNode, key: &str, value: String) {
    match node.meta.iter_mut().find(|(k, _)| k == key) {
        Some(slot) => slot.1 = value,
        None => node.meta.push((key.to_string(), value)),
    }
}

pub(super) fn set_call_meta(node: &mut FlowNode, tool: &str, args: &Value) {
    set_meta(node, "tool", tool.to_string());
    if !args.is_null() {
        let args = serde_json::to_string_pretty(args).unwrap_or_else(|_| args.to_string());
        set_meta(node, "args", capped(&args));
    }
}

/// Who answered a live approval, and for how long it holds: `user · session`.
pub(super) fn approval_label(approval: &PolicyApproval) -> String {
    let who = match &approval.approver {
        PolicyApprover::Hook => "hook".to_string(),
        PolicyApprover::User { user_id: Some(id) } => format!("user {id}"),
        PolicyApprover::User { user_id: None } => "user".to_string(),
    };
    match &approval.remember {
        Some(remember) => format!("{who} · {remember}"),
        None => who,
    }
}

pub(super) fn spawn_label(name: &str) -> String {
    short(&format!("spawn {name}"), MAX_LABEL_CHARS)
}

/// What a tool call does, on one line. A `task` names the agent it asks
/// and what for.
pub(super) fn call_label(tool: &str, args: &Value) -> String {
    if tool != TASK_TOOL_NAME {
        return summarize_tool_call(tool, args);
    }
    let kind = arg(args, "subagent_type");
    let prompt = arg(args, "prompt");
    match (kind.is_empty(), prompt.is_empty()) {
        (true, true) => tool.to_string(),
        (false, true) => format!("{tool} {kind}"),
        (true, false) => format!("{tool} · {prompt}"),
        (false, false) => format!("{tool} {kind} · {prompt}"),
    }
}

pub(super) const fn execution_status(execution: &ToolExecution) -> TraceNodeStatus {
    match execution.decision {
        ToolGateVerdict::Approved if !execution.is_error => TraceNodeStatus::Ok,
        ToolGateVerdict::Approved | ToolGateVerdict::Denied => TraceNodeStatus::Error,
        ToolGateVerdict::PendingApproval => TraceNodeStatus::Warn,
    }
}

/// How long a call that worked took, else how it ended.
pub(super) fn execution_detail(execution: &ToolExecution, status: TraceNodeStatus) -> String {
    match execution.duration_ms {
        Some(ms) if status == TraceNodeStatus::Ok => format_duration_ms(ms),
        _ => short(&tool_outcome(execution), MAX_DETAIL_CHARS),
    }
}

/// The string argument `key` of a call or payload; empty when it has none.
pub(super) fn arg<'a>(args: &'a Value, key: &str) -> &'a str {
    args.get(key).and_then(Value::as_str).unwrap_or("")
}

/// `text` on one line of at most `max` characters.
pub(super) fn short(text: &str, max: usize) -> String {
    truncate_chars(&collapse_whitespace(text), max)
}

/// `text` as kept in a node's metadata.
pub(super) fn capped(text: &str) -> String {
    truncate_chars(text.trim(), MAX_META_CHARS)
}
