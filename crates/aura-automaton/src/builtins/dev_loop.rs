//! Dev-loop automaton – the core continuous task-execution loop.
//!
//! Replaces `DevLoopEngine::run_loop()` from `aura-app`. Each tick claims a
//! task via `DomainApi`, runs it through `AgentRunner`, processes the outcome,
//! and decides whether to continue, retry, or finish.

use std::sync::Arc;

use tracing::{error, info, warn};

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

const STATE_PROJECT_ID: &str = "project_id";
const STATE_AGENT_INSTANCE_ID: &str = "agent_instance_id";
const STATE_SESSION_ID: &str = "session_id";
const STATE_COMPLETED_COUNT: &str = "completed_count";
const STATE_FAILED_COUNT: &str = "failed_count";
const STATE_WORK_LOG: &str = "work_log";
const STATE_RETRY_COUNTS: &str = "retry_counts";
const STATE_LOOP_FINISHED: &str = "loop_finished";

const MAX_RETRIES_PER_TASK: u32 = 2;

/// Configuration extracted from `TickContext::config`.
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

    /// Attach a real tool executor for filesystem/command operations.
    /// Without this, the agent cannot perform any file or command operations.
    #[must_use]
    pub fn with_tool_executor(
        mut self,
        executor: Arc<dyn aura_agent::types::AgentToolExecutor>,
    ) -> Self {
        self.tool_executor = Some(executor);
        self
    }
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

        match self
            .domain
            .create_session(aura_tools::domain_tools::CreateSessionParams {
                instance_id: cfg.agent_instance_id.clone(),
                project_id: cfg.project_id.clone(),
                model: Some(cfg.model.clone()),
            })
            .await
        {
            Ok(session) => {
                ctx.emit(AutomatonEvent::LogLine {
                    message: format!(
                        "session {} created for project {}",
                        session.id, cfg.project_id
                    ),
                });
            }
            Err(e) => {
                warn!(project_id = %cfg.project_id, error = %e, "session creation unavailable, proceeding without");
                ctx.emit(AutomatonEvent::LogLine {
                    message: format!("dev loop starting for project {} (no session)", cfg.project_id),
                });
            }
        }

        Ok(())
    }

    async fn tick(&self, ctx: &mut TickContext) -> Result<TickOutcome, AutomatonError> {
        if ctx.is_cancelled() {
            return Ok(TickOutcome::Done);
        }

        let cfg = DevLoopConfig::from_json(&ctx.config)?;
        let project_id = ctx
            .state
            .get::<String>(STATE_PROJECT_ID)
            .unwrap_or(cfg.project_id.clone());
        let agent_id = ctx
            .state
            .get::<String>(STATE_AGENT_INSTANCE_ID)
            .unwrap_or(cfg.agent_instance_id.clone());

        // ------------------------------------------------------------------
        // 1. Claim next task
        // ------------------------------------------------------------------
        let task = match self.domain.claim_next_task(&project_id, &agent_id, None).await {
            Ok(Some(t)) => t,
            Ok(None) => {
                if self.try_retry_failed(ctx, &project_id).await? {
                    return Ok(TickOutcome::Continue);
                }
                return self.finish(ctx).await;
            }
            Err(e) => {
                error!(error = %e, "claim_next_task failed");
                return Err(AutomatonError::DomainApi(e.to_string()));
            }
        };

        ctx.emit(AutomatonEvent::TaskStarted {
            task_id: task.id.clone(),
            task_title: task.title.clone(),
        });

        // ------------------------------------------------------------------
        // 2. Execute task
        // ------------------------------------------------------------------
        let result = self.execute_task(ctx, &cfg, &task).await;

        // ------------------------------------------------------------------
        // 3. Process result
        // ------------------------------------------------------------------
        match result {
            Ok(exec) => {
                self.domain
                    .transition_task(&task.id, "done", None)
                    .await
                    .map_err(|e| AutomatonError::DomainApi(e.to_string()))?;

                let completed: u32 = ctx.state.get(STATE_COMPLETED_COUNT).unwrap_or(0) + 1;
                ctx.state.set(STATE_COMPLETED_COUNT, &completed);

                let mut work_log: Vec<String> = ctx.state.get(STATE_WORK_LOG).unwrap_or_default();
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
            }
            Err(e) => {
                warn!(task_id = %task.id, error = %e, "task execution failed");

                if let Err(e) = self.domain.transition_task(&task.id, "failed", None).await {
                    warn!(task_id = %task.id, error = %e, "failed to transition task to failed status");
                }

                let failed: u32 = ctx.state.get(STATE_FAILED_COUNT).unwrap_or(0) + 1;
                ctx.state.set(STATE_FAILED_COUNT, &failed);

                let mut work_log: Vec<String> = ctx.state.get(STATE_WORK_LOG).unwrap_or_default();
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
        if self.tool_executor.is_none() {
            return Err(AutomatonError::InvalidConfig(
                "no tool executor configured — the agent cannot perform file or command operations".into(),
            ));
        }

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

        let project_info = ProjectInfo {
            name: &project.name,
            description: project.description.as_deref().unwrap_or(""),
            folder_path: &project.path,
            build_command: project.build_command.as_deref(),
            test_command: project.test_command.as_deref(),
        };
        let spec_info = SpecInfo {
            title: &spec.title,
            markdown_contents: &spec.content,
        };
        let task_info = TaskInfo {
            title: &task.title,
            description: &task.description,
            execution_notes: "",
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
            project_folder: project.path.clone(),
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
        };

        self.runner
            .execute_task(
                self.provider.as_ref(),
                &executor,
                &params,
                Some(event_tx),
                Some(cancel),
            )
            .await
            .map_err(|e| AutomatonError::AgentExecution(e.to_string()))
    }

    async fn try_retry_failed(
        &self,
        ctx: &mut TickContext,
        project_id: &str,
    ) -> Result<bool, AutomatonError> {
        let tasks = self
            .domain
            .list_tasks(project_id, None, None)
            .await
            .map_err(|e| AutomatonError::DomainApi(e.to_string()))?;

        let mut retry_counts: std::collections::HashMap<String, u32> =
            ctx.state.get(STATE_RETRY_COUNTS).unwrap_or_default();

        let retryable: Vec<&TaskDescriptor> = tasks
            .iter()
            .filter(|t| {
                t.status == "failed"
                    && *retry_counts.get(&t.id).unwrap_or(&0) < MAX_RETRIES_PER_TASK
            })
            .collect();

        if retryable.is_empty() {
            return Ok(false);
        }

        for t in &retryable {
            let count = retry_counts.entry(t.id.clone()).or_insert(0);
            *count += 1;
            info!(task_id = %t.id, title = %t.title, attempt = *count, "retrying failed task");

            self.domain
                .transition_task(&t.id, "ready", None)
                .await
                .map_err(|e| AutomatonError::DomainApi(e.to_string()))?;

            ctx.emit(AutomatonEvent::TaskRetrying {
                task_id: t.id.clone(),
                attempt: *count,
                reason: "automatic retry after failure".into(),
            });
        }

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

        ctx.emit(AutomatonEvent::LoopFinished {
            outcome: outcome.into(),
            completed_count: completed,
            failed_count: failed,
        });
        ctx.state.set(STATE_LOOP_FINISHED, &true);

        Ok(TickOutcome::Done)
    }
}

/// Check whether a task descriptor represents a shell-only command.
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

/// Minimal no-op executor used as the inner delegate when the real tool
/// execution is handled by the wrapping `TaskToolExecutor`.
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
