//! Public entry-points that install automatons into the runtime.
//!
//! These are the methods `AutomatonController::start_dev_loop` /
//! `run_task` (via `mod.rs`) ultimately delegate to. They handle:
//!
//! 1. Per-project re-entrancy checks (only one dev loop per project).
//! 2. Tool/integration filtering (delegated to
//!    [`AutomatonBridge::prepare_installed_tools`] in [`super::build`]).
//! 3. Per-agent kernel construction (delegated to
//!    [`AutomatonBridge::build_kernel`] in [`super::build`]).
//! 4. Recording runtime capabilities for downstream debugging.
//! 5. Wiring the gateway domain so automaton-driven mutations land in
//!    the record log as `System::DomainMutation` (Invariant §2 / §8).
//! 6. Installing the automaton, recording the lifecycle event, and
//!    spawning the replay-aware event forwarder.

use std::path::PathBuf;
use std::sync::Arc;

use aura_agent::agent_runner::AgentRunnerConfig;
use aura_agent::{KernelDomainGateway, KernelModelGateway, KernelToolGateway};
use aura_agent_kernel::Kernel;
use aura_core_protocol::AgentPersona;
use aura_core_types::AgentPermissions;
use aura_engine_automaton::{DevLoopAutomaton, TaskRunAutomaton};
use aura_exec_tools::catalog::ToolCatalog;
use aura_exec_tools::domain_tools::DomainApi;
use tracing::{info, warn};

use crate::capabilities;
use aura_core_protocol::installed_integration_to_core;

use super::build::KernelParams;
use super::kickoff::{require_model, Kickoff, KickoffTools, TaskSpec};
use super::{AutomatonBridge, ProjectHandle};

/// Bootstrapping artefacts shared by every automaton kickoff path
/// (`start_dev_loop_with_capabilities`, `run_task_with_capabilities`,
/// future entry-points). Populated by [`AutomatonBridge::prepare_automaton_run`].
struct AutomatonRunContext {
    kernel: Arc<Kernel>,
    /// Held as the concrete [`KernelModelGateway`] type (rather than
    /// `Arc<dyn ModelProvider>`) so the automaton constructors'
    /// sealed [`aura_agent::RecordingModelProvider`] bound is satisfied
    /// at the call site. This makes the §1 "Sole External Gateway"
    /// invariant a structural property of the dispatch path: nothing
    /// in this file can hand a raw HTTP provider to an automaton.
    model_gw: Arc<KernelModelGateway>,
    tool_gw: Arc<dyn aura_agent::AgentToolExecutor>,
    gateway_domain: Arc<dyn DomainApi>,
    runner_config: AgentRunnerConfig,
    catalog: Arc<ToolCatalog>,
    effective_workspace: Option<PathBuf>,
}

/// What differs between the dev-loop and task-run kickoffs beyond
/// their automaton type.
///
/// The labels are spliced into the same error templates the original
/// two functions used, preserving log/observability strings
/// character-for-character.
#[derive(Clone, Copy)]
struct AutomatonKind {
    /// Inserted into `failed to build {} kernel`. Uses different wording
    /// per kind: `"dev loop"` (so the message ends `... dev loop kernel`)
    /// vs `"task runtime"` (`... task runtime kernel`).
    kernel: &'static str,
    /// Inserted into `failed to record {} runtime capabilities`.
    /// `"dev loop"` for the long-running dev loop, plain `"task"` for
    /// one-shot task runs.
    capabilities: &'static str,
    /// Inserted into `failed to install {} automaton`.
    install: &'static str,
    /// Request kind registered with the agent's identity. The
    /// dev-loop's first model call advertises `DevLoopBootstrap`; the
    /// loop self-promotes to `DevLoopContinuation` on subsequent
    /// iterations via the automaton-side runner config, so the registry
    /// only needs to seed the bootstrap kind for any pre-loop scheduling
    /// wakeups. Task runs use the chat request-kind on the worker
    /// fan-out path because they don't enter the dev-loop
    /// self-promotion cycle.
    request_kind: aura_model::ModelRequestKind,
}

const DEV_LOOP: AutomatonKind = AutomatonKind {
    kernel: "dev loop",
    capabilities: "dev loop",
    install: "dev-loop",
    request_kind: aura_model::ModelRequestKind::DevLoopBootstrap,
};

const TASK_RUN: AutomatonKind = AutomatonKind {
    kernel: "task runtime",
    capabilities: "task",
    install: "task-run",
    request_kind: aura_model::ModelRequestKind::Chat,
};

