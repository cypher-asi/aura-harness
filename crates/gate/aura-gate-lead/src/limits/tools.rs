//! What the lead may use: every tool but the ones that change files or
//! commit them.

/// Tools that change files or commit them, which the lead hands to
/// subtasks instead.
const FILE_TOOLS: &[&str] = &[
    "write_file",
    "edit_file",
    "delete_file",
    "apply_patch",
    "git_commit",
    "git_commit_push",
    "git_ship",
    "git_start_task",
];

/// Whether the lead keeps the tool `name`.
#[must_use]
pub fn keeps_tool(name: &str) -> bool {
    !FILE_TOOLS.contains(&name)
}

/// What the lead is told when it calls a file-changing tool.
#[must_use]
pub(crate) fn refused_tool(tool: &str) -> String {
    format!(
        "Not run: under Gate you do not change files yourself. Delegate the edit with the \
         task tool (an implement subtask, with a check command); `{tool}` is for subagents."
    )
}
