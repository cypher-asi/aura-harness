//! Core types for the agent orchestration layer.

use async_trait::async_trait;

/// Information about a tool call to be executed.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ToolCallInfo {
    /// Tool use ID from the model.
    pub id: String,
    /// Tool name.
    pub name: String,
    /// Tool arguments as JSON.
    pub input: serde_json::Value,
}

/// Result of executing a single tool call.
#[derive(Debug, Clone)]
pub struct ToolCallResult {
    /// Tool use ID.
    pub tool_use_id: String,
    /// Result content (text or error message).
    pub content: String,
    /// Whether the tool execution failed.
    pub is_error: bool,
    /// When true, the loop terminates after processing all results in this batch.
    /// Used by engine tools like `task_done` to signal task completion.
    pub stop_loop: bool,
}

impl ToolCallResult {
    /// Create a successful result.
    #[must_use]
    pub fn success(tool_use_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            tool_use_id: tool_use_id.into(),
            content: content.into(),
            is_error: false,
            stop_loop: false,
        }
    }

    /// Create an error result.
    #[must_use]
    pub fn error(tool_use_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            tool_use_id: tool_use_id.into(),
            content: content.into(),
            is_error: true,
            stop_loop: false,
        }
    }
}

/// Result of an automatic build check.
#[derive(Debug, Clone, Default)]
pub struct AutoBuildResult {
    /// Whether the build succeeded.
    pub success: bool,
    /// Build output (stdout + stderr).
    pub output: String,
    /// Number of errors detected.
    pub error_count: usize,
}

/// Captured build error baseline for distinguishing pre-existing from new errors.
#[derive(Debug, Clone, Default)]
pub struct BuildBaseline {
    /// Error signatures from the baseline build.
    pub error_signatures: Vec<String>,
}

impl BuildBaseline {
    /// Annotate build output by diffing against pre-existing errors.
    #[must_use]
    pub fn annotate(&self, output: &str) -> String {
        if self.error_signatures.is_empty() {
            return output.to_string();
        }
        let current_sigs = Self::extract_signatures(output);
        if current_sigs.is_empty() {
            return output.to_string();
        }
        let mut new_count = 0usize;
        let mut preexisting_count = 0usize;
        for sig in &current_sigs {
            if self.error_signatures.contains(sig) {
                preexisting_count += 1;
            } else {
                new_count += 1;
            }
        }
        if preexisting_count == 0 {
            return output.to_string();
        }
        format!(
            "[BASELINE] {new_count} error(s) are NEW (introduced by your changes), \
             {preexisting_count} error(s) are PRE-EXISTING (ignore them). Focus only on the new errors.\n\n{output}",
        )
    }

    /// Extract individual error blocks and produce a normalized signature per block.
    #[must_use]
    pub fn extract_signatures(stderr: &str) -> Vec<String> {
        let mut signatures = Vec::new();
        let mut current_block = String::new();
        for line in stderr.lines() {
            let trimmed = line.trim_start();
            let is_start = trimmed.starts_with("error[E")
                || (trimmed.starts_with("error:") && !trimmed.starts_with("error: aborting"));
            if is_start && !current_block.is_empty() {
                let sig = Self::normalize_block(&current_block);
                if !sig.is_empty() {
                    signatures.push(sig);
                }
                current_block.clear();
            }
            if !current_block.is_empty() || is_start {
                current_block.push_str(line);
                current_block.push('\n');
            }
        }
        if !current_block.is_empty() {
            let sig = Self::normalize_block(&current_block);
            if !sig.is_empty() {
                signatures.push(sig);
            }
        }
        signatures
    }

    fn normalize_block(block: &str) -> String {
        let mut lines: Vec<String> = Vec::new();
        for line in block.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty()
                || trimmed.starts_with("For more information")
                || trimmed.starts_with("help:")
            {
                continue;
            }
            if trimmed.starts_with("-->") {
                lines.push("-->LOCATION".into());
                continue;
            }
            if trimmed.chars().next().is_some_and(|c| c.is_ascii_digit()) && trimmed.contains('|') {
                continue;
            }
            if trimmed
                .chars()
                .all(|c| c == '^' || c == '-' || c == ' ' || c == '~' || c == '+')
            {
                continue;
            }
            let normalized = Self::strip_line_col(trimmed);
            if !normalized.is_empty() {
                lines.push(normalized);
            }
        }
        lines.sort();
        lines.dedup();
        lines.join("\n")
    }

    fn strip_line_col(line: &str) -> String {
        let mut result = String::with_capacity(line.len());
        let chars: Vec<char> = line.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            if chars[i] == ':' && i + 1 < chars.len() && chars[i + 1].is_ascii_digit() {
                result.push(':');
                result.push('N');
                i += 1;
                while i < chars.len() && chars[i].is_ascii_digit() {
                    i += 1;
                }
            } else {
                result.push(chars[i]);
                i += 1;
            }
        }
        result
    }
}

/// Result of the full agent loop execution.
#[derive(Debug, Default)]
pub struct AgentLoopResult {
    /// Whether the loop timed out.
    pub timed_out: bool,
    /// Whether the loop stopped due to insufficient credits.
    pub insufficient_credits: bool,
    /// Whether the loop stopped due to stall detection.
    pub stalled: bool,
    /// LLM error that terminated the loop, if any.
    pub llm_error: Option<String>,
    /// Accumulated assistant text across all iterations.
    pub total_text: String,
    /// Accumulated thinking text across all iterations.
    pub total_thinking: String,
    /// Total input tokens used.
    pub total_input_tokens: u64,
    /// Total output tokens used.
    pub total_output_tokens: u64,
    /// Total cache creation input tokens used across all iterations.
    pub total_cache_creation_input_tokens: u64,
    /// Total cache read input tokens used across all iterations.
    pub total_cache_read_input_tokens: u64,
    /// Number of iterations completed.
    pub iterations: usize,
    /// Final message history.
    pub messages: Vec<aura_reasoner::Message>,
}

/// Implementors execute tool calls and optionally provide build integration.
///
/// `aura-harness` provides a default implementation wrapping `ExecutorRouter`.
/// `aura-app` can implement this with project-aware paths, domain tools
/// (spec/task CRUD, dev loop, engine phase gating), and event forwarding.
#[async_trait]
pub trait AgentToolExecutor: Send + Sync {
    /// Execute a batch of tool calls.
    ///
    /// Implementations may:
    /// - Gate certain tools (e.g., writes before `submit_plan`)
    /// - Dispatch domain tools to external services
    /// - Track file operations for stub detection
    /// - Signal loop termination via `stop_loop`
    async fn execute(&self, tool_calls: &[ToolCallInfo]) -> Vec<ToolCallResult>;

    /// Run a lightweight build check (e.g., `cargo check --lib`).
    ///
    /// Returns `None` when build checking is not configured.
    async fn auto_build_check(&self) -> Option<AutoBuildResult> {
        None
    }

    /// Capture current build error state as a baseline for distinguishing
    /// pre-existing errors from newly introduced ones.
    async fn capture_build_baseline(&self) -> Option<BuildBaseline> {
        None
    }
}
