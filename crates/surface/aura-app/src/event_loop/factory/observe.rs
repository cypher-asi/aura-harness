//! Seeing every push an agent makes, wherever it runs.
//!
//! The push tools (`git_push`, `git_commit_push`, `git_ship`) put the merge
//! queue marker in their results ([`QueuedPush::from_tool_output`]), and
//! every tool call of every agent, local or cloud, reaches the UI through
//! [`crate::event_loop::agent_events::AgentUi`], which hands each command
//! to [`record`]. A result counts as a push only when its call started as
//! one of those tools ([`UiCommand::ShowTool`] names it), so another
//! tool's output (a `run_command` echoing a marker) can't queue anything.
//! A cloud turn's conversation is read again when it ends
//! ([`super::turn::pushes`]), so a push whose stream was cut off still
//! lands in the queue; the queue drops the repeat.
//!
//! Like the chat notes, the sink is process-wide: it is set once, when
//! the factory starts.

use aura_core_types::AgentId;
use aura_exec_merge_queue::QueuedPush;
use aura_exec_tools::GIT_REMOTE_TOOL_NAMES;
use aura_ui::UiCommand;
use std::collections::HashSet;
use std::sync::{Mutex, OnceLock, PoisonError};
use tokio::sync::mpsc;

/// Most push calls followed at once; more means results were lost, and
/// the oldest are forgotten.
const MAX_OPEN_CALLS: usize = 4_096;

/// A push an agent made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ObservedPush {
    /// The agent that pushed
    pub agent: AgentId,
    /// What it pushed
    pub push: QueuedPush,
}

static PUSHES: OnceLock<mpsc::UnboundedSender<ObservedPush>> = OnceLock::new();

/// Ids of push tool calls that started and have no result yet.
static OPEN_CALLS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Send every push seen from now on to `sink`. Only the first call takes.
pub(crate) fn install(sink: mpsc::UnboundedSender<ObservedPush>) -> bool {
    PUSHES.set(sink).is_ok()
}

/// Whether `name` is a tool whose results carry pushes.
pub(crate) fn is_push_tool(name: &str) -> bool {
    GIT_REMOTE_TOOL_NAMES.contains(&name)
}

/// Follow `command` of an agent's chat; returns the push it completes,
/// if it is a push tool's successful result.
pub(crate) fn push_in(command: &UiCommand) -> Option<QueuedPush> {
    let mut open = OPEN_CALLS.lock().unwrap_or_else(PoisonError::into_inner);
    match command {
        UiCommand::ShowTool(tool) if is_push_tool(&tool.name) => {
            if open.len() >= MAX_OPEN_CALLS {
                open.remove(0);
            }
            open.push(tool.id.clone());
            None
        }
        UiCommand::CompleteTool {
            id,
            result,
            success,
        } => {
            let at = open.iter().position(|open| open == id)?;
            open.remove(at);
            success
                .then(|| QueuedPush::from_tool_output(result))
                .flatten()
        }
        _ => None,
    }
}

/// Hand the push in `command` of `agent`'s chat, if any, to the factory.
pub(crate) fn record(agent: AgentId, command: &UiCommand) {
    if let Some(push) = push_in(command) {
        report(ObservedPush { agent, push });
    }
}

/// Hand `push` to the factory; dropped (logged) before it started.
pub(crate) fn report(push: ObservedPush) {
    match PUSHES.get() {
        Some(sink) => {
            // The service is gone only while the app shuts down, when no
            // push needs merging any more.
            if let Err(lost) = sink.send(push) {
                tracing::debug!(agent = %lost.0.agent, "The merge queue has stopped; a push was not queued");
            }
        }
        None => {
            tracing::warn!(agent = %push.agent, branch = %push.push.branch, "A push was made before the merge queue started");
        }
    }
}

/// Distinct pushes in `pushes`, first seen first.
pub(crate) fn distinct(pushes: Vec<QueuedPush>) -> Vec<QueuedPush> {
    let mut seen = HashSet::new();
    pushes
        .into_iter()
        .filter(|push| seen.insert((push.branch.clone(), push.head.clone())))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use aura_ui::events::ToolData;

    const MARKER: &str =
        r#"{"pushed":true,"merge_queue":{"branch":"aura/a","head":"abc","remote":"origin"}}"#;

    fn start(id: &str, name: &str) -> UiCommand {
        UiCommand::ShowTool(ToolData {
            id: id.to_string(),
            name: name.to_string(),
            args: String::new(),
        })
    }

    fn result(id: &str, text: &str, success: bool) -> UiCommand {
        UiCommand::CompleteTool {
            id: id.to_string(),
            result: text.to_string(),
            success,
        }
    }

    #[test]
    fn a_push_tools_successful_result_is_a_push() {
        for tool in GIT_REMOTE_TOOL_NAMES {
            let id = format!("push-{tool}");
            assert_eq!(push_in(&start(&id, tool)), None);
            let push = push_in(&result(&id, MARKER, true)).unwrap();
            assert_eq!(push.branch, "aura/a");
            assert_eq!(push.remote.as_deref(), Some("origin"));
        }
    }

    #[test]
    fn another_tools_output_with_a_marker_is_not_a_push() {
        push_in(&start("forged", "run_command"));
        assert_eq!(push_in(&result("forged", MARKER, true)), None);
        assert_eq!(push_in(&result("never-started", MARKER, true)), None);
    }

    #[test]
    fn failed_results_and_other_commands_are_not_pushes() {
        push_in(&start("failed-push", "git_push"));
        assert_eq!(push_in(&result("failed-push", MARKER, false)), None);
        push_in(&start("plain-push", "git_push"));
        assert_eq!(push_in(&result("plain-push", "{\"ok\":true}", true)), None);
        assert_eq!(push_in(&UiCommand::Complete), None);
    }

    #[test]
    fn repeats_are_dropped() {
        let push = |head: &str| QueuedPush {
            branch: "b".to_string(),
            head: head.to_string(),
            remote: None,
        };
        assert_eq!(
            distinct(vec![push("1"), push("2"), push("1")]),
            [push("1"), push("2")]
        );
    }
}
