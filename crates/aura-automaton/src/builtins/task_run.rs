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

use super::dev_loop::{commit_and_push, validate_execution, TaskAggregate};
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
    /// Retry-warm-up: the reason text persisted on the previous
    /// attempt's `task_failed` record. Threaded into `TaskInfo
    /// ::execution_notes` so the model does not see a prompt-identical
    /// cold re-run on single-task retries. `None` on initial attempts
    /// and on dev-loop ticks (dev-loop derives its own notes via
    /// `STATE_FAILURE_REASONS` in `aura-app`).
    prior_failure: Option<String>,
    /// Retry-warm-up: recent work-log entries the caller wants the
    /// agent to re-see. Matches the shape
    /// `AgenticTaskParams::work_log` expects. Defaults to empty for
    /// initial attempts.
    work_log: Vec<String>,
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
        let prior_failure = config
            .get("prior_failure")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        let work_log = config
            .get("work_log")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        Ok(Self {
            project_id,
            task_id,
            agent_instance_id,
            prior_failure,
            work_log,
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

        let result = self
            .run_agentic_task(ctx, &project, &spec, &task, &cfg)
            .await;
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
    ) -> Result<aura_agent::agent_runner::TaskExecutionResult, AutomatonError> {
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
            .map_err(|e| AutomatonError::AgentExecution(e.to_string()))
    }

    async fn run_agentic_task(
        &self,
        ctx: &TickContext,
        project: &aura_tools::domain_tools::ProjectDescriptor,
        spec: &aura_tools::domain_tools::SpecDescriptor,
        task: &aura_tools::domain_tools::TaskDescriptor,
        cfg: &TaskRunConfig,
    ) -> Result<aura_agent::agent_runner::TaskExecutionResult, AutomatonError> {
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
            execution_notes: cfg.prior_failure.as_deref().unwrap_or(""),
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
            work_log: cfg.work_log.as_slice(),
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
            .await
            .map_err(|e| AutomatonError::AgentExecution(e.to_string()))?;

        validate_execution(result)
    }

    async fn finalize_task(
        &self,
        ctx: &mut TickContext,
        task_id: &str,
        _task_title: &str,
        result: Result<aura_agent::agent_runner::TaskExecutionResult, AutomatonError>,
    ) -> Result<TickOutcome, AutomatonError> {
        match result {
            Ok(exec) => {
                self.domain
                    .transition_task(task_id, "done", None)
                    .await
                    .map_err(|e| AutomatonError::DomainApi(e.to_string()))?;

                info!(task_id, notes = %exec.notes, "task completed");

                // Compute the DoD aggregate BEFORE moving `exec.notes`
                // into the `TaskCompleted` summary; after the move
                // `exec` can no longer be borrowed by the aggregate
                // builder. Same precheck contract as dev_loop/tick.rs.
                let aggregate = TaskAggregate::from_exec(&exec);

                ctx.emit(AutomatonEvent::TaskCompleted {
                    task_id: task_id.to_string(),
                    summary: exec.notes,
                });
                ctx.emit(AutomatonEvent::TokenUsage {
                    input_tokens: exec.input_tokens,
                    output_tokens: exec.output_tokens,
                });

                commit_and_push(ctx, self.tool_executor.as_ref(), task_id, &aggregate).await;
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

#[cfg(test)]
mod tests {
    use super::TaskRunConfig;
    use serde_json::json;

    #[test]
    fn from_json_defaults_prior_failure_and_work_log_to_empty() {
        let cfg = TaskRunConfig::from_json(&json!({
            "project_id": "proj-1",
            "task_id": "task-1",
        }))
        .expect("parse minimal config");
        assert_eq!(cfg.project_id, "proj-1");
        assert_eq!(cfg.task_id, "task-1");
        assert_eq!(cfg.agent_instance_id, "default");
        assert!(cfg.prior_failure.is_none());
        assert!(cfg.work_log.is_empty());
    }

    #[test]
    fn from_json_treats_empty_prior_failure_as_none() {
        // Dev-loop / initial attempts send `""` rather than omitting
        // the field. Treat it the same as absent so callers don't
        // have to branch.
        let cfg = TaskRunConfig::from_json(&json!({
            "project_id": "proj-1",
            "task_id": "task-1",
            "prior_failure": "",
        }))
        .expect("parse empty prior_failure");
        assert!(cfg.prior_failure.is_none());
    }

    #[test]
    fn from_json_parses_prior_failure_and_work_log() {
        let cfg = TaskRunConfig::from_json(&json!({
            "project_id": "proj-1",
            "task_id": "task-1",
            "agent_instance_id": "inst-7",
            "prior_failure": "stream terminated with error: Internal server error",
            "work_log": [
                "attempt 1: edited src/lib.rs",
                "attempt 1: tests failed with type error",
            ],
        }))
        .expect("parse retry-warm config");
        assert_eq!(cfg.agent_instance_id, "inst-7");
        assert_eq!(
            cfg.prior_failure.as_deref(),
            Some("stream terminated with error: Internal server error")
        );
        assert_eq!(cfg.work_log.len(), 2);
        assert_eq!(cfg.work_log[0], "attempt 1: edited src/lib.rs");
    }

    #[test]
    fn from_json_skips_non_string_work_log_entries() {
        // Forward-compat: if a newer server shape sends structured
        // work_log entries, drop them silently instead of erroring.
        let cfg = TaskRunConfig::from_json(&json!({
            "project_id": "proj-1",
            "task_id": "task-1",
            "work_log": [
                "ok string",
                {"structured": "ignored"},
                42,
            ],
        }))
        .expect("parse mixed work_log");
        assert_eq!(cfg.work_log, vec!["ok string".to_string()]);
    }
}
