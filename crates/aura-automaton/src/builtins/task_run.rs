//! Single-task runner automaton.
//!
//! Replaces `DevLoopEngine::run_single_task()` from `aura-app`. On-demand:
//! a single tick executes one task and returns `Done`.

use std::sync::Arc;

use tracing::{error, info, warn};

use aura_agent::agent_runner::{
    AgentRunner, AgentRunnerConfig, AgenticTaskParams, ShellTaskParams,
};
use aura_agent::prompts::{ProjectInfo, SessionInfo, SpecInfo, TaskInfo};
use aura_reasoner::ModelProvider;
use aura_tools::catalog::{ToolCatalog, ToolProfile};
use aura_tools::domain_tools::DomainApi;

use crate::context::TickContext;
use crate::error::AutomatonError;
use crate::events::AutomatonEvent;
use crate::runtime::{Automaton, TickOutcome};
use crate::schedule::Schedule;

use super::dev_loop::commit_and_push;

pub struct TaskRunAutomaton {
    domain: Arc<dyn DomainApi>,
    provider: Arc<dyn ModelProvider>,
    runner: AgentRunner,
    catalog: Arc<ToolCatalog>,
    tool_executor: Option<Arc<dyn aura_agent::types::AgentToolExecutor>>,
}

impl TaskRunAutomaton {
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
    #[must_use]
    pub fn with_tool_executor(
        mut self,
        executor: Arc<dyn aura_agent::types::AgentToolExecutor>,
    ) -> Self {
        self.tool_executor = Some(executor);
        self
    }
}

struct TaskRunConfig {
    project_id: String,
    task_id: String,
    agent_instance_id: String,
}

impl TaskRunConfig {
    fn from_json(config: &serde_json::Value) -> Result<Self, AutomatonError> {
        let project_id = config
            .get("project_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AutomatonError::InvalidConfig("missing project_id".into()))?
            .to_string();
        let task_id = config
            .get("task_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AutomatonError::InvalidConfig("missing task_id".into()))?
            .to_string();
        let agent_instance_id = config
            .get("agent_instance_id")
            .and_then(|v| v.as_str())
            .unwrap_or("default")
            .to_string();
        Ok(Self {
            project_id,
            task_id,
            agent_instance_id,
        })
    }
}

#[async_trait::async_trait]
impl Automaton for TaskRunAutomaton {
    fn kind(&self) -> &str {
        "task-run"
    }

    fn default_schedule(&self) -> Schedule {
        Schedule::OnDemand
    }

    async fn tick(&self, ctx: &mut TickContext) -> Result<TickOutcome, AutomatonError> {
        if self.tool_executor.is_none() {
            return Err(AutomatonError::InvalidConfig(
                "no tool executor configured — the agent cannot perform file or command operations".into(),
            ));
        }

        let cfg = TaskRunConfig::from_json(&ctx.config)?;

        // ------------------------------------------------------------------
        // 1. Fetch task, project, spec
        // ------------------------------------------------------------------
        let tasks = self
            .domain
            .list_tasks(&cfg.project_id, None, None)
            .await
            .map_err(|e| AutomatonError::DomainApi(e.to_string()))?;

        let task = tasks
            .iter()
            .find(|t| t.id == cfg.task_id)
            .ok_or_else(|| AutomatonError::DomainApi(format!("task {} not found", cfg.task_id)))?;

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

        ctx.emit(AutomatonEvent::TaskStarted {
            task_id: task.id.clone(),
            task_title: task.title.clone(),
        });

        // ------------------------------------------------------------------
        // 2. Transition task to in-progress (pending → ready → in_progress)
        // ------------------------------------------------------------------
        if task.status == "pending" {
            let _ = self.domain.transition_task(&task.id, "ready", None).await;
        }
        if let Err(e) = self.domain.transition_task(&task.id, "in_progress", None).await {
            warn!(task_id = %task.id, error = %e, "Failed to transition task to in_progress (continuing anyway)");
        }

        // ------------------------------------------------------------------
        // 3. Execute
        // ------------------------------------------------------------------
        if let Some(shell_cmd) = super::dev_loop::extract_shell_command(task) {
            let workspace = ctx
                .workspace_root
                .as_deref()
                .unwrap_or(std::path::Path::new(&project.path));

            let result = self
                .runner
                .execute_shell_task(
                    &ShellTaskParams {
                        command: &shell_cmd,
                        project_root: workspace,
                    },
                    None,
                )
                .await;

            return self.finalize_task(ctx, &task.id, &task.title, result.map_err(Into::into)).await;
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
        let task_info = TaskInfo {
            title: &task.title,
            description: &task.description,
            execution_notes: "",
            files_changed: &[],
        };
        let session_info = SessionInfo {
            summary_of_previous_context: "",
        };
        let tools = self.catalog.tools_for_profile(ToolProfile::Engine);

        let params = AgenticTaskParams {
            project: &project_info,
            spec: &spec_info,
            task: &task_info,
            session: &session_info,
            agent: None,
            work_log: &[],
            completed_deps: &[],
            workspace_map: "",
            codebase_snapshot: "",
            type_defs_context: "",
            dep_api_context: "",
            member_count: 1,
            tools,
        };

        let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel();
        let automaton_tx = ctx.event_tx.clone();
        tokio::spawn(async move {
            while let Some(evt) = event_rx.recv().await {
                super::dev_loop::forward_agent_event(&automaton_tx, evt);
            }
        });

        let cancel = ctx.cancellation_token().clone();

        let inner_executor: Arc<dyn aura_agent::types::AgentToolExecutor> = self
            .tool_executor
            .clone()
            .unwrap_or_else(|| Arc::new(NoOpExecutor));
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

        let result = match result {
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
                    Err(anyhow::anyhow!("{}", msg))
                } else {
                    Ok(exec)
                }
            }
            Err(e) => Err(e.into()),
        };

        self.finalize_task(ctx, &task.id, &task.title, result).await
    }
}

impl TaskRunAutomaton {
    async fn finalize_task(
        &self,
        ctx: &mut TickContext,
        task_id: &str,
        _task_title: &str,
        result: Result<aura_agent::agent_runner::TaskExecutionResult, anyhow::Error>,
    ) -> Result<TickOutcome, AutomatonError> {
        match result {
            Ok(exec) => {
                self.domain
                    .transition_task(task_id, "done", None)
                    .await
                    .map_err(|e| AutomatonError::DomainApi(e.to_string()))?;

                info!(task_id, notes = %exec.notes, "task completed");

                ctx.emit(AutomatonEvent::TaskCompleted {
                    task_id: task_id.to_string(),
                    summary: exec.notes,
                });
                ctx.emit(AutomatonEvent::TokenUsage {
                    input_tokens: exec.input_tokens,
                    output_tokens: exec.output_tokens,
                });

                commit_and_push(ctx, task_id).await;
            }
            Err(e) => {
                error!(task_id, error = %e, "task execution failed");

                if let Err(e) = self.domain.transition_task(task_id, "failed", None).await {
                    warn!(task_id, error = %e, "failed to transition task to failed status");
                }

                ctx.emit(AutomatonEvent::TaskFailed {
                    task_id: task_id.to_string(),
                    reason: e.to_string(),
                });
            }
        }

        Ok(TickOutcome::Done)
    }
}

struct NoOpExecutor;

#[async_trait::async_trait]
impl aura_agent::types::AgentToolExecutor for NoOpExecutor {
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
