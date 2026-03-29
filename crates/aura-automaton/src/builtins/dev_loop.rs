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
            .unwrap_or(aura_core::DEFAULT_MODEL)
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
    let mut queue_vec: Vec<&str> = queue.drain(..).collect();
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

#[async_trait::async_trait]
impl Automaton for DevLoopAutomaton {
    fn kind(&self) -> &str {
        "dev-loop"
    }

    fn default_schedule(&self) -> Schedule {
        Schedule::Continuous
    }

    async fn on_install(&self, ctx: &TickContext) -> Result<(), AutomatonError> {
        let cfg = DevLoopConfig::from_json(&ctx.config)?;
        info!(project_id = %cfg.project_id, "Dev loop automaton installed");
        ctx.emit(AutomatonEvent::LogLine {
            message: format!("dev loop starting for project {}", cfg.project_id),
        });
        Ok(())
    }

    async fn tick(&self, ctx: &mut TickContext) -> Result<TickOutcome, AutomatonError> {
        if ctx.is_cancelled() {
            return Ok(TickOutcome::Done);
        }

        let cfg = DevLoopConfig::from_json(&ctx.config)?;
        let project_id = &cfg.project_id;

        // ==================================================================
        // 0. Initialize: fetch all tasks, sort, build internal queue
        // ==================================================================
        let initialized: bool = ctx.state.get(STATE_INITIALIZED).unwrap_or(false);
        if !initialized {
            if self.tool_executor.is_none() {
                return Err(AutomatonError::InvalidConfig(
                    "no tool executor configured — the agent cannot perform file or command operations".into(),
                ));
            }

            let tasks = self
                .domain
                .list_tasks(project_id, None, None)
                .await
                .map_err(|e| AutomatonError::DomainApi(e.to_string()))?;

            if tasks.is_empty() {
                info!("No tasks found for project, finishing");
                return self.finish(ctx).await;
            }

            let already_done: Vec<String> = tasks
                .iter()
                .filter(|t| t.status == "done")
                .map(|t| t.id.clone())
                .collect();

            let executable: Vec<&TaskDescriptor> = tasks
                .iter()
                .filter(|t| t.status != "done")
                .collect();

            let sorted = topological_sort(
                &executable.iter().map(|t| (*t).clone()).collect::<Vec<_>>(),
            );

            info!(
                total = tasks.len(),
                already_done = already_done.len(),
                to_execute = sorted.len(),
                "Task queue initialized"
            );

            ctx.state.set(STATE_TASK_QUEUE, &sorted);
            ctx.state.set(STATE_DONE_IDS, &already_done);
            ctx.state.set::<Vec<String>>(STATE_FAILED_IDS, &vec![]);
            ctx.state.set(STATE_INITIALIZED, &true);

            ctx.emit(AutomatonEvent::LogLine {
                message: format!(
                    "Dev loop ready: {} tasks to execute ({} already done)",
                    sorted.len(),
                    already_done.len()
                ),
            });

            return Ok(TickOutcome::Continue);
        }

        // ==================================================================
        // 1. Pick next task from queue
        // ==================================================================
        let mut queue: Vec<String> = ctx.state.get(STATE_TASK_QUEUE).unwrap_or_default();
        let done_ids: Vec<String> = ctx.state.get(STATE_DONE_IDS).unwrap_or_default();
        let done_set: HashSet<&str> = done_ids.iter().map(|s| s.as_str()).collect();

        if queue.is_empty() {
            // Check for retries before finishing
            if self.try_retry_failed(ctx, project_id).await? {
                return Ok(TickOutcome::Continue);
            }
            info!("Task queue empty, finishing loop");
            return self.finish(ctx).await;
        }

        let task_id = queue.remove(0);
        ctx.state.set(STATE_TASK_QUEUE, &queue);

        // Fetch the task details
        let task = match self.domain.get_task(&task_id, None).await {
            Ok(t) => t,
            Err(e) => {
                warn!(task_id = %task_id, error = %e, "Failed to fetch task, skipping");
                return Ok(TickOutcome::Continue);
            }
        };

        // Check dependencies are satisfied
        if !task.dependencies.is_empty()
            && !task.dependencies.iter().all(|dep| done_set.contains(dep.as_str()))
        {
            // Dependencies not met — push to back of queue
            info!(task_id = %task.id, title = %task.title, "Dependencies not yet met, deferring");
            let mut queue: Vec<String> = ctx.state.get(STATE_TASK_QUEUE).unwrap_or_default();
            queue.push(task.id.clone());
            ctx.state.set(STATE_TASK_QUEUE, &queue);
            return Ok(TickOutcome::Continue);
        }

        // ==================================================================
        // 2. Transition to in_progress and execute
        // ==================================================================
        info!(task_id = %task.id, title = %task.title, "Starting task");

        if task.status == "pending" {
            if let Err(e) = self.domain.transition_task(&task.id, "ready", None).await {
                warn!(task_id = %task.id, error = %e, "Failed to transition task to ready");
            }
        }
        if let Err(e) = self.domain.transition_task(&task.id, "in_progress", None).await {
            warn!(task_id = %task.id, error = %e, "Failed to transition task to in_progress (continuing anyway)");
        }

        ctx.emit(AutomatonEvent::TaskStarted {
            task_id: task.id.clone(),
            task_title: task.title.clone(),
        });

        let result = self.execute_task(ctx, &cfg, &task).await;

        // ==================================================================
        // 3. Process result
        // ==================================================================
        match result {
            Ok(exec) => {
                if let Err(e) = self.domain.transition_task(&task.id, "done", None).await {
                    warn!(task_id = %task.id, error = %e, "Failed to sync task done status to backend");
                }

                let mut done_ids: Vec<String> =
                    ctx.state.get(STATE_DONE_IDS).unwrap_or_default();
                done_ids.push(task.id.clone());
                ctx.state.set(STATE_DONE_IDS, &done_ids);

                let completed: u32 = ctx.state.get(STATE_COMPLETED_COUNT).unwrap_or(0) + 1;
                ctx.state.set(STATE_COMPLETED_COUNT, &completed);

                let mut work_log: Vec<String> =
                    ctx.state.get(STATE_WORK_LOG).unwrap_or_default();
                work_log.push(format!(
                    "Task (completed): {}\nNotes: {}",
                    task.title, exec.notes
                ));
                ctx.state.set(STATE_WORK_LOG, &work_log);

                ctx.emit(AutomatonEvent::TaskCompleted {
                    task_id: task.id.clone(),
                    summary: exec.notes,
                });
                ctx.emit(AutomatonEvent::TokenUsage {
                    input_tokens: exec.input_tokens,
                    output_tokens: exec.output_tokens,
                });

                commit_and_push(ctx, &task.id).await;

                info!(task_id = %task.id, title = %task.title, "Task completed successfully");
            }
            Err(e) => {
                warn!(task_id = %task.id, error = %e, "Task execution failed");

                if let Err(te) = self.domain.transition_task(&task.id, "failed", None).await {
                    warn!(task_id = %task.id, error = %te, "Failed to sync task failed status to backend");
                }

                let mut failed_ids: Vec<String> =
                    ctx.state.get(STATE_FAILED_IDS).unwrap_or_default();
                failed_ids.push(task.id.clone());
                ctx.state.set(STATE_FAILED_IDS, &failed_ids);

                let mut failure_reasons: HashMap<String, String> =
                    ctx.state.get(STATE_FAILURE_REASONS).unwrap_or_default();
                failure_reasons.insert(task.id.clone(), e.to_string());
                ctx.state.set(STATE_FAILURE_REASONS, &failure_reasons);

                let failed: u32 = ctx.state.get(STATE_FAILED_COUNT).unwrap_or(0) + 1;
                ctx.state.set(STATE_FAILED_COUNT, &failed);

                let mut work_log: Vec<String> =
                    ctx.state.get(STATE_WORK_LOG).unwrap_or_default();
                work_log.push(format!("Task (failed): {}\nReason: {e}", task.title));
                ctx.state.set(STATE_WORK_LOG, &work_log);

                ctx.emit(AutomatonEvent::TaskFailed {
                    task_id: task.id.clone(),
                    reason: e.to_string(),
                });
            }
        }

        Ok(TickOutcome::Continue)
    }

