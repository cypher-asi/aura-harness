//! Translation layer between `aura_agent::AgentLoopEvent` and the
//! crate-local [`AutomatonEvent`]. Lives in its own file so the
//! large match arm doesn't clutter the `dev_loop` dispatch root.

use crate::events::AutomatonEvent;

pub fn forward_agent_event(
    tx: &tokio::sync::mpsc::Sender<AutomatonEvent>,
    evt: aura_agent::AgentLoopEvent,
    task_id: Option<&str>,
) {
    use aura_agent::AgentLoopEvent;
    let task_id_value = || task_id.map(str::to_owned);
    let automaton_event = match evt {
        AgentLoopEvent::TextDelta(text) => AutomatonEvent::TextDelta {
            task_id: task_id_value(),
            text,
        },
        AgentLoopEvent::ThinkingDelta(thinking) => AutomatonEvent::ThinkingDelta {
            task_id: task_id_value(),
            thinking,
        },
        AgentLoopEvent::ToolStart { id, name } => AutomatonEvent::ToolCallStarted {
            task_id: task_id_value(),
            id,
            name,
        },
        AgentLoopEvent::ToolInputSnapshot { id, name, input } => {
            // Partial JSON is expected while a `tool_use` block is
            // still streaming -- forward it with
            // `snapshot_partial: true` so the UI can render an
            // "in flight…" card instead of dropping the event
            // entirely and leaving the card empty. When the JSON
            // parses cleanly we still emit `snapshot_partial: false`
            // so downstream consumers that only care about finished
            // blocks can filter.
            match serde_json::from_str::<serde_json::Value>(&input) {
                Ok(parsed) => AutomatonEvent::ToolCallSnapshot {
                    task_id: task_id_value(),
                    id,
                    name,
                    input: parsed,
                    snapshot_partial: false,
                },
                Err(_) => AutomatonEvent::ToolCallSnapshot {
                    task_id: task_id_value(),
                    id,
                    name,
                    input: serde_json::Value::String(input),
                    snapshot_partial: true,
                },
            }
        }
        AgentLoopEvent::ToolResult {
            tool_use_id,
            tool_name,
            content,
            is_error,
        } => AutomatonEvent::ToolResult {
            task_id: task_id_value(),
            id: tool_use_id,
            name: tool_name,
            result: content,
            is_error,
        },
        // 1:1 projection of the harness's authoritative completion
        // frame. The server's DoD gate in
        // `apps/aura-os-server/src/handlers/dev_loop.rs`
        // (`successful_write_event_path`) counts
        // `tool_call_completed` events with `is_error=false` as file
        // change evidence when populating
        // `CachedTaskOutput::files_changed`. Without this mapping the
        // gate rejects every pure-edit task with "files 0".
        AgentLoopEvent::ToolCallCompleted {
            tool_use_id,
            tool_name,
            input,
            is_error,
        } => AutomatonEvent::ToolCallCompleted {
            task_id: task_id_value(),
            id: tool_use_id,
            name: tool_name,
            input,
            is_error,
        },
        AgentLoopEvent::IterationComplete {
            input_tokens,
            output_tokens,
            ..
        } => AutomatonEvent::TokenUsage {
            task_id: task_id_value(),
            input_tokens,
            output_tokens,
        },
        AgentLoopEvent::Warning(msg) => AutomatonEvent::LogLine { message: msg },
        AgentLoopEvent::Error { message, .. } => AutomatonEvent::Error {
            automaton_id: String::new(),
            message,
        },
        // Per-tool-call streaming retry lifecycle carries the active
        // task id when this forwarder is used by task-run/dev-loop.
        AgentLoopEvent::ToolCallRetrying {
            tool_use_id,
            tool_name,
            attempt,
            max_attempts,
            delay_ms,
            reason,
        } => AutomatonEvent::ToolCallRetrying {
            task_id: task_id.unwrap_or_default().to_string(),
            tool_use_id,
            tool_name,
            attempt,
            max_attempts,
            delay_ms,
            reason,
        },
        AgentLoopEvent::ToolCallFailed {
            tool_use_id,
            tool_name,
            reason,
        } => AutomatonEvent::ToolCallFailed {
            task_id: task_id.unwrap_or_default().to_string(),
            tool_use_id,
            tool_name,
            reason,
        },
        // `debug.*` observability frames pass through verbatim; the
        // `From<DebugEvent>` impl preserves the exact JSON shape the
        // aura-os forwarder routes on (`type: "debug.<kind>"`).
        AgentLoopEvent::Debug(ev) => AutomatonEvent::from(ev),
        _ => return,
    };
    if let Err(e) = tx.try_send(automaton_event) {
        tracing::warn!("automaton event channel full or closed: {e}");
    }
}
