//! Per-session kernel construction and agent-host registration.

use super::init::session_user_defaults;
use crate::gateway::session::cross_agent_hook::{AuraServerAgentHook, AuraServerSpawnHook};
use crate::gateway::session::{Session, WsContext};
use crate::protocol::OutboundMessage;
use aura_agent_kernel::{Kernel, KernelConfig, PolicyConfig};
use aura_core_types::{is_effectively_full_access, AgentToolPermissions, UserToolDefaults};
use aura_engine::{executor, AgentEnv, AgentHooks, RootAgent};
use aura_exec_tools::domain_tools::DomainToolExecutor;
use aura_exec_tools::{CommandOutputHook, ToolResolver};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Sends what a foreground command prints onto the turn's stream.
/// Called from the threads reading the command, so a full stream drops
/// the piece rather than stalling the command.
struct WireCommandOutput {
    outbound: mpsc::Sender<OutboundMessage>,
}

impl CommandOutputHook for WireCommandOutput {
    fn output(&self, tool_use_id: &str, chunk: &[u8]) {
        let chunk = String::from_utf8_lossy(chunk);
        if chunk.is_empty() {
            return;
        }
        let _ = self.outbound.try_send(OutboundMessage::ToolOutput(
            crate::protocol::ToolOutputMsg {
                id: tool_use_id.to_string(),
                chunk: chunk.into_owned(),
            },
        ));
    }
}

pub(super) fn resolve_session_workspace(session: &Session) -> (std::path::PathBuf, bool) {
    if let Some(ref project_path) = session.project_path {
        return (project_path.clone(), true);
    }

    if session.workspace != session.workspace_base {
        return (session.workspace.clone(), true);
    }

    (session.workspace.clone(), false)
}

pub(super) fn session_scoped_tool_config(
    base: &aura_exec_tools::ToolConfig,
    user_default: &UserToolDefaults,
    agent_override: Option<&AgentToolPermissions>,
) -> aura_exec_tools::ToolConfig {
    let mut config = base.clone();
    config.command.bypass_allowlists = base.command.allow_unrestricted_full_access
        && is_effectively_full_access(user_default, agent_override);
    config
}

/// Domain tool executor bound to this session's auth token and project.
fn session_domain_executor(session: &Session, ctx: &WsContext) -> Option<Arc<DomainToolExecutor>> {
    ctx.domain_api.as_ref().map(|api| {
        Arc::new(DomainToolExecutor::with_session_context(
            api.clone(),
            session.auth_token.clone(),
            session.project_id.clone(),
        ))
    })
}

/// `parent_outbound` + `parent_cancellation` are `Some` only on the
/// per-turn build path. When supplied, the subagent dispatch hook is
/// wrapped in a [`crate::gateway::session::subagent_stream::RuntimeSubagentObservabilityHook`]
/// so each `task` child run is announced + streamed on the parent
/// stream and registered as its own WS-attachable thread. The session
/// bootstrap path passes `None` (no active turn to stream onto) and
/// gets the plain blocking dispatcher.
pub(in crate::gateway::session) async fn build_kernel_with_config(
    session: &Session,
    ctx: &WsContext,
    tool_config: &aura_exec_tools::ToolConfig,
    parent_outbound: Option<&mpsc::Sender<OutboundMessage>>,
    parent_cancellation: Option<&CancellationToken>,
) -> Result<Arc<Kernel>, aura_agent_kernel::KernelError> {
    let domain_exec = session_domain_executor(session, ctx);

    let user_default = session_user_defaults(session, ctx)?;
    let session_tool_config = session_scoped_tool_config(
        tool_config,
        &user_default,
        session.tool_permissions.as_ref(),
    );

    let resolver =
        executor::build_tool_resolver(&ctx.catalog, &session_tool_config, domain_exec.clone())
            .with_installed_tools(session.installed_tools.clone());
    let resolver = register_session_tools(resolver, session, ctx);

    let (workspace, use_workspace_base_as_root) = resolve_session_workspace(session);
    let policy = session_policy(session, user_default.clone());

    // The session's agent is a root in the node's agent host; its hooks
    // handle task, spawn_agent and agent messaging. On the per-turn path
    // the task hook is wrapped so each child run is observable as its own
    // live WS thread.
    let hooks = register_session_agent(session, ctx)?;
    let resolver = attach_agent_hooks(resolver, &hooks, ctx, parent_outbound, parent_cancellation);
    // Foreground commands print on the turn's stream while they run.
    // The session bootstrap has no stream, and the call's result still
    // carries everything a frame could not take.
    let resolver = match parent_outbound {
        Some(outbound) => resolver.with_command_output_hook(Arc::new(WireCommandOutput {
            outbound: outbound.clone(),
        })),
        None => resolver,
    };
    let resolver = resolver
        .with_caller_permissions(session.agent_permissions.clone())
        .with_tool_permission_context(user_default, session.tool_permissions.clone())
        .with_originating_user_id(session.user_id.clone());
    let resolver = with_caller_context(resolver, session);

    let router = executor::build_executor_router(resolver);
    let config = session_kernel_config(session, workspace, use_workspace_base_as_root, policy);

    let kernel = Kernel::new(
        ctx.store.clone(),
        session
            .provider_override
            .clone()
            .unwrap_or_else(|| ctx.provider.clone()),
        router,
        config,
        session.agent_id,
    )?;

    Ok(Arc::new(kernel))
}