    async fn on_stop(&self, ctx: &TickContext) -> Result<(), AutomatonError> {
        let already_finished: bool = ctx.state.get(STATE_LOOP_FINISHED).unwrap_or(false);
        if !already_finished {
            let completed: u32 = ctx.state.get(STATE_COMPLETED_COUNT).unwrap_or(0);
            let failed: u32 = ctx.state.get(STATE_FAILED_COUNT).unwrap_or(0);

            ctx.emit(AutomatonEvent::LoopFinished {
                outcome: "stopped".into(),
                completed_count: completed,
                failed_count: failed,
            });
        }

        Ok(())
    }
}

impl DevLoopAutomaton {
    async fn execute_task(
        &self,
        ctx: &TickContext,
        cfg: &DevLoopConfig,
        task: &TaskDescriptor,
    ) -> Result<TaskExecutionResult, AutomatonError> {
        let project = self
            .domain
            .get_project(&cfg.project_id, None)
            .await
            .map_err(|e| AutomatonError::DomainApi(e.to_string()))?;

        let spec = self
            .domain
            .get_spec(&task.spec_id, None)
            .await
            .map_err(|e| AutomatonError::DomainApi(e.to_string()))?;

        if let Some(shell_cmd) = extract_shell_command(task) {
            let workspace = ctx
                .workspace_root
                .as_deref()
                .unwrap_or(std::path::Path::new(&project.path));
            return self
                .runner
                .execute_shell_task(
                    &ShellTaskParams {
                        command: &shell_cmd,
                        project_root: workspace,
                    },
                    None,
                )
                .await
                .map_err(|e| AutomatonError::AgentExecution(e.to_string()));
        }

        let effective_path = ctx
            .workspace_root
            .as_ref()
            .map(|p| p.to_string_lossy().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| project.path.clone());

        let project_info = ProjectInfo {
            name: &project.name,
            description: project.description.as_deref().unwrap_or(""),
            folder_path: &effective_path,
            build_command: project.build_command.as_deref(),
            test_command: project.test_command.as_deref(),
        };
        let spec_info = SpecInfo {
            title: &spec.title,
            markdown_contents: &spec.content,
        };
        let failure_reasons: HashMap<String, String> =
            ctx.state.get(STATE_FAILURE_REASONS).unwrap_or_default();
        let prior_failure = failure_reasons.get(&task.id).cloned().unwrap_or_default();
        let task_info = TaskInfo {
            title: &task.title,
            description: &task.description,
            execution_notes: &prior_failure,
            files_changed: &[],
        };
        let session_info = SessionInfo {
            summary_of_previous_context: "",
        };
        let work_log: Vec<String> = ctx.state.get(STATE_WORK_LOG).unwrap_or_default();
        let tools = self.catalog.tools_for_profile(ToolProfile::Engine);

        let params = AgenticTaskParams {
            project: &project_info,
            spec: &spec_info,
            task: &task_info,
            session: &session_info,
            agent: None,
            work_log: &work_log,
            completed_deps: &[],
            workspace_map: "",
            codebase_snapshot: "",
            type_defs_context: "",
            dep_api_context: "",
            member_count: 1,
            tools,
        };

        let cancel = ctx.cancellation_token().clone();

        let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel();
        let automaton_tx = ctx.event_tx.clone();
        tokio::spawn(async move {
            while let Some(evt) = event_rx.recv().await {
                forward_agent_event(&automaton_tx, evt);
            }
        });

        let inner_executor: Arc<dyn aura_agent::types::AgentToolExecutor> = self
            .tool_executor
            .clone()
            .unwrap_or_else(|| Arc::new(NoOpToolExecutor));
        let executor = aura_agent::task_executor::TaskToolExecutor {
            inner: inner_executor,
            project_folder: effective_path.clone(),
            build_command: project.build_command.clone(),
            task_context: String::new(),
            tracked_file_ops: Default::default(),
            notes: Default::default(),
            follow_ups: Default::default(),
            stub_fix_attempts: Default::default(),
            task_phase: Arc::new(tokio::sync::Mutex::new(
                aura_agent::planning::TaskPhase::Exploring,
            )),
            self_review: Default::default(),
            event_tx: Some(event_tx.clone()),
            no_changes_needed: Default::default(),
            recent_tool_outcomes: Default::default(),
        };

        let result = self
            .runner
            .execute_task(
                self.provider.as_ref(),
                &executor,
                &params,
                Some(event_tx),
                Some(cancel),
            )
            .await;

        match result {
            Ok(mut exec) => {
                executor.merge_into_result(&mut exec).await;
                if exec.file_ops.is_empty() && !exec.no_changes_needed {
                    let msg = if exec.reached_implementing {
                        "task reached implementation phase but no file operations completed \
                         — likely truncated by max_tokens or interrupted. \
                         On retry, use smaller incremental edits (one file per turn)."
                    } else {
                        "task completed without any file operations — completion not verified"
                    };
                    Err(AutomatonError::AgentExecution(msg.into()))
                } else {
                    Ok(exec)
                }
            }
            Err(e) => Err(AutomatonError::AgentExecution(e.to_string())),
        }
    }

