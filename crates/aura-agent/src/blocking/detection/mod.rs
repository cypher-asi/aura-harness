//! Blocking detection logic.
//!
//! Each detector examines the current tool call against loop state and
//! returns whether to block it (with a recovery message for the model).

use crate::constants::{
    CMD_FAILURE_BLOCK_THRESHOLD, COMMAND_TOOLS, EXPLORATION_TOOLS, MAX_RANGE_READS_PER_FILE,
    MAX_READS_PER_FILE, WRITE_COOLDOWN_ITERATIONS, WRITE_FAILURE_BLOCK_THRESHOLD, WRITE_TOOLS,
};
use crate::read_guard::ReadGuardState;
use crate::types::ToolCallInfo;
use std::collections::{HashMap, HashSet};

/// Mutable state for blocking detection across iterations.
#[derive(Debug, Default)]
pub struct BlockingContext {
    /// Paths that have been successfully written to in previous iterations.
    pub(crate) written_paths: HashSet<String>,
    /// Per-file write failure counts.
    pub(crate) write_failures: HashMap<String, usize>,
    /// Consecutive command failures across iterations.
    pub(crate) consecutive_cmd_failures: usize,
    /// Per-path write cooldowns (iterations remaining).
    pub(crate) write_cooldowns: HashMap<String, usize>,
    /// Current exploration count.
    pub(crate) exploration_count: usize,
    /// Exploration allowance (may be extended on successful writes).
    pub(crate) exploration_allowance: usize,
    /// Count of write tool calls that had no extractable path (malformed args).
    pub(crate) malformed_write_count: usize,
}

impl BlockingContext {
    /// Create a new blocking context with the given exploration allowance.
    #[must_use]
    pub fn new(exploration_allowance: usize) -> Self {
        Self {
            exploration_allowance,
            ..Self::default()
        }
    }

    /// Decrement all write cooldowns, removing expired ones.
    pub(crate) fn decrement_cooldowns(&mut self) {
        self.write_cooldowns.retain(|_, v| {
            *v = v.saturating_sub(1);
            *v > 0
        });
    }

    /// Record a successful write to extend exploration allowance and reset read guards.
    pub(crate) fn on_write_success(&mut self, path: &str, read_guard: &mut ReadGuardState) {
        self.written_paths.insert(path.to_string());
        self.write_failures.remove(path);
        self.exploration_allowance += 2;
        read_guard.reset_for_path(path);
    }

    /// Record a write failure.
    pub(crate) fn on_write_failure(&mut self, path: &str) {
        let count = self.write_failures.entry(path.to_string()).or_insert(0);
        *count += 1;
        if *count >= WRITE_FAILURE_BLOCK_THRESHOLD {
            self.write_cooldowns
                .insert(path.to_string(), WRITE_COOLDOWN_ITERATIONS);
        }
    }

    /// Record a write tool call with missing/invalid path.
    pub(crate) fn on_malformed_write(&mut self) {
        self.malformed_write_count += 1;
    }

    /// Record a command result (success or failure).
    pub(crate) fn on_command_result(&mut self, success: bool) {
        if success {
            self.consecutive_cmd_failures = 0;
        } else {
            self.consecutive_cmd_failures += 1;
        }
    }
}

/// Result of checking whether a tool call should be blocked.
#[derive(Debug)]
pub struct BlockCheckResult {
    /// Whether the tool call is blocked.
    pub(crate) blocked: bool,
    /// Recovery message to inject if blocked.
    pub(crate) recovery_message: Option<String>,
}

impl BlockCheckResult {
    const fn allowed() -> Self {
        Self {
            blocked: false,
            recovery_message: None,
        }
    }

    fn blocked(msg: impl Into<String>) -> Self {
        Self {
            blocked: true,
            recovery_message: Some(msg.into()),
        }
    }
}

