//! Dev-loop automaton – the core continuous task-execution loop.
//!
//! The loop is fully self-managed: it fetches all tasks on first tick,
//! topologically sorts them by dependencies, and executes them one at a
//! time. Task status transitions are handled internally and synced back
//! to the domain API as a best-effort side-effect.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use tracing::{info, warn};

use aura_agent::agent_runner::{
    AgentRunner, AgentRunnerConfig, AgenticTaskParams, ShellTaskParams, TaskExecutionResult,
    TaskTrackingConfig,
};
use aura_agent::prompts::{ProjectInfo, SessionInfo, SpecInfo, TaskInfo};
use aura_reasoner::ModelProvider;
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

pub use tick::commit_and_push;

#[cfg(test)]
mod tests;

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
    agent_instance_id: String,
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

pub fn forward_agent_event(
    tx: &tokio::sync::mpsc::UnboundedSender<AutomatonEvent>,
    evt: aura_agent::events::AgentLoopEvent,
) {
    use aura_agent::events::AgentLoopEvent;
    let automaton_event = match evt {
        AgentLoopEvent::TextDelta(d) => AutomatonEvent::TextDelta { delta: d },
        AgentLoopEvent::ThinkingDelta(d) => AutomatonEvent::ThinkingDelta { delta: d },
        AgentLoopEvent::ToolStart { id, name } => AutomatonEvent::ToolCallStarted { id, name },
        AgentLoopEvent::ToolInputSnapshot { id, name, input } => {
            let Ok(input) = serde_json::from_str::<serde_json::Value>(&input) else {
                return;
            };
            AutomatonEvent::ToolCallSnapshot { id, name, input }
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
        _ => return,
    };
    let _ = tx.send(automaton_event);
}

struct NoOpToolExecutor;

#[async_trait::async_trait]
impl aura_agent::types::AgentToolExecutor for NoOpToolExecutor {
    async fn execute(
        &self,
        tool_calls: &[aura_agent::types::ToolCallInfo],
    ) -> Vec<aura_agent::types::ToolCallResult> {
        tool_calls
            .iter()
            .map(|tc| {
                aura_agent::types::ToolCallResult::error(&tc.id, "no tool executor configured")
            })
            .collect()
    }
}