    async fn try_retry_failed(
        &self,
        ctx: &mut TickContext,
        _project_id: &str,
    ) -> Result<bool, AutomatonError> {
        let failed_ids: Vec<String> = ctx.state.get(STATE_FAILED_IDS).unwrap_or_default();
        if failed_ids.is_empty() {
            return Ok(false);
        }

        let mut retry_counts: HashMap<String, u32> =
            ctx.state.get(STATE_RETRY_COUNTS).unwrap_or_default();

        let retryable: Vec<String> = failed_ids
            .iter()
            .filter(|id| *retry_counts.get(*id).unwrap_or(&0) < MAX_RETRIES_PER_TASK)
            .cloned()
            .collect();

        if retryable.is_empty() {
            return Ok(false);
        }

        let mut queue: Vec<String> = ctx.state.get(STATE_TASK_QUEUE).unwrap_or_default();
        let new_failed: Vec<String> = failed_ids
            .iter()
            .filter(|id| !retryable.contains(id))
            .cloned()
            .collect();

        for id in &retryable {
            let count = retry_counts.entry(id.clone()).or_insert(0);
            *count += 1;
            info!(task_id = %id, attempt = *count, "Retrying failed task");

            if let Err(e) = self
                .domain
                .transition_task(id, "ready", None)
                .await
            {
                warn!(task_id = %id, error = %e, "Failed to sync retry status to backend");
            }

            queue.push(id.clone());

            ctx.emit(AutomatonEvent::TaskRetrying {
                task_id: id.clone(),
                attempt: *count,
                reason: "automatic retry after failure".into(),
            });
        }

        ctx.state.set(STATE_TASK_QUEUE, &queue);
        ctx.state.set(STATE_FAILED_IDS, &new_failed);
        ctx.state.set(STATE_RETRY_COUNTS, &retry_counts);
        Ok(true)
    }

