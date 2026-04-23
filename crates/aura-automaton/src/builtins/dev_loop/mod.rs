//! Dev-loop automaton – the core continuous task-execution loop.
//!
//! The loop is fully self-managed: it fetches all tasks on first tick,
//! topologically sorts them by dependencies, and executes them one at a
//! time. Task status transitions are handled internally and synced back
//! to the domain API as a best-effort side-effect.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use aura_agent::agent_runner::{
    AgentRunner, AgentRunnerConfig, AgenticTaskParams, ShellTaskParams, TaskExecutionResult,
    TaskTrackingConfig,
};
use aura_agent::prompts::{ProjectInfo, SessionInfo, SpecInfo, TaskInfo};
use aura_reasoner::{ContentBlock, Message, ModelProvider, Role};
use aura_tools::catalog::{ToolCatalog, ToolProfile};
use aura_tools::domain_tools::{DomainApi, TaskDescriptor};

use crate::context::TickContext;
use crate::error::AutomatonError;
use crate::events::AutomatonEvent;
use crate::runtime::{Automaton, TickOutcome};
use crate::schedule::Schedule;

mod finish;
mod run;
mod tick;

pub(crate) use tick::commit_and_push;

/// Reason string attached to `AutomatonEvent::CommitSkipped` when
/// the DoD precheck trips. Kept as a module constant so the tick.rs
/// skip path and any future task-run callers stay in lockstep.
pub(crate) const COMMIT_SKIPPED_NO_CHANGES: &str =
    "no file changes or verification evidence; skipping commit to avoid orphan commits";

/// Per-task aggregate the automaton consults before dispatching
/// `git_commit` / `git_commit_push`. The canonical server-side
/// `CachedTaskOutput` lives in `aura-os-server` and is not visible
/// from this crate, so we derive an equivalent shape locally from
/// the `TaskExecutionResult` the runner hands back:
///
/// * `files_changed` counts the file-mutation evidence we can see
///   without talking to the server: `exec.file_ops` (the canonical
///   list of writes the runner actually applied) plus the set of
///   unique `path`s from successful `write_file` / `edit_file` /
///   `delete_file` `tool_result`s in the task's message log. Using
///   the max of the two is a belt-and-braces signal: `file_ops`
///   may lag when the runner abandoned a partially-applied batch,
///   and the message scan catches successful tool calls whose side
///   effects never made it into `file_ops`.
///
/// * `verification_steps` counts successful `run_command`
///   `tool_result`s. `run_command` is the only generic shell tool
///   in the catalog (see `aura_agent::constants::COMMAND_TOOLS`)
///   so build / test / fmt / lint invocations all flow through it;
///   a single non-error result is treated as verification evidence.
///
/// `should_skip_commit` trips only when both counters are zero,
/// matching the plan's "no file changes AND no verification
/// evidence" trigger for the commit-skip DoD precheck.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TaskAggregate {
    pub files_changed: usize,
    pub verification_steps: usize,
}

