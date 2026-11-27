//! Seeing every push an agent makes, wherever it runs.
//!
//! A push tool's result carries the merge queue marker
//! ([`QueuedPush::from_tool_output`]), and every tool result of every
//! agent, local or cloud, reaches the UI through
//! [`crate::event_loop::agent_events::AgentUi`], which hands each one to
//! [`record`]. So no push from aura skips the queue: the marker is put in
//! by the push tools themselves, and read at the one place every result
//! passes. Like the chat notes, the sink is process-wide: it is set once,
//! when the factory starts.

use aura_core_types::AgentId;
use aura_exec_merge_queue::QueuedPush;
use aura_ui::UiCommand;
use std::sync::OnceLock;
use tokio::sync::mpsc;

/// A push an agent made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ObservedPush {
    /// The agent that pushed
    pub agent: AgentId,
    /// What it pushed
    pub push: QueuedPush,
}

static PUSHES: OnceLock<mpsc::UnboundedSender<ObservedPush>> = OnceLock::new();

/// Send every push seen from now on to `sink`. Only the first call takes.
pub(crate) fn install(sink: mpsc::UnboundedSender<ObservedPush>) -> bool {
    PUSHES.set(sink).is_ok()
}

/// The push in `command`, an agent's chat command, if it is a successful
/// tool result with a push marker.
pub(crate) fn push_in(command: &UiCommand) -> Option<QueuedPush> {
    match command {
        UiCommand::CompleteTool {
            result,
            success: true,
            ..
        } => QueuedPush::from_tool_output(result),
        _ => None,
    }
}

/// Hand the push in `command` of `agent`'s chat, if any, to the factory.
pub(crate) fn record(agent: AgentId, command: &UiCommand) {
    let Some(push) = push_in(command) else {
        return;
    };
    report(ObservedPush { agent, push });
}

/// Hand `push` to the factory; dropped (logged) before it started.
pub(crate) fn report(push: ObservedPush) {
    match PUSHES.get() {
        Some(sink) => {
            let _ = sink.send(push);
        }
        None => {
            tracing::warn!(agent = %push.agent, branch = %push.push.branch, "A push was made before the merge queue started");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(text: &str, success: bool) -> UiCommand {
        UiCommand::CompleteTool {
            id: "t1".to_string(),
            result: text.to_string(),
            success,
        }
    }

    #[test]
    fn a_successful_push_result_is_a_push() {
        let text =
            r#"{"pushed":true,"merge_queue":{"branch":"aura/a","head":"abc","remote":"origin"}}"#;
        let push = push_in(&result(text, true)).unwrap();
        assert_eq!(push.branch, "aura/a");
        assert_eq!(push.remote.as_deref(), Some("origin"));
    }

    #[test]
    fn failed_results_and_other_commands_are_not_pushes() {
        let text = r#"{"merge_queue":{"branch":"aura/a","head":"abc"}}"#;
        assert_eq!(push_in(&result(text, false)), None);
        assert_eq!(push_in(&result("{\"ok\":true}", true)), None);
        assert_eq!(push_in(&UiCommand::Complete), None);
    }
}