    async fn finish(&self, ctx: &mut TickContext) -> Result<TickOutcome, AutomatonError> {
        let completed: u32 = ctx.state.get(STATE_COMPLETED_COUNT).unwrap_or(0);
        let failed: u32 = ctx.state.get(STATE_FAILED_COUNT).unwrap_or(0);

        let outcome = if failed > 0 {
            "all_tasks_blocked"
        } else {
            "all_tasks_complete"
        };

        info!(outcome, completed, failed, "Dev loop finished");

        ctx.emit(AutomatonEvent::LoopFinished {
            outcome: outcome.into(),
            completed_count: completed,
            failed_count: failed,
        });
        ctx.state.set(STATE_LOOP_FINISHED, &true);

        Ok(TickOutcome::Done)
    }
}

pub(crate) fn extract_shell_command(task: &TaskDescriptor) -> Option<String> {
    let title_lower = task.title.to_lowercase();
    if title_lower.starts_with("run:") || title_lower.starts_with("shell:") {
        let cmd = task.title.splitn(2, ':').nth(1)?.trim().to_string();
        if !cmd.is_empty() {
            return Some(cmd);
        }
    }
    None
}

pub(crate) fn forward_agent_event(
    tx: &tokio::sync::mpsc::UnboundedSender<AutomatonEvent>,
    evt: aura_agent::events::AgentLoopEvent,
) {
    use aura_agent::events::AgentLoopEvent;
    let automaton_event = match evt {
        AgentLoopEvent::TextDelta(d) => AutomatonEvent::TextDelta { delta: d },
        AgentLoopEvent::ThinkingDelta(d) => AutomatonEvent::ThinkingDelta { delta: d },
        AgentLoopEvent::ToolStart { id, name } => AutomatonEvent::ToolCallStarted { id, name },
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

/// Commit staged changes and push to the Orbit remote if the automaton config
/// includes `git_repo_url`. Called after each successful task completion.
pub(super) async fn commit_and_push(ctx: &mut TickContext, task_id: &str) {
    let workspace = match ctx.workspace_root.as_ref() {
        Some(ws) => ws.to_string_lossy().to_string(),
        None => return,
    };

    if !aura_agent::git::is_git_repo(&workspace) {
        return;
    }

    let commit_msg = format!("task({}): completed", task_id);
    let sha = match aura_agent::git::git_commit(&workspace, &commit_msg).await {
        Ok(Some(sha)) => sha,
        Ok(None) => {
            ctx.emit(AutomatonEvent::GitCommitFailed {
                task_id: task_id.to_string(),
                reason: "No changes to commit".to_string(),
            });
            return;
        }
        Err(e) => {
            warn!(task_id, error = %e, "auto-commit after task completion failed");
            ctx.emit(AutomatonEvent::GitCommitFailed {
                task_id: task_id.to_string(),
                reason: format!("Commit failed: {e}"),
            });
            return;
        }
    };

    ctx.emit(AutomatonEvent::GitCommitted {
        task_id: task_id.to_string(),
        commit_sha: sha,
    });

    let git_repo_url = ctx.config.get("git_repo_url").and_then(|v| v.as_str());
    let git_branch = ctx
        .config
        .get("git_branch")
        .and_then(|v| v.as_str())
        .unwrap_or("main");
    let auth_token = ctx.config.get("auth_token").and_then(|v| v.as_str());

    if let (Some(repo_url), Some(jwt)) = (git_repo_url, auth_token) {
        match aura_agent::git::git_push(&workspace, repo_url, git_branch, jwt).await {
            Ok(commits) => {
                let commit_values: Vec<serde_json::Value> = commits
                    .iter()
                    .map(|c| serde_json::json!({"sha": c.sha, "message": c.message}))
                    .collect();
                ctx.emit(AutomatonEvent::GitPushed {
                    task_id: task_id.to_string(),
                    repo: repo_url.to_string(),
                    branch: git_branch.to_string(),
                    commits: commit_values,
                });
                info!(task_id, branch = git_branch, "auto-pushed to orbit");
            }
            Err(e) => {
                warn!(task_id, error = %e, "auto-push to orbit failed");
                ctx.emit(AutomatonEvent::GitPushFailed {
                    task_id: task_id.to_string(),
                    reason: format!("Push failed: {e}"),
                });
            }
        }
    }
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