/// Register the computer-use and dev-loop control tools this session can use.
fn register_session_tools(
    mut resolver: ToolResolver,
    session: &Session,
    ctx: &WsContext,
) -> ToolResolver {
    // Register the live computer-use tool only when the run opted into
    // computer-use AND an executor URL is configured. The catalog gates
    // visibility behind `Capability::ComputerUse`; this registers the
    // executable side bound to the per-session executor endpoint.
    if session.computer_use {
        if let Some(url) = session
            .computer_executor_url
            .as_deref()
            .filter(|u| !u.trim().is_empty())
        {
            resolver.register(Box::new(aura_exec_tools::ComputerTool::new(url)));
        }
    }

    if let Some(skills) = crate::gateway::session::skills::SessionSkills::of(session, ctx) {
        resolver.register(Box::new(aura_exec_tools::skills::LoadSkillTool::new(
            Arc::new(skills),
        )));
    }

    if let Some(ref controller) = ctx.automaton_controller {
        let project_id = session.project_id.clone().unwrap_or_default();
        let workspace_root = session.project_path.clone();
        for tool in aura_exec_tools::automaton_tools::devloop_control_tools(
            controller.clone(),
            project_id,
            workspace_root,
            session.auth_token.clone(),
        ) {
            resolver.register(tool);
        }
    }
    resolver
}

fn session_policy(session: &Session, user_default: UserToolDefaults) -> PolicyConfig {
    let mut policy = PolicyConfig::default();
    policy.set_installed_integrations(session.installed_integrations.iter().cloned());
    policy.set_tool_integration_requirements(session.installed_tools.iter().filter_map(|tool| {
        tool.required_integration
            .clone()
            .map(|requirement| (tool.name.clone(), requirement))
    }));
    // Permissions are mandatory on every session; wire them into the
    // kernel policy unconditionally so the Delegate gate enforces them.
    policy.agent_permissions = session.agent_permissions.clone();
    policy
        .with_user_default(user_default)
        .with_agent_override(session.tool_permissions.clone())
        .with_mode(session.mode)
}

fn attach_agent_hooks(
    resolver: ToolResolver,
    hooks: &AgentHooks,
    ctx: &WsContext,
    parent_outbound: Option<&mpsc::Sender<OutboundMessage>>,
    parent_cancellation: Option<&CancellationToken>,
) -> ToolResolver {
    let dispatch_hook: Arc<dyn aura_exec_tools::SubagentDispatchHook> = match parent_outbound {
        Some(outbound) => Arc::new(
            crate::gateway::session::subagent_stream::RuntimeSubagentObservabilityHook::new(
                Arc::new(hooks.clone()),
                outbound.clone(),
                ctx.chat_runs.clone(),
                parent_cancellation.cloned(),
                ctx.run_id.clone(),
            ),
        ),
        None => Arc::new(hooks.clone()),
    };
    hooks
        .attach(resolver)
        .with_subagent_dispatch_hook(dispatch_hook)
}

/// Thread the caller's external agent id, project and model into the
/// tool context for cross-agent tools.
fn with_caller_context(mut resolver: ToolResolver, session: &Session) -> ToolResolver {
    // Thread the upstream OS UUID for this session into the tool context
    // so cross-agent tools can ship a server-resolvable caller id as
    // `originating_agent_id` / `parent_agent_id` instead of the truncated
    // harness blake3 hash. `skill_agent_id` is populated from
    // `RuntimeRequest.agent_identity.template_id` (the `aura-os-server`
    // `agents.agent_id` UUID) when present, with a fallback to the raw
    // `agent_id` string — see `SessionState::from_runtime_request` in
    // `state.rs`. Without this wire-up
    // the server-side `spawn_cross_agent_reply_callback` POSTs to
    // `/api/agents/{16_char_hex}/events/stream`, which the
    // `Path<AgentId = Uuid>` extractor rejects with 400 and the async
    // reply chain dies silently.
    if let Some(external_id) = session.skill_agent_id.as_deref() {
        if !external_id.trim().is_empty() {
            resolver = resolver.with_caller_external_agent_id(external_id.to_string());
        }
    }

    // Preserve project continuity when this agent delegates to another
    // project member. Agents can belong to several projects, so omitting
    // this value lets the server fall back to the target's most-recent chat.
    if let Some(project_id) = session.project_id.as_deref() {
        if !project_id.trim().is_empty() {
            resolver = resolver.with_caller_project_id(project_id.to_string());
        }
    }

    // Thread this session's resolved model into the tool context so
    // cross-agent tools (`send_to_agent`, `delegate_task`) forward it
    // to the target agent's turn. Cross-agent recipients usually have
    // no server-side configured model, so without this the recipient's
    // harness session opens with an empty model and the turn fails with
    // "model name must not be empty". Blank model is left unset so the
    // builder keeps the legacy null-model wire behavior.
    if !session.model.trim().is_empty() {
        resolver = resolver.with_caller_model_id(session.model.clone());
    }
    resolver
}