impl AutomatonBridge {
    /// Run the bootstrap pipeline shared by every automaton kickoff
    /// path: build the per-project kernel, record its runtime
    /// capabilities, then assemble the model/tool/domain gateways and
    /// per-run config the automaton constructors require, with the
    /// kickoff's execution profile applied.
    ///
    /// `kind` controls only the operator-facing error strings so the
    /// original `failed to build dev loop kernel` /
    /// `failed to build task runtime kernel` (and the matching
    /// capabilities error) messages remain identical to the pre-refactor
    /// implementation.
    async fn prepare_automaton_run(
        &self,
        kickoff: &Kickoff<'_>,
        tools: KickoffTools,
        kind: AutomatonKind,
    ) -> Result<AutomatonRunContext, String> {
        let auth_token = kickoff.auth_token.as_deref();
        let domain = self.domain_with_jwt(auth_token);
        let effective_workspace = tools.workspace_root.clone();
        let installed_integrations = tools
            .installed_integrations
            .unwrap_or_default()
            .into_iter()
            .map(installed_integration_to_core)
            .collect::<Vec<_>>();
        let installed_tools =
            Self::prepare_installed_tools(tools.installed_tools, &installed_integrations);

        let kernel = self
            .build_kernel(KernelParams {
                domain: domain.clone(),
                auth_token,
                project_id: Some(kickoff.project_id),
                workspace: effective_workspace
                    .as_deref()
                    .unwrap_or_else(|| std::path::Path::new(".")),
                use_workspace_base_as_root: effective_workspace.is_some(),
                installed_tools: &installed_tools,
                installed_integrations: &installed_integrations,
                agent_permissions: tools.agent_permissions,
            })
            .map_err(|e| format!("failed to build {} kernel: {e}", kind.kernel))?;
        if let Err(e) = capabilities::record_runtime_capabilities(
            &kernel,
            "automaton",
            None,
            &installed_tools,
            &installed_integrations,
        )
        .await
        {
            return Err(format!(
                "failed to record {} runtime capabilities: {e}",
                kind.capabilities
            ));
        }
        let mut ctx = self.run_context(
            kickoff,
            kernel,
            domain,
            &installed_tools,
            effective_workspace,
        );
        apply_execution_profile(&mut ctx.runner_config, kickoff.execution_profile.as_ref())?;
        Ok(ctx)
    }

    /// The gateways, runner config and catalog an automaton runs with
    /// on `kernel`.
    fn run_context(
        &self,
        kickoff: &Kickoff<'_>,
        kernel: Arc<Kernel>,
        domain: Arc<dyn DomainApi>,
        installed_tools: &[aura_core_types::InstalledToolDefinition],
        effective_workspace: Option<PathBuf>,
    ) -> AutomatonRunContext {
        let model_gw: Arc<KernelModelGateway> = Arc::new(KernelModelGateway::new(kernel.clone()));
        let tool_gw: Arc<dyn aura_agent::AgentToolExecutor> =
            Arc::new(KernelToolGateway::new(kernel.clone()));
        // Wrap the domain so mutations driven by automaton orchestration
        // (not the LLM tool loop) route through `kernel.process_direct`
        // and produce `SystemKind::DomainMutation` record entries. The
        // raw `domain` is still used inside `build_kernel` for the
        // `DomainToolExecutor`, whose mutations are captured via
        // `ToolExecution` entries by the kernel itself.
        let gateway_domain: Arc<dyn DomainApi> =
            Arc::new(KernelDomainGateway::new(domain, kernel.clone()));
        let runner_config = self.build_runner_config(kickoff.identity());
        let catalog = Arc::new(self.catalog.with_installed_tools(
            aura_exec_tools::catalog::ToolProfile::Engine,
            installed_tools,
        ));
        AutomatonRunContext {
            kernel,
            model_gw,
            tool_gw,
            gateway_domain,
            runner_config,
            catalog,
            effective_workspace,
        }
    }

    /// Install `automaton` and wire up its agent: register its identity
    /// (before the lifecycle-event nudge, so the scheduler tick that runs
    /// as part of it picks the identity up), forward its events and
    /// apply the execution profile. Returns its id and handle.
    async fn install_automaton(
        &self,
        kickoff: &Kickoff<'_>,
        kind: AutomatonKind,
        automaton: Box<dyn aura_engine_automaton::Automaton>,
        config: serde_json::Value,
        workspace: Option<PathBuf>,
        agent_id: aura_core_types::AgentId,
    ) -> Result<(String, aura_engine_automaton::AutomatonHandle), String> {
        let (handle, event_rx) = self
            .runtime
            .install(automaton, config, workspace)
            .await
            .map_err(|e| format!("failed to install {} automaton: {e}", kind.install))?;

        let automaton_id = handle.id().as_str().to_string();
        self.register_automaton_identity(agent_id, kickoff.identity(), kind.request_kind);
        self.spawn_event_forwarder(automaton_id.clone(), event_rx);
        self.register_execution_profile(agent_id, kickoff.execution_profile.as_ref());
        Ok((automaton_id, handle))
    }