impl TaskAggregate {
    /// Derive the aggregate from a completed `TaskExecutionResult`.
    pub(crate) fn from_exec(exec: &TaskExecutionResult) -> Self {
        let mut tool_uses: HashMap<String, String> = HashMap::new();
        let mut successful_file_paths: HashSet<String> = HashSet::new();
        let mut verification_steps: usize = 0;

        // First pass: index tool_use ids -> tool name so we can
        // classify the matching tool_result blocks in the second
        // pass. The assistant always emits the tool_use before the
        // user-side tool_result, so a single forward pass over the
        // message log is sufficient.
        for msg in &exec.messages {
            match msg.role {
                Role::Assistant => {
                    for block in &msg.content {
                        if let ContentBlock::ToolUse { id, name, .. } = block {
                            tool_uses.insert(id.clone(), name.clone());
                        }
                    }
                }
                Role::User => {
                    for block in &msg.content {
                        if let ContentBlock::ToolResult {
                            tool_use_id,
                            is_error,
                            ..
                        } = block
                        {
                            if *is_error {
                                continue;
                            }
                            let Some(tool_name) = tool_uses.get(tool_use_id) else {
                                continue;
                            };
                            match tool_name.as_str() {
                                "write_file" | "edit_file" | "delete_file" => {
                                    // Recover the path argument from
                                    // the originating tool_use to
                                    // dedupe repeated writes to the
                                    // same file. Tool_uses without a
                                    // `path` field still count via
                                    // their id (rare: malformed
                                    // input).
                                    let path_key = exec
                                        .messages
                                        .iter()
                                        .flat_map(|m| m.content.iter())
                                        .find_map(|b| match b {
                                            ContentBlock::ToolUse { id, input, .. }
                                                if id == tool_use_id =>
                                            {
                                                input
                                                    .get("path")
                                                    .and_then(|v| v.as_str())
                                                    .map(str::to_string)
                                            }
                                            _ => None,
                                        })
                                        .unwrap_or_else(|| format!("<id:{tool_use_id}>"));
                                    successful_file_paths.insert(path_key);
                                }
                                "run_command" => {
                                    verification_steps += 1;
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
        }

        // Prefer the max of the runner-reported `file_ops` count and
        // the message-derived count (see struct docs).
        let files_changed = exec.file_ops.len().max(successful_file_paths.len());

        Self {
            files_changed,
            verification_steps,
        }
    }

    /// DoD precheck: skip the commit only when we have neither any
    /// observed file changes nor any verification-step evidence.
    pub(crate) fn should_skip_commit(&self) -> bool {
        self.files_changed == 0 && self.verification_steps == 0
    }
}

#[cfg(test)]
mod tests;

/// Structured hint attached to a `NeedsDecomposition` outcome so the
/// orchestrator (Phase 3, in aura-os) can auto-split a task that reached
/// implementation phase but produced no file operations. Empty/None fields
/// are expected when the validator cannot reliably recover the context.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct DecompositionHint {
    /// Unique paths the agent attempted to `write_file` / `edit_file`
    /// without ever producing a non-error `tool_result`.
    pub failed_paths: Vec<String>,
    /// Name of the most recent assistant-side tool_use block, if any.
    pub last_pending_tool_name: Option<String>,
    /// Short JSON summary of that tool_use's input (via
    /// `aura_agent::helpers::summarize_write_input` when applicable).
    pub last_pending_tool_input_summary: Option<String>,
}

const STATE_COMPLETED_COUNT: &str = "completed_count";
const STATE_FAILED_COUNT: &str = "failed_count";
const STATE_WORK_LOG: &str = "work_log";
const STATE_RETRY_COUNTS: &str = "retry_counts";
const STATE_LOOP_FINISHED: &str = "loop_finished";
const STATE_TASK_QUEUE: &str = "task_queue";
const STATE_DONE_IDS: &str = "done_ids";
const STATE_FAILED_IDS: &str = "failed_ids";
const STATE_FAILURE_REASONS: &str = "failure_reasons";
const STATE_INITIALIZED: &str = "initialized";

const MAX_RETRIES_PER_TASK: u32 = 2;

struct DevLoopConfig {
    project_id: String,
    // TODO: will be used when dev-loop sessions tag their agent instance
    #[allow(dead_code)]
    agent_instance_id: String,
    // TODO: will be used for model selection in dev-loop
    #[allow(dead_code)]
    model: String,
}

impl DevLoopConfig {
    fn from_json(config: &serde_json::Value) -> Result<Self, AutomatonError> {
        let project_id = config
            .get("project_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AutomatonError::InvalidConfig("missing project_id".into()))?
            .to_string();
        let agent_instance_id = config
            .get("agent_instance_id")
            .and_then(|v| v.as_str())
            .unwrap_or("default")
            .to_string();
        let model = config
            .get("model")
            .and_then(|v| v.as_str())
            .unwrap_or(aura_agent::DEFAULT_MODEL)
            .to_string();
        Ok(Self {
            project_id,
            agent_instance_id,
            model,
        })
    }
}

pub struct DevLoopAutomaton {
    domain: Arc<dyn DomainApi>,
    provider: Arc<dyn ModelProvider>,
    runner: AgentRunner,
    catalog: Arc<ToolCatalog>,
    tool_executor: Option<Arc<dyn aura_agent::types::AgentToolExecutor>>,
}

impl DevLoopAutomaton {
    pub fn new(
        domain: Arc<dyn DomainApi>,
        provider: Arc<dyn ModelProvider>,
        config: AgentRunnerConfig,
        catalog: Arc<ToolCatalog>,
    ) -> Self {
        Self {
            domain,
            provider,
            runner: AgentRunner::new(config),
            catalog,
            tool_executor: None,
        }
    }

    #[must_use]
    pub fn with_tool_executor(
        mut self,
        executor: Arc<dyn aura_agent::types::AgentToolExecutor>,
    ) -> Self {
        self.tool_executor = Some(executor);
        self
    }
}

/// Topologically sort tasks by dependencies. Returns task IDs in execution
/// order. Tasks with no dependencies come first.
fn topological_sort(tasks: &[TaskDescriptor]) -> Vec<String> {
    let task_ids: HashSet<&str> = tasks.iter().map(|t| t.id.as_str()).collect();
    let mut in_degree: HashMap<&str, usize> = HashMap::new();
    let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();

    for t in tasks {
        in_degree.entry(&t.id).or_insert(0);
        adj.entry(&t.id).or_default();
        for dep in &t.dependencies {
            if task_ids.contains(dep.as_str()) {
                adj.entry(dep.as_str()).or_default().push(&t.id);
                *in_degree.entry(&t.id).or_insert(0) += 1;
            }
        }
    }

    let mut queue: VecDeque<&str> = in_degree
        .iter()
        .filter(|(_, &deg)| deg == 0)
        .map(|(&id, _)| id)
        .collect();

    // Stable sort: prefer tasks by their order field
    let order_map: HashMap<&str, u32> = tasks.iter().map(|t| (t.id.as_str(), t.order)).collect();
    let mut queue_vec: Vec<&str> = queue.iter().copied().collect();
    queue.clear();
    queue_vec.sort_by_key(|id| order_map.get(id).copied().unwrap_or(u32::MAX));
    queue = queue_vec.into_iter().collect();

    let mut result = Vec::new();
    while let Some(node) = queue.pop_front() {
        result.push(node.to_string());
        if let Some(neighbors) = adj.get(node) {
            let mut next_batch: Vec<&str> = Vec::new();
            for &neighbor in neighbors {
                if let Some(deg) = in_degree.get_mut(neighbor) {
                    *deg -= 1;
                    if *deg == 0 {
                        next_batch.push(neighbor);
                    }
                }
            }
            next_batch.sort_by_key(|id| order_map.get(id).copied().unwrap_or(u32::MAX));
            for n in next_batch {
                queue.push_back(n);
            }
        }
    }

    result
}

pub fn extract_shell_command(task: &TaskDescriptor) -> Option<String> {
    let title_lower = task.title.to_lowercase();
    if title_lower.starts_with("run:") || title_lower.starts_with("shell:") {
        let cmd = task.title.split_once(':')?.1.trim().to_string();
        if !cmd.is_empty() {
            return Some(cmd);
        }
    }
    None
}

/// Validate an agent-task execution result. Returns:
/// - `Ok(exec)` when the task produced file ops or explicitly declared
///   no-changes-needed.
/// - `Err(AutomatonError::NeedsDecomposition { hint })` when the task
///   reached the implementing phase but produced no file ops — the caller
///   (or the Phase 3 orchestrator in aura-os) can consume the hint to
///   auto-split and retry.
/// - `Err(AutomatonError::AgentExecution(..))` for the classic
///   "completed-without-changes" case that never reached implementing.
pub(crate) fn validate_execution(
    exec: TaskExecutionResult,
) -> Result<TaskExecutionResult, AutomatonError> {
    if !exec.file_ops.is_empty() || exec.no_changes_needed {
        return Ok(exec);
    }

    if exec.reached_implementing {
        let hint = build_decomposition_hint(&exec.messages);
        return Err(AutomatonError::NeedsDecomposition { hint });
    }

    Err(AutomatonError::AgentExecution(
        "task completed without any file operations — completion not verified".into(),
    ))
}

/// Extract a best-effort `DecompositionHint` from the final message history
/// of a task that reached implementation phase without any file ops.
///
/// `failed_paths` = unique paths from write_file/edit_file tool_use blocks
/// whose tool_use id never produced a non-error tool_result.
/// `last_pending_tool_name` = name of the last ToolUse in the most recent
/// assistant message.
/// `last_pending_tool_input_summary` = short summary via
/// `aura_agent::helpers::summarize_write_input` (when it applies) or the
/// raw JSON truncated to a reasonable length.
pub(crate) fn build_decomposition_hint(messages: &[Message]) -> DecompositionHint {
    if messages.is_empty() {
        return DecompositionHint::default();
    }

    let mut tool_uses: HashMap<String, (String, serde_json::Value)> = HashMap::new();
    let mut successful_ids: HashSet<String> = HashSet::new();

    for msg in messages {
        match msg.role {
            Role::Assistant => {
                for block in &msg.content {
                    if let ContentBlock::ToolUse { id, name, input } = block {
                        tool_uses.insert(id.clone(), (name.clone(), input.clone()));
                    }
                }
            }
            Role::User => {
                for block in &msg.content {
                    if let ContentBlock::ToolResult {
                        tool_use_id,
                        is_error,
                        ..
                    } = block
                    {
                        if !*is_error {
                            successful_ids.insert(tool_use_id.clone());
                        }
                    }
                }
            }
        }
    }

    let mut failed_paths: Vec<String> = Vec::new();
    let mut seen_paths: HashSet<String> = HashSet::new();
    for (id, (name, input)) in &tool_uses {
        if successful_ids.contains(id) {
            continue;
        }
        if !matches!(name.as_str(), "write_file" | "edit_file") {
            continue;
        }
        if let Some(path) = input.get("path").and_then(|v| v.as_str()) {
            if seen_paths.insert(path.to_string()) {
                failed_paths.push(path.to_string());
            }
        }
    }

    let (last_pending_tool_name, last_pending_tool_input_summary) = last_pending_tool_use(messages);

    DecompositionHint {
        failed_paths,
        last_pending_tool_name,
        last_pending_tool_input_summary,
    }
}

fn last_pending_tool_use(messages: &[Message]) -> (Option<String>, Option<String>) {
    let last_assistant = messages.iter().rev().find(|m| m.role == Role::Assistant);
    let Some(msg) = last_assistant else {
        return (None, None);
    };
    let last_tool_use = msg.content.iter().rev().find_map(|b| match b {
        ContentBlock::ToolUse { name, input, .. } => Some((name.clone(), input.clone())),
        _ => None,
    });
    let Some((name, input)) = last_tool_use else {
        return (None, None);
    };

    let summary = aura_agent::helpers::summarize_write_input(&name, &input)
        .and_then(|v| serde_json::to_string(&v).ok())
        .or_else(|| serde_json::to_string(&input).ok())
        .map(|s| truncate_summary(&s, 240));

    (Some(name), summary)
}

fn truncate_summary(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let mut cut = max;
        while !s.is_char_boundary(cut) && cut > 0 {
            cut -= 1;
        }
        format!("{}…", &s[..cut])
    }
}

pub fn forward_agent_event(
    tx: &tokio::sync::mpsc::Sender<AutomatonEvent>,
    evt: aura_agent::AgentLoopEvent,
) {
    use aura_agent::AgentLoopEvent;
    let automaton_event = match evt {
        AgentLoopEvent::TextDelta(d) => AutomatonEvent::TextDelta { delta: d },
        AgentLoopEvent::ThinkingDelta(d) => AutomatonEvent::ThinkingDelta { delta: d },
        AgentLoopEvent::ToolStart { id, name } => AutomatonEvent::ToolCallStarted { id, name },
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
                    id,
                    name,
                    input: parsed,
                    snapshot_partial: false,
                },
                Err(_) => AutomatonEvent::ToolCallSnapshot {
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
            id: tool_use_id,
            name: tool_name,
            result: content,
            is_error,
        },
        AgentLoopEvent::IterationComplete {
            input_tokens,
            output_tokens,
            ..
        } => AutomatonEvent::TokenUsage {
            input_tokens,
            output_tokens,
        },
        AgentLoopEvent::Warning(msg) => AutomatonEvent::LogLine { message: msg },
        AgentLoopEvent::Error { message, .. } => AutomatonEvent::Error {
            automaton_id: String::new(),
            message,
        },
        // Per-tool-call streaming retry lifecycle. `task_id` is left
        // empty here because this forwarder is not task-aware; the
        // surrounding tick.rs / task_run.rs emits TaskStarted with the
        // id so UI consumers can correlate on the preceding frame.
        AgentLoopEvent::ToolCallRetrying {
            tool_use_id,
            tool_name,
            attempt,
            max_attempts,
            delay_ms,
            reason,
        } => AutomatonEvent::ToolCallRetrying {
            task_id: String::new(),
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
            task_id: String::new(),
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