fn session_kernel_config(
    session: &Session,
    workspace: std::path::PathBuf,
    use_workspace_base_as_root: bool,
    policy: PolicyConfig,
) -> KernelConfig {
    KernelConfig {
        workspace_base: workspace,
        use_workspace_base_as_root,
        policy,
        tool_approval_prompter: session
            .tool_approval_broker
            .clone()
            .map(|broker| broker as Arc<dyn aura_agent_kernel::ToolApprovalPrompter>),
        record_observer: session.tool_approval_broker.as_ref().map(|broker| {
            Arc::new(
                crate::gateway::session::tool_policy::ToolPolicyNotifier::new(broker.outbound()),
            ) as Arc<dyn aura_agent_kernel::RecordObserver>
        }),
        originating_user_id: Some(session.user_id.clone()),
        ..KernelConfig::default()
    }
}

/// Register the session's agent as a root in the node's agent host and
/// return the hooks that act for it.
///
/// Called for every kernel build, so the root always carries the
/// session's latest auth token, permissions and inbox. Agents it already
/// spawned keep running. The council ([`crate::gateway::session::council::start_council_run`])
/// dispatches its members through the same hooks.
pub(in crate::gateway::session) fn register_session_agent(
    session: &Session,
    ctx: &WsContext,
) -> Result<AgentHooks, aura_agent_kernel::KernelError> {
    let env = session_agent_env(session, ctx)?;
    ctx.agent_host.register_root(RootAgent {
        agent_id: session.agent_id,
        name: "root".to_string(),
        permissions: session.agent_permissions.clone(),
        tool_permissions: session.tool_permissions.clone(),
        mode: session.mode,
        model: session.model.clone(),
        env,
        inbox: Some(Arc::new(
            crate::gateway::session::agent_inbox::SessionInbox::new(&session.agent_mailbox),
        )),
    });
    Ok(ctx.agent_host.hooks_for(session.agent_id))
}

/// Environment the session's root agent and its children run in.
fn session_agent_env(
    session: &Session,
    ctx: &WsContext,
) -> Result<AgentEnv, aura_agent_kernel::KernelError> {
    let domain_exec = session_domain_executor(session, ctx);
    let user_default = session_user_defaults(session, ctx)?;
    let session_tool_config = session_scoped_tool_config(
        &ctx.tool_config,
        &user_default,
        session.tool_permissions.as_ref(),
    );

    let factory = crate::gateway::session::child_kernel::SessionChildKernelFactory::new(
        crate::gateway::session::child_kernel::SessionChildKernelFactoryParams {
            catalog: ctx.catalog.clone(),
            session_tool_config,
            domain_exec,
            installed_tools: session.installed_tools.clone(),
            installed_integrations: session.installed_integrations.clone(),
            automaton_controller: ctx.automaton_controller.clone(),
            automaton_project_id: session.project_id.clone().unwrap_or_default(),
            automaton_workspace_root: session.project_path.clone(),
            auth_token: session.auth_token.clone(),
        },
    );
    let mut env = AgentEnv::new(factory);
    // Children work in the session's project, not a scratch directory.
    env.workspace = Some(resolve_session_workspace(session));
    env.user_tool_defaults = user_default;
    env.originating_user_id = Some(session.user_id.clone());
    attach_aura_os_hooks(&mut env, session, ctx);
    Ok(env)
}

fn attach_aura_os_hooks(env: &mut AgentEnv, session: &Session, ctx: &WsContext) {
    if let Some(base_url) = ctx
        .aura_os_server_url
        .as_deref()
        .filter(|url| !url.is_empty())
    {
        // With aura-os, spawn_agent creates aura-os agents, and messages
        // to agents outside this node go through aura-os.
        env.spawn_override = Some(Arc::new(AuraServerSpawnHook::new(
            base_url.to_string(),
            session.auth_token.clone(),
            session.aura_org_id.clone(),
            ctx.store.clone(),
        )));
        let remote = Arc::new(AuraServerAgentHook::new(
            base_url.to_string(),
            session.auth_token.clone(),
        ));
        env.remote_control = Some(remote.clone());
        env.remote_read = Some(remote);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_output_is_a_tool_output_frame() {
        let (tx, mut rx) = mpsc::channel(1);
        let hook = WireCommandOutput { outbound: tx };
        hook.output("toolu_1", b"building\n");
        hook.output("toolu_1", b"more\n");
        match rx.try_recv() {
            Ok(OutboundMessage::ToolOutput(msg)) => {
                assert_eq!(msg.id, "toolu_1");
                assert_eq!(msg.chunk, "building\n");
            }
            other => panic!("expected tool output, got {other:?}"),
        }
        assert!(rx.try_recv().is_err(), "a full stream drops the rest");
    }
}
