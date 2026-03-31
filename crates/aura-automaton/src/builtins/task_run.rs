//! Single-task runner automaton.
//!
//! Replaces `DevLoopEngine::run_single_task()` from `aura-app`. On-demand:
//! a single tick executes one task and returns `Done`.

use std::sync::Arc;

use tracing::{error, info, warn};

use aura_agent::agent_runner::{
    AgentRunner, AgentRunnerConfig, AgenticTaskParams, ShellTaskParams, TaskTrackingConfig,
};
use aura_agent::prompts::{ProjectInfo, SessionInfo, SpecInfo, TaskInfo};
use aura_reasoner::ModelProvider;
use aura_tools::catalog::{ToolCatalog, ToolProfile};
use aura_tools::domain_tools::DomainApi;

use super::dev_loop::commit_and_push;
use super::noop_executor::NoOpExecutor;
use crate::context::TickContext;
use crate::error::AutomatonError;
use crate::events::AutomatonEvent;
use crate::runtime::{Automaton, TickOutcome};
use crate::schedule::Schedule;

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

#[allow(clippy::struct_field_names)]
struct TaskRunConfig {
    project_id: String,
    task_id: String,
    // TODO: will be used when task sessions tag their agent instance
    #[allow(dead_code)]
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
    fn kind(&self) -> &'static str {
        "task-run"
    }

    fn default_schedule(&self) -> Schedule {
        Schedule::OnDemand
    }

    async fn tick(&self, ctx: &mut TickContext) -> Result<TickOutcome, AutomatonError> {
        if self.tool_executor.is_none() {
            return Err(AutomatonError::InvalidConfig(
                "no tool executor configured — the agent cannot perform file or command operations"
                    .into(),
            ));
        }

        let cfg = TaskRunConfig::from_json(&ctx.config)?;
        let (task, project, spec) = self.fetch_task_context(&cfg).await?;

        ctx.emit(AutomatonEvent::TaskStarted {
            task_id: task.id.clone(),
            task_title: task.title.clone(),
        });

        self.transition_to_in_progress(&task).await;

        if let Some(shell_cmd) = super::dev_loop::extract_shell_command(&task) {
            let result = self.run_shell_task(ctx, &project, &shell_cmd).await;
            return self.finalize_task(ctx, &task.id, &task.title, result).await;
        }

        let result = self.run_agentic_task(ctx, &project, &spec, &task).await;
        self.finalize_task(ctx, &task.id, &task.title, result).await
    }
}

impl TaskRunAutomaton {
    async fn fetch_task_context(
        &self,
        cfg: &TaskRunConfig,
    ) -> Result<
        (
            aura_tools::domain_tools::TaskDescriptor,
            aura_tools::domain_tools::ProjectDescriptor,
            aura_tools::domain_tools::SpecDescriptor,
        ),
        AutomatonError,
    > {
        let tasks = self
            .domain
            .list_tasks(&cfg.project_id, None, None)
            .await
            .map_err(|e| AutomatonError::DomainApi(e.to_string()))?;

        let task = tasks
            .iter()
            .find(|t| t.id == cfg.task_id)
            .ok_or_else(|| AutomatonError::DomainApi(format!("task {} not found", cfg.task_id)))?
            .clone();

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

        Ok((task, project, spec))
    }

    async fn transition_to_in_progress(&self, task: &aura_tools::domain_tools::TaskDescriptor) {
        if task.status == "pending" {
            let _ = self.domain.transition_task(&task.id, "ready", None).await;
        }
        if let Err(e) = self
            .domain
            .transition_task(&task.id, "in_progress", None)
            .await
        {
            warn!(task_id = %task.id, error = %e, "Failed to transition task to in_progress (continuing anyway)");
        }
    }

    async fn run_shell_task(
        &self,
        ctx: &TickContext,
        project: &aura_tools::domain_tools::ProjectDescriptor,
        shell_cmd: &str,
    ) -> Result<aura_agent::agent_runner::TaskExecutionResult, anyhow::Error> {
        let workspace = ctx
            .workspace_root
            .as_deref()
            .unwrap_or_else(|| std::path::Path::new(&project.path));
        self.runner
            .execute_shell_task(
                &ShellTaskParams {
                    command: shell_cmd,
                    project_root: workspace,
                },
                None,
            )
            .await
            .map_err(Into::into)
    }

    async fn run_agentic_task(
        &self,
        ctx: &TickContext,
        project: &aura_tools::domain_tools::ProjectDescriptor,
        spec: &aura_tools::domain_tools::SpecDescriptor,
        task: &aura_tools::domain_tools::TaskDescriptor,
    ) -> Result<aura_agent::agent_runner::TaskExecutionResult, anyhow::Error> {
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

        let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(1024);
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

        let tracking = TaskTrackingConfig {
            inner_executor,
            project_folder: effective_path.clone(),
            build_command: project.build_command.clone(),
        };

        let result = self
            .runner
            .execute_task_tracked(
                self.provider.as_ref(),
                tracking,
                &params,
                Some(event_tx),
                Some(cancel),
            )
            .await;

        match result {
            Ok(exec) => {
                if exec.file_ops.is_empty() && !exec.no_changes_needed {
                    let msg = if exec.reached_implementing {
                        "task reached implementation phase but no file operations completed \
                         — likely truncated by max_tokens or interrupted. \
                         On retry, use smaller incremental edits (one file per turn)."
                    } else {
                        "task completed without any file operations — completion not verified"
                    };
                    Err(anyhow::anyhow!("{msg}"))
                } else {
                    Ok(exec)
                }
            }
            Err(e) => Err(e.into()),
        }
    }

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
