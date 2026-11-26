//! Chat session preparation and the `SessionReady` bootstrap.

use super::kernel::build_kernel_with_config;
use crate::gateway::session::chat::populate_tool_definitions;
use crate::gateway::session::state::is_managed_safe_workspace_path;
use crate::gateway::session::{Session, WsContext};
use crate::protocol::{
    tool_info_from_definition_with_state, OutboundMessage, SessionReady, SkillInfo, ToolInfo,
};
use aura_core_types::{resolve_effective_permission, ToolState, UserToolDefaults};
use aura_engine::capabilities;
use aura_model::ModelProvider;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{error, info};

pub(in crate::gateway::session) fn session_user_defaults(
    session: &Session,
    ctx: &WsContext,
) -> Result<UserToolDefaults, aura_agent_kernel::KernelError> {
    ctx.store
        .get_user_tool_defaults(&session.user_id)
        .map_err(|e| aura_agent_kernel::KernelError::Store(format!("get_user_tool_defaults: {e}")))
        .map(|defaults| defaults.unwrap_or_default())
}

fn effective_tool_infos(session: &Session, defaults: &UserToolDefaults) -> Vec<ToolInfo> {
    session
        .tool_definitions
        .iter()
        .filter_map(|tool| {
            let state = resolve_effective_permission(
                defaults,
                session.tool_permissions.as_ref(),
                &tool.name,
            );
            (state != ToolState::Deny).then(|| tool_info_from_definition_with_state(tool, state))
        })
        .collect()
}

/// Error returned when building a chat [`Session`] from a
/// [`RuntimeRequest`] fails before the WS attaches.
///
/// Carries a `code` matching the legacy `OutboundMessage::Error.code`
/// strings so the HTTP error path emits the same diagnostic surface
/// the pre-`POST /v1/run` flow used.
#[derive(Debug)]
pub(crate) struct ChatRequestError {
    pub code: &'static str,
    pub message: String,
}

/// Build a fully-applied chat [`Session`] from a [`RuntimeRequest`]
/// before the WebSocket attaches.
///
/// Replaces the pre-Phase-A `handle_session_init` flow. Validation
/// errors (invalid workspace, malformed provider overrides, etc.)
/// are returned as a [`ChatRequestError`] so the gateway can surface
/// them as a 4xx HTTP response.
pub(crate) async fn prepare_chat_session(
    request: aura_core_protocol::RuntimeRequest,
    ctx: &WsContext,
) -> Result<Session, ChatRequestError> {
    let resolved_provider_override = resolve_provider_override(&request)?;

    let mut session = Session::new(ctx.workspace_base.clone());
    session.project_base = ctx.project_base.clone();
    session.auth_token = ctx.auth_token.clone();

    if let Err(e) = session.apply_chat_runtime_request(request) {
        return Err(ChatRequestError {
            code: "invalid_workspace",
            message: e,
        });
    }

    load_session_tool_permissions(&mut session, ctx)?;

    if let Some(provider) = resolved_provider_override {
        session.provider_name = provider.name().to_string();
        session.provider_override = Some(provider);
    }

    relocate_unmanaged_project_path(&mut session, ctx);

    populate_tool_definitions(&mut session, ctx);

    Ok(session)
}

/// Resolve the request's provider overrides into a provider, if any.
fn resolve_provider_override(
    request: &aura_core_protocol::RuntimeRequest,
) -> Result<Option<Arc<dyn ModelProvider + Send + Sync>>, ChatRequestError> {
    let Some(overrides) = request.model.provider_overrides.as_ref() else {
        return Ok(None);
    };
    let reasoner_overrides = aura_model::SessionOverrides {
        default_model: overrides.default_model.clone(),
        fallback_model: overrides.fallback_model.clone(),
        prompt_caching_enabled: overrides.prompt_caching_enabled,
        prompt_cache_key: overrides.prompt_cache_key.clone(),
        prompt_cache_retention: overrides.prompt_cache_retention.clone(),
    };
    match aura_model::with_session_overrides(reasoner_overrides) {
        Ok(selection) => Ok(Some(selection.provider)),
        Err(e) => Err(ChatRequestError {
            code: "invalid_provider_config",
            message: e.to_string(),
        }),
    }
}

/// Load the agent's stored tool permissions when the request carried none.
fn load_session_tool_permissions(
    session: &mut Session,
    ctx: &WsContext,
) -> Result<(), ChatRequestError> {
    if session.tool_permissions.is_none() {
        let tool_context =
            crate::tool_permissions::load_agent_tool_context(ctx.store.as_ref(), session.agent_id);
        match tool_context {
            Ok(agent_ctx) => {
                session.tool_permissions = agent_ctx.tool_permissions;
            }
            Err(e) => {
                return Err(ChatRequestError {
                    code: "tool_permissions_load_failed",
                    message: e,
                });
            }
        }
    }
    Ok(())
}