    /// Drop a finished or orphaned dev-loop handle for `project_id`, or
    /// refuse when a dev loop is still running there.
    fn clear_stale_dev_loop(&self, project_id: &str) -> Result<(), String> {
        let Some(entry) = self.project_handles.get(project_id) else {
            return Ok(());
        };
        let tracked = entry.value();
        if !tracked.handle.is_finished() && self.runtime.get_info(&tracked.automaton_id).is_some() {
            return Err(format!(
                "A dev loop is already running for project {project_id} (automaton_id: {})",
                tracked.automaton_id
            ));
        }
        if !tracked.handle.is_finished() {
            warn!(
                project_id,
                automaton_id = %tracked.automaton_id,
                "dropping stale dev-loop project handle with no matching runtime automaton"
            );
        }
        drop(entry);
        self.project_handles.remove(project_id);
        Ok(())
    }

    /// Commit the start's lifecycle event and run the first tick in the
    /// background, so the caller gets the run id at once.
    fn spawn_lifecycle_commit(
        &self,
        agent_id: aura_core_types::AgentId,
        automaton_id: &str,
        event: &str,
    ) {
        tokio::spawn(Self::commit_lifecycle_event(
            self.store.clone(),
            self.scheduler.clone(),
            agent_id,
            automaton_id.to_string(),
            event.to_string(),
        ));
    }

    #[allow(clippy::too_many_arguments)] // TODO(W4): collapse dev-loop kickoff args.
    pub async fn start_dev_loop_with_capabilities(
        &self,
        project_id: &str,
        workspace_root: Option<PathBuf>,
        auth_token: Option<String>,
        model: Option<String>,
        git_repo_url: Option<String>,
        git_branch: Option<String>,
        installed_tools: Option<Vec<aura_core_protocol::InstalledTool>>,
        installed_integrations: Option<Vec<aura_core_protocol::InstalledIntegration>>,
        agent_permissions: AgentPermissions,
        aura_org_id: Option<String>,
        aura_session_id: Option<String>,
        aura_agent_id: Option<String>,
        agent_persona: Option<AgentPersona>,
        agent_skills: Vec<String>,
        agent_system_prompt: Option<String>,
        execution_profile: Option<aura_core_protocol::ModelSelection>,
    ) -> Result<String, String> {
        // Hard-fail when the operator did not pin a model. The
        // pre-fix dev-loop path silently fell back to a build-time
        // constant (`claude-opus-4-6`) here, which is exactly the
        // regression the worker-identity work is closing. We surface
        // the gap loudly so the caller (typically aura-os's
        // `POST /v1/run`) sees the configuration mismatch instead of
        // routing eval traffic to an unintended model.
        let model = require_model(model.as_deref(), "dev loop start request")?;
        self.clear_stale_dev_loop(project_id)?;
        let kickoff = Kickoff {
            project_id,
            model,
            auth_token,
            git_repo_url,
            git_branch,
            aura_org_id,
            aura_session_id,
            aura_agent_id,
            agent_persona,
            agent_skills,
            agent_system_prompt,
            execution_profile,
        };
        let tools = KickoffTools {
            workspace_root,
            installed_tools,
            installed_integrations,
            agent_permissions,
        };
        self.launch_dev_loop(kickoff, tools).await
    }

    /// Install a dev loop for a validated `kickoff` and hand it to the
    /// scheduler.
    async fn launch_dev_loop(
        &self,
        kickoff: Kickoff<'_>,
        tools: KickoffTools,
    ) -> Result<String, String> {
        let project_id = kickoff.project_id;
        let ctx = self
            .prepare_automaton_run(&kickoff, tools, DEV_LOOP)
            .await?;
        let automaton = DevLoopAutomaton::new(
            ctx.gateway_domain,
            ctx.model_gw,
            ctx.runner_config,
            ctx.catalog,
        )
        .with_tool_executor(ctx.tool_gw);
        let agent_id = ctx.kernel.agent_id;
        let (automaton_id, handle) = self
            .install_automaton(
                &kickoff,
                DEV_LOOP,
                Box::new(automaton),
                kickoff.dev_loop_config(),
                ctx.effective_workspace,
                agent_id,
            )
            .await?;

        info!(project_id, automaton_id = %automaton_id, "Dev loop started");
        // Record the handle BEFORE the (now background) bootstrap tick so
        // a concurrent or retried start observes the running loop, and the
        // handle is never lost if the first iteration outlives the
        // request.
        self.project_handles.insert(
            project_id.to_string(),
            ProjectHandle {
                automaton_id: automaton_id.clone(),
                agent_id,
                handle,
            },
        );
        // Drive the lifecycle commit + first bootstrap iteration in the
        // background. The scheduler tick runs the dev loop's first full
        // agent turn, which routinely exceeds the gateway's 30s request
        // timeout; awaiting it here turned every successful start into a
        // 408. Returning the run id now hands the loop to the scheduler,
        // matching the non-blocking contract POST /v1/run advertises.
        self.spawn_lifecycle_commit(agent_id, &automaton_id, "start_dev_loop");
        Ok(automaton_id)
    }