/// Check if a tool call should be blocked based on all detectors.
pub fn detect_all_blocked(
    tool: &ToolCallInfo,
    ctx: &BlockingContext,
    read_guard: &ReadGuardState,
) -> BlockCheckResult {
    if let Some(result) = detect_missing_required_args(tool) {
        if result.blocked {
            return result;
        }
    }

    if let Some(result) = detect_blocked_writes(tool, ctx) {
        if result.blocked {
            return result;
        }
    }

    if let Some(result) = detect_blocked_write_failures(tool, ctx) {
        if result.blocked {
            return result;
        }
    }

    if let Some(result) = detect_blocked_commands(tool, ctx) {
        if result.blocked {
            return result;
        }
    }

    if let Some(result) = detect_blocked_exploration(tool, ctx) {
        if result.blocked {
            return result;
        }
    }

    if let Some(result) = detect_blocked_reads(tool, read_guard) {
        if result.blocked {
            return result;
        }
    }

    if let Some(result) = detect_write_cooldowns(tool, ctx) {
        if result.blocked {
            return result;
        }
    }

    if let Some(result) = detect_shell_read_workaround(tool) {
        if result.blocked {
            return result;
        }
    }

    BlockCheckResult::allowed()
}

/// Detector 0: Block tools that are missing required arguments.
///
/// When a model emits a tool call with empty input `{}`, downstream detectors
/// all return `None` (inapplicable) instead of blocking, letting the call
/// through to the executor where it fails and disrupts stall detection.
/// This detector catches that case upfront for all tool families.
fn detect_missing_required_args(tool: &ToolCallInfo) -> Option<BlockCheckResult> {
    if WRITE_TOOLS.contains(&tool.name.as_str()) && extract_path(tool).is_none() {
        return Some(BlockCheckResult::blocked(format!(
            "`{}` requires a `path` argument. Provide the file path to operate on.",
            tool.name
        )));
    }
    if COMMAND_TOOLS.contains(&tool.name.as_str()) {
        let has_command = tool
            .input
            .get("command")
            .and_then(|v| v.as_str())
            .is_some_and(|s| !s.trim().is_empty());
        if !has_command {
            return Some(BlockCheckResult::blocked(format!(
                "`{}` requires a `command` argument. Provide the shell command to execute.",
                tool.name
            )));
        }
    }
    if EXPLORATION_TOOLS.contains(&tool.name.as_str())
        && tool.name == "read_file"
        && extract_path(tool).is_none()
    {
        return Some(BlockCheckResult::blocked(
            "`read_file` requires a `path` argument. Provide the file path to read.".to_string(),
        ));
    }
    None
}

fn extract_path(tool: &ToolCallInfo) -> Option<String> {
    tool.input
        .get("path")
        .and_then(|v| v.as_str())
        .map(String::from)
}

/// Detector 1: Block duplicate full-file writes to paths already written in this turn.
///
/// Only blocks `write_file` (full rewrites). `edit_file` and `delete_file`
/// are allowed so the agent can make targeted changes after an initial write.
fn detect_blocked_writes(tool: &ToolCallInfo, ctx: &BlockingContext) -> Option<BlockCheckResult> {
    if tool.name != "write_file" {
        return None;
    }
    let path = extract_path(tool)?;
    if ctx.written_paths.contains(&path) {
        Some(BlockCheckResult::blocked(format!(
            "You already wrote to `{path}` in this turn. Use `edit_file` to make targeted changes \
             instead of rewriting the entire file. If you need to rewrite, read the file first \
             to verify your changes."
        )))
    } else {
        Some(BlockCheckResult::allowed())
    }
}

/// Detector 2: Block writes to files that have failed too many times.
fn detect_blocked_write_failures(
    tool: &ToolCallInfo,
    ctx: &BlockingContext,
) -> Option<BlockCheckResult> {
    if !WRITE_TOOLS.contains(&tool.name.as_str()) {
        return None;
    }
    let path = extract_path(tool)?;
    if let Some(&count) = ctx.write_failures.get(&path) {
        if count >= WRITE_FAILURE_BLOCK_THRESHOLD {
            return Some(BlockCheckResult::blocked(format!(
                "Writes to `{path}` have failed {count} times. Try a different approach \
                 or read the file to understand its current state."
            )));
        }
    }
    Some(BlockCheckResult::allowed())
}