/// Re-root a project path outside the managed safe workspaces under the
/// configured project base.
fn relocate_unmanaged_project_path(session: &mut Session, ctx: &WsContext) {
    if let (Some(ref base), Some(ref pp)) = (&ctx.project_base, &session.project_path) {
        if !is_managed_safe_workspace_path(&ctx.workspace_base, pp) {
            let slug = pp.file_name().and_then(|n| n.to_str()).unwrap_or("default");
            session.project_path = Some(base.join(slug));
        }
    }
}

/// Bootstrap an already-applied chat [`Session`] now that its WS is
/// attached.
///
/// Emits the per-session kernel bootstrap (the `SessionStart`
/// transaction + runtime-capability install record), publishes the
/// identity to the runtime scheduler registry, and sends the
/// terminal `SessionReady` frame so the client can start streaming
/// `user_message` traffic.
pub(in crate::gateway::session) async fn emit_session_ready(
    session: &mut Session,
    outbound_tx: &mpsc::Sender<OutboundMessage>,
    ctx: &WsContext,
) {
    bootstrap_session(session, ctx).await;

    // Publish the per-agent identity to the runtime-side scheduler
    // registry. The chat WS path itself goes through
    // `dispatch_turn_to_agent`, which already builds the right
    // `AgentLoopConfig` from `Session`. The reason we still register
    // here is the worker fan-out path: when a tool-permission update
    // or post-automaton-completion fan-out triggers
    // `Scheduler::schedule_agent`, the scheduler must be able to
    // build the correct config out-of-band. Without this registration
    // the worker path 429s with `aura_org_id="missing"
    // aura_session_id="missing"`. See
    // `crates/surface/aura-node/src/scheduler.rs` for the registry
    // contract and `crates/surface/aura-node/src/gateway/handlers/tx.rs` /
    // `crates/surface/aura-node/src/gateway/handlers/tool_permissions.rs` for the
    // worker fan-out callers.
    ctx.scheduler
        .identity_registry()
        .register(session.agent_id, session.as_runtime_identity());

    let defaults = match session_user_defaults(session, ctx) {
        Ok(defaults) => defaults,
        Err(e) => {
            error!(
                session_id = %session.session_id,
                error = %e,
                "Failed to load user tool defaults for SessionReady"
            );
            UserToolDefaults::default()
        }
    };
    let tools: Vec<ToolInfo> = effective_tool_infos(session, &defaults);
    let skills = session_skill_infos(session, ctx);

    info!(
        session_id = %session.session_id,
        model = %session.model,
        tool_count = tools.len(),
        integration_count = session.installed_integrations.len(),
        skill_count = skills.len(),
        "Session initialized"
    );

    let _ = outbound_tx.try_send(OutboundMessage::SessionReady(SessionReady {
        session_id: session.session_id.clone(),
        tools,
        skills,
    }));
}

fn session_skill_infos(session: &Session, ctx: &WsContext) -> Vec<SkillInfo> {
    match (&ctx.skill_manager, &session.skill_agent_id) {
        (Some(sm), Some(agent_id)) => {
            if let Ok(mgr) = sm.read() {
                mgr.agent_skill_meta(agent_id)
                    .into_iter()
                    .map(|m| SkillInfo {
                        name: m.name,
                        description: m.description,
                    })
                    .collect()
            } else {
                Vec::new()
            }
        }
        _ => Vec::new(),
    }
}

/// Build the per-session kernel and emit its bootstrap transactions.
///
/// Two side-effecting steps are performed (and three error paths logged
/// via the original `error!` strings so operators don't see message
/// drift):
/// 1. `Transaction::session_start` — Invariant §2 (Every State Change
///    Is a Transaction) + §11 (Session-Scoped Approvals): the session
///    start is itself a state change that must be recorded and that
///    resets the kernel's session-scoped approval cache. Emit the
///    transaction before anything else on this kernel so the record
///    reflects session boundaries for replay.
/// 2. `record_runtime_capabilities` — bundles the `installed_tools` /
///    `installed_integrations` snapshot into a single `System` record
///    entry so downstream replay knows what surface the LLM was given.
///
/// Failures only log; we deliberately don't surface them to the caller
/// because a kernel build / capability-recording failure must NOT
/// prevent us from sending `SessionReady` (the UI would otherwise spin
/// forever waiting on a session that already loaded).
async fn bootstrap_session(session: &Session, ctx: &WsContext) {
    match build_kernel_with_config(session, ctx, &ctx.tool_config, None, None).await {
        Ok(kernel) => {
            if let Err(e) = kernel
                .process_direct(aura_core_types::Transaction::session_start(
                    session.agent_id,
                ))
                .await
            {
                error!(
                    session_id = %session.session_id,
                    error = %e,
                    "Failed to record SessionStart transaction through kernel"
                );
            }

            if let Err(e) = capabilities::record_runtime_capabilities(
                &kernel,
                "session",
                Some(&session.session_id),
                &session.installed_tools,
                &session.installed_integrations,
            )
            .await
            {
                error!(
                    session_id = %session.session_id,
                    error = %e,
                    "Failed to record runtime capability install through kernel during session init"
                );
            }
        }
        Err(e) => {
            error!(
                session_id = %session.session_id,
                error = %e,
                "Failed to build kernel for session capability recording"
            );
        }
    }
}