    #[allow(clippy::too_many_arguments)] // TODO(W4): collapse task-runner args.
    pub async fn run_task_with_capabilities(
        &self,
        project_id: &str,
        task_id: &str,
        workspace_root: Option<PathBuf>,
        auth_token: Option<String>,
        model: Option<String>,
        git_repo_url: Option<String>,
        git_branch: Option<String>,
        installed_tools: Option<Vec<aura_core_protocol::InstalledTool>>,
        installed_integrations: Option<Vec<aura_core_protocol::InstalledIntegration>>,
        agent_permissions: AgentPermissions,
        prior_failure: Option<String>,
        work_log: Vec<String>,
        aura_org_id: Option<String>,
        aura_session_id: Option<String>,
        aura_agent_id: Option<String>,
        agent_persona: Option<AgentPersona>,
        agent_skills: Vec<String>,
        agent_system_prompt: Option<String>,
        execution_profile: Option<aura_core_protocol::ModelSelection>,
    ) -> Result<String, String> {
        // Mirror the dev-loop entry-point: refuse to start without an
        // explicit model. Same regression rationale.
        let model = require_model(model.as_deref(), "task run request")?;
        let kickoff = Kickoff {
            project_id,
            model,
            auth_token,
            git_repo_url,
            git_branch,
            aura_org_id,
            aura_session_id,
            aura_agent_id,
            agent_persona,
            agent_skills,
            agent_system_prompt,
            execution_profile,
        };
        let tools = KickoffTools {
            workspace_root,
            installed_tools,
            installed_integrations,
            agent_permissions,
        };
        let task = TaskSpec {
            task_id,
            prior_failure,
            work_log,
        };
        self.launch_task_run(kickoff, tools, task).await
    }

    /// Install a task run for a validated `kickoff` and hand it to the
    /// scheduler.
    async fn launch_task_run(
        &self,
        kickoff: Kickoff<'_>,
        tools: KickoffTools,
        task: TaskSpec<'_>,
    ) -> Result<String, String> {
        let ctx = self
            .prepare_automaton_run(&kickoff, tools, TASK_RUN)
            .await?;
        let automaton = TaskRunAutomaton::new(
            ctx.gateway_domain,
            ctx.model_gw,
            ctx.runner_config,
            ctx.catalog,
        )
        .with_tool_executor(ctx.tool_gw);
        let agent_id = ctx.kernel.agent_id;
        let (automaton_id, _handle) = self
            .install_automaton(
                &kickoff,
                TASK_RUN,
                Box::new(automaton),
                kickoff.task_config(&task),
                ctx.effective_workspace,
                agent_id,
            )
            .await?;

        let (project_id, task_id) = (kickoff.project_id, task.task_id);
        info!(project_id, task_id, automaton_id = %automaton_id, "Task execution started (non-blocking)");
        // Background the lifecycle commit + first tick so POST /v1/run
        // returns immediately instead of blocking on the task's first
        // full agent iteration past the gateway's 30s request timeout.
        self.spawn_lifecycle_commit(agent_id, &automaton_id, "start_task_run");
        Ok(automaton_id)
    }
}

/// Preserve explicit task/dev-loop limits and effort instead of dropping the
/// model selection at the HTTP-to-automaton boundary.
fn apply_execution_profile(
    config: &mut AgentRunnerConfig,
    profile: Option<&aura_core_protocol::ModelSelection>,
) -> Result<(), String> {
    let Some(profile) = profile else {
        return Ok(());
    };
    if let Some(turns) = profile.max_turns {
        if turns == 0 {
            return Err("max_turns must be positive".into());
        }
        config.max_agentic_iterations = turns as usize;
    }
    config.reasoning_effort = profile
        .reasoning_effort
        .and_then(|effort| aura_model::ThinkingEffort::from_wire(effort.as_wire()));
    Ok(())
}

#[cfg(test)]
mod execution_profile_tests;
