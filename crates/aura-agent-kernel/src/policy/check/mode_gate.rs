//! Session [`AgentMode`] gate — the first stage of tool authorization.
//!
//! Every `Delegate` proposal is classified into a [`GatedAction`] and
//! checked against the session mode with [`DefaultModeGate`] before
//! capabilities, scope, integrations, or the per-tool `on`/`off`/`ask`
//! state are consulted. The mode is a ceiling: it can only deny.
//!
//! Classification is an explicit table keyed by tool name. A tool that
//! is not in the table is treated as [`GatedAction::Network`] (an
//! unclassified external effect) so that runtime-installed, MCP, or
//! newly added tools are denied outside `Agent` mode until someone
//! classifies them.

use super::verdict::PolicyVerdict;
use super::Policy;
use aura_core_modes::{AgentMode, DefaultModeGate, GatedAction, ModeGate};
use aura_core_types::ToolCall;

/// Effect class of a known tool, before path-sensitive refinement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolModeClass {
    /// Reads workspace files or agent state.
    Read,
    /// Creates, edits, or deletes a workspace file named by `args.path`.
    FileWrite,
    /// Writes a markdown document outside the workspace (specs).
    MarkdownDocument,
    /// Mutates the workspace without naming a single file (e.g. a commit).
    WorkspaceMutation,
    /// Runs a subprocess.
    Subprocess,
    /// Calls or mutates an external system.
    Network,
    /// Starts, controls, or delegates to another agent or automaton.
    Spawn,
    /// Read-only or bookkeeping tool invocation.
    Invoke,
}

/// Explicit mode classification for a tool name. `None` means the tool
/// is unclassified and [`gated_action`] falls back to
/// [`GatedAction::Network`].
#[must_use]
pub fn tool_mode_class(tool: &str) -> Option<ToolModeClass> {
    use ToolModeClass as C;
    let class = match tool {
        "read_file" | "list_files" | "stat_file" | "find_files" | "search_code"
        | "get_agent_state" | "list_agents" => C::Read,

        "write_file" | "edit_file" | "delete_file" => C::FileWrite,

        "create_spec"
        | "update_spec"
        | "update_spec_section"
        | "append_to_spec"
        | "delete_spec" => C::MarkdownDocument,

        "git_commit" => C::WorkspaceMutation,

        "run_command" => C::Subprocess,

        "git_push"
        | "git_commit_push"
        | "orbit_push"
        | "orbit_create_repo"
        | "orbit_create_branch"
        | "orbit_create_pr"
        | "orbit_merge_pr"
        | "post_to_feed"
        | "record_usage"
        | "create_task"
        | "update_task"
        | "delete_task"
        | "transition_task"
        | "update_project"
        | "computer" => C::Network,

        "spawn_agent" | "task" | "delegate_task" | "send_to_agent" | "agent_lifecycle"
        | "run_task" | "start_dev_loop" | "pause_dev_loop" | "stop_dev_loop" => C::Spawn,

        "list_specs"
        | "get_spec"
        | "list_tasks"
        | "list_projects"
        | "get_project"
        | "check_budget"
        | "orbit_list_repos"
        | "orbit_list_branches"
        | "orbit_list_commits"
        | "orbit_get_diff"
        | "orbit_list_prs"
        | "get_task_context"
        | "task_done"
        | "submit_plan" => C::Invoke,

        _ => return None,
    };
    Some(class)
}

/// Classify a tool call into the [`GatedAction`] the mode gate checks.
#[must_use]
pub fn gated_action(tool_call: &ToolCall) -> GatedAction {
    match tool_mode_class(&tool_call.tool) {
        Some(ToolModeClass::Read) => GatedAction::ReadFile,
        Some(ToolModeClass::FileWrite) => {
            if targets_markdown(&tool_call.args) {
                GatedAction::WriteMarkdown
            } else {
                GatedAction::WriteNonMarkdown
            }
        }
        Some(ToolModeClass::MarkdownDocument) => GatedAction::WriteMarkdown,
        Some(ToolModeClass::WorkspaceMutation) => GatedAction::WriteNonMarkdown,
        Some(ToolModeClass::Subprocess) => GatedAction::Subprocess,
        Some(ToolModeClass::Network) | None => GatedAction::Network,
        Some(ToolModeClass::Spawn) => GatedAction::SpawnAgent,
        Some(ToolModeClass::Invoke) => GatedAction::ToolInvoke {
            name: tool_call.tool.clone(),
        },
    }
}

fn targets_markdown(args: &serde_json::Value) -> bool {
    args.get("path")
        .and_then(serde_json::Value::as_str)
        .and_then(|path| std::path::Path::new(path).extension())
        .and_then(std::ffi::OsStr::to_str)
        .is_some_and(|ext| ext.eq_ignore_ascii_case("md") || ext.eq_ignore_ascii_case("markdown"))
}

impl Policy {
    /// Run the session mode gate for `tool_call`. `None` means the
    /// mode permits the call and later stages decide.
    pub(super) fn check_mode(&self, tool_call: &ToolCall) -> Option<PolicyVerdict> {
        let mode = self.config.mode;
        if mode == AgentMode::Agent {
            return None;
        }
        DefaultModeGate
            .check(mode, gated_action(tool_call))
            .err()
            .map(|violation| PolicyVerdict::Deny {
                reason: format!(
                    "Tool '{}' denied by {mode:?} mode: {violation}",
                    tool_call.tool
                ),
            })
    }
}
