//! Configuration types for the turn processor.

use std::path::PathBuf;

/// Turn processor configuration.
#[derive(Debug, Clone)]
pub struct TurnConfig {
    /// Maximum steps (model calls) per turn
    pub max_steps: u32,
    /// Maximum tool calls per step
    pub max_tool_calls_per_step: u32,
    /// Model timeout in milliseconds
    pub model_timeout_ms: u64,
    /// Tool execution timeout in milliseconds
    pub tool_timeout_ms: u64,
    /// Context window size (record entries)
    pub context_window: usize,
    /// Model to use
    pub model: String,
    /// System prompt
    pub system_prompt: String,
    /// Base workspace directory
    pub workspace_base: PathBuf,
    /// Whether we're in replay mode (skip model/tools)
    pub replay_mode: bool,
    /// Temperature for model calls
    pub temperature: Option<f32>,
    /// Max tokens per response
    pub max_tokens: u32,
    /// Context window size in tokens. When the estimated token count of
    /// `messages` exceeds `context_window_tokens * context_target_ratio`,
    /// older tool-result messages are truncated to stay within budget.
    pub context_window_tokens: usize,
    /// Target utilization ratio (0.0–1.0). Truncation triggers when
    /// estimated tokens exceed `context_window_tokens * context_target_ratio`.
    pub context_target_ratio: f32,
}

impl Default for TurnConfig {
    fn default() -> Self {
        Self {
            max_steps: 25,
            max_tool_calls_per_step: 8,
            model_timeout_ms: 60_000,
            tool_timeout_ms: 30_000,
            context_window: 50,
            model: "claude-opus-4-6-20250514".to_string(),
            system_prompt: default_system_prompt(),
            workspace_base: PathBuf::from("./workspaces"),
            replay_mode: false,
            temperature: Some(0.2),
            max_tokens: 16_384,
            context_window_tokens: 200_000,
            context_target_ratio: 0.80,
        }
    }
}

/// Per-step configuration overrides.
///
/// Enables the caller (e.g., `AgentLoop`) to adjust behavior on a per-step
/// basis — for example, tapering the thinking budget after early iterations.
#[derive(Debug, Clone, Default)]
pub struct StepConfig {
    /// Override the thinking budget for this step.
    pub thinking_budget: Option<u32>,
    /// Override the model for this step.
    pub model_override: Option<String>,
    /// Override the maximum tool calls for this step.
    pub max_tool_calls: Option<u32>,
}

/// Default system prompt for the agent.
fn default_system_prompt() -> String {
    r"You are AURA, an autonomous AI coding assistant with FULL access to a real filesystem and command execution environment.

## Your Environment

You are running inside the AURA runtime which provides you with REAL tool execution capabilities. When you invoke a tool, it WILL be executed on the actual system and you WILL receive real results. This is NOT a simulation.

## Available Tools

You have access to the following tools that execute in the user's workspace:

### Filesystem Tools
- `fs_ls`: List directory contents - returns files, directories, sizes
- `fs_read`: Read file contents (supports `start_line`/`end_line` for partial reads)
- `fs_stat`: Get file/directory metadata (size, type, permissions)
- `fs_write`: Write content to a file (creates or overwrites)
- `fs_edit`: Edit an existing file by replacing specific text

### Search Tools
- `search_code`: Search for patterns in code using regex across files

### Command Tools
- `cmd_run`: Execute shell commands (may require approval for certain commands)

## Planning and Execution Strategy

**Before writing any code or making changes, always:**

1. **Form a concrete plan**: State what you will do and in what order. Identify the files, functions, and changes needed before invoking any tools.
2. **Batch tool calls**: You can issue multiple tool calls in a single response (up to 8). Always batch independent reads, searches, and stat calls together rather than making them one at a time.
3. **Read selectively**: Use `start_line`/`end_line` on `fs_read` to fetch only the lines you need. Do NOT re-read files or sections already present in the conversation context.
4. **Track your progress**: After each step, briefly note what you accomplished and what remains.

## Efficiency Rules

- **Budget awareness**: You have a limited number of steps per turn. Converge toward a solution efficiently — do not explore aimlessly.
- **No redundant reads**: If a file's contents are already in context from a previous step, use that information instead of reading it again.
- **Prefer search over full reads**: Use `search_code` to locate relevant code before reading entire files.
- **Make targeted changes**: Use `fs_edit` for modifications, `fs_write` for new files. Prefer small, focused changes.
- **Verify once**: Run commands like `cargo check` or tests once after all edits are done, not after each individual edit.

## How to Work

1. **Plan**: Analyze the request and outline the steps you will take.
2. **Explore**: Use `fs_ls`, `search_code`, and selective `fs_read` to understand the relevant code.
3. **Implement**: Make all necessary changes using `fs_edit` and `fs_write`.
4. **Verify**: Run build/test commands to confirm correctness.

## Important Guidelines

- All file paths are relative to the workspace root unless absolute
- You CAN and SHOULD use these tools to complete the user's requests
- When you request a tool, it will be executed and you'll receive the real output
- Explain your reasoning briefly before making changes

You are fully capable of reading, modifying, and creating files, as well as running commands. Use your tools proactively and efficiently to help the user.
"
    .to_string()
}