/// Detector 3: Block all commands after too many consecutive failures.
fn detect_blocked_commands(tool: &ToolCallInfo, ctx: &BlockingContext) -> Option<BlockCheckResult> {
    if !COMMAND_TOOLS.contains(&tool.name.as_str()) {
        return None;
    }
    if ctx.consecutive_cmd_failures >= CMD_FAILURE_BLOCK_THRESHOLD {
        Some(BlockCheckResult::blocked(format!(
            "Commands have failed {} consecutive times. Fix the underlying issue before \
             running more commands. Review error messages and make code changes first.",
            ctx.consecutive_cmd_failures
        )))
    } else {
        Some(BlockCheckResult::allowed())
    }
}

/// Detector 4: Block exploration tools when allowance is exceeded.
fn detect_blocked_exploration(
    tool: &ToolCallInfo,
    ctx: &BlockingContext,
) -> Option<BlockCheckResult> {
    if !EXPLORATION_TOOLS.contains(&tool.name.as_str()) {
        return None;
    }
    if ctx.exploration_count >= ctx.exploration_allowance {
        Some(BlockCheckResult::blocked(
            "Exploration budget exceeded. You have spent too many iterations reading files \
             and searching without making changes. Start implementing now with the information \
             you have.",
        ))
    } else {
        Some(BlockCheckResult::allowed())
    }
}

/// Detector 5: Block reads that exceed the per-file read guard limits.
fn detect_blocked_reads(
    tool: &ToolCallInfo,
    read_guard: &ReadGuardState,
) -> Option<BlockCheckResult> {
    let is_read = tool.name == "read_file";
    if !is_read {
        return None;
    }
    let path = extract_path(tool)?;
    let is_range = tool.input.get("start_line").is_some() || tool.input.get("end_line").is_some();

    if is_range {
        if read_guard.range_read_count(&path) >= MAX_RANGE_READS_PER_FILE {
            return Some(BlockCheckResult::blocked(format!(
                "You have read ranges of `{path}` too many times. The content should already \
                 be in your context. Use the information you have."
            )));
        }
    } else if read_guard.full_read_count(&path) >= MAX_READS_PER_FILE {
        return Some(BlockCheckResult::blocked(format!(
            "You have read `{path}` in full too many times. The content is already in your \
             context. Use the information you have or read a specific line range."
        )));
    }

    Some(BlockCheckResult::allowed())
}

/// Detector 6: Block writes to paths with active cooldowns.
fn detect_write_cooldowns(tool: &ToolCallInfo, ctx: &BlockingContext) -> Option<BlockCheckResult> {
    if !WRITE_TOOLS.contains(&tool.name.as_str()) {
        return None;
    }
    let path = extract_path(tool)?;
    if let Some(&remaining) = ctx.write_cooldowns.get(&path) {
        if remaining > 0 {
            return Some(BlockCheckResult::blocked(format!(
                "Writes to `{path}` are on cooldown ({remaining} iterations remaining) \
                 due to repeated failures. Try a different approach."
            )));
        }
    }
    Some(BlockCheckResult::allowed())
}

/// Detector 7: Block shell commands that are just reading files.
fn detect_shell_read_workaround(tool: &ToolCallInfo) -> Option<BlockCheckResult> {
    if !COMMAND_TOOLS.contains(&tool.name.as_str()) {
        return None;
    }
    let command = tool
        .input
        .get("command")
        .or_else(|| tool.input.get("args"))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if is_shell_read_cmd(command) {
        Some(BlockCheckResult::blocked(
            "Using shell commands to read files is not allowed. \
             Use `read_file` instead.",
        ))
    } else {
        Some(BlockCheckResult::allowed())
    }
}

/// Check if a shell command is just reading a file.
pub fn is_shell_read_cmd(command: &str) -> bool {
    let lower = command.to_lowercase();
    let read_cmds = [
        "cat ",
        "type ",
        "get-content ",
        "head ",
        "tail ",
        "less ",
        "more ",
    ];
    read_cmds
        .iter()
        .any(|cmd| lower.starts_with(cmd) || lower.contains(&format!("| {cmd}")))
}

#[cfg(test)]
mod tests;
