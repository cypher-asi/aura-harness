//! Agent-turn preparation: building the per-turn kernel, prompt and
//! config, then spawning the agent loop.

use super::attachments::build_user_message;
use super::{AgentTurn, TitleTask};
use crate::gateway::session::helpers;
use crate::gateway::session::{Session, WsContext};
use crate::protocol::{AssistantMessageStart, ErrorMsg, OutboundMessage, UserMessage};
use aura_agent::{
    AgentLoop, AgentLoopConfig, AgentLoopEvent, AgentLoopResult, KernelModelGateway,
    KernelToolGateway,
};
use aura_agent_kernel::Kernel;
use aura_model::Message;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info};
use uuid::Uuid;

pub(in crate::gateway::session) fn populate_tool_definitions(
    session: &mut Session,
    ctx: &WsContext,
) {
    let user_default = ctx
        .store
        .get_user_tool_defaults(&session.user_id)
        .ok()
        .flatten()
        .unwrap_or_default();
    session.tool_definitions = crate::tool_permissions::effective_tool_definitions(
        &ctx.catalog,
        &ctx.tool_config,
        &session.installed_tools,
        &session.installed_integrations,
        &user_default,
        session.tool_permissions.as_ref(),
        Some(&session.agent_permissions),
    )
    .into_iter()
    .map(|(definition, _state)| definition)
    .collect();
}

/// Prepared turn-context — the raw materials [`dispatch_turn_to_agent`]
/// needs to spawn the agent loop. `model_gateway` and `tool_gateway` are
/// already wired to the per-turn kernel; `messages` and `tools` are
/// owned snapshots so the spawned task does not borrow `Session`.
struct PreparedTurn {
    model_gateway: KernelModelGateway,
    tool_gateway: KernelToolGateway,
    kernel: Arc<Kernel>,
    config: AgentLoopConfig,
    messages: Vec<Message>,
    tools: Vec<aura_model::ToolDefinition>,
    recorder: Option<Arc<aura_agent_trace::RunRecorder>>,
    title: Option<TitleTask>,
}

/// Prepare and spawn an agent-loop turn as a background task.
pub(super) async fn start_turn(
    session: &mut Session,
    msg: UserMessage,
    outbound_tx: &mpsc::Sender<OutboundMessage>,
    ctx: &WsContext,
) -> Option<AgentTurn> {
    // Phase A: the chat WS is only attached after
    // `prepare_chat_session` has already applied the
    // [`aura_core_protocol::RuntimeRequest`] on the HTTP side, so
    // `session.initialized` is always true here. The previous
    // "not_initialized" error path (which guarded a missing
    // `session_init` first frame) is unreachable under the new
    // two-step exchange.
    let message_id = Uuid::new_v4().to_string();
    let _ = outbound_tx.try_send(OutboundMessage::AssistantMessageStart(
        AssistantMessageStart {
            message_id: message_id.clone(),
        },
    ));

    // Mint the per-turn cancellation token up front so it can be both
    // threaded into the subagent dispatch hook (built during kernel
    // construction) and used to drive the agent loop below. This makes
    // `SpawnRequest.cancellation` carry the parent token so cancelling
    // the parent turn propagates into a `Wait` child.
    let cancel_token = CancellationToken::new();

    let prepared = match prepare_turn_context(session, msg, ctx, outbound_tx, &cancel_token).await {
        Ok(p) => p,
        Err(err) => {
            crate::inbound_console::ws_rejection_line(
                "framing",
                &err.code,
                Some(&format!("session={} {}", session.session_id, err.message)),
            );
            let _ = outbound_tx.try_send(OutboundMessage::Error(err));
            return None;
        }
    };

    Some(dispatch_turn_to_agent(
        prepared,
        outbound_tx,
        message_id,
        cancel_token,
    ))
}

/// Build everything the agent loop needs for a single turn:
/// the user `Message` (with optional image attachments), a session-scoped
/// `tool_config` extended with skill grants, the per-turn kernel and its
/// gateways, and the populated `AgentLoopConfig` (system prompt, skills,
/// memory observer).
///
/// Side-effects, in the order the original inline code performed them:
/// 1. `session.messages.push(user_msg)` — append before any potential
///    failure, so the conversation log reflects the user's intent even
///    if the kernel build subsequently errors.
/// 2. `Transaction::user_prompt` — Invariant §2: every user-visible
///    state change must be a transaction committed through the kernel.
///    Record the prompt before the agent loop runs so the record log
///    reflects the turn boundary even if the loop aborts mid-stream.
///
/// Returns `Err(ErrorMsg)` for the two failure modes the caller surfaces
/// over the WebSocket: kernel build failure and user-prompt recording
/// failure. Error codes / messages match the pre-refactor strings exactly.
async fn prepare_turn_context(
    session: &mut Session,
    msg: UserMessage,
    ctx: &WsContext,
    outbound_tx: &mpsc::Sender<OutboundMessage>,
    cancel_token: &CancellationToken,
) -> Result<PreparedTurn, ErrorMsg> {
    let user_msg = build_user_message(&msg).await;
    session.messages.push(user_msg);

    let tool_config = skill_tool_config(session, ctx);
    let kernel = build_turn_kernel(session, ctx, &tool_config, outbound_tx, cancel_token).await?;
    record_user_prompt(&kernel, session, &msg.content).await?;

    let model_gateway = KernelModelGateway::new(kernel.clone());
    let tool_gateway = KernelToolGateway::new(kernel.clone());

    let mut config = session.agent_loop_config();
    config.tool_hints = msg.tool_hints;
    if let Some(branch) = ctx.tool_config.push_branch.as_deref() {
        config.system_prompt.push_str(&cloud_git_note(branch));
    }

    // Through the turn's own kernel: a second kernel for the agent would
    // claim the same sequence numbers as this one while the turn writes.
    let title = session.title_model.take().map(|model| {
        TitleTask::spawn(
            KernelModelGateway::new(kernel.clone()),
            config.clone(),
            model,
            msg.content.clone(),
            outbound_tx.clone(),
        )
    });

    apply_subagent_surface(&mut config);

    // Resolve active skill names before creating the memory observer so we can
    // forward them for procedure extraction.
    let active_skill_names = inject_skills(session, ctx, &mut config);
    if let Some(ref mm) = ctx.memory_manager {
        attach_memory(mm, session, &mut config, &msg.content, active_skill_names).await;
    }

    let tools = session.tool_definitions.clone();
    let messages = session.messages.clone();

    let recorder = helpers::trace_recorder(session, ctx, &config.model);
    if let Some(rec) = &recorder {
        rec.begin_turn(&msg.content);
        config.trace_blobs = rec.blob_store();
    }

    Ok(PreparedTurn {
        model_gateway,
        tool_gateway,
        kernel,
        config,
        messages,
        tools,
        recorder,
        title,
    })
}

/// Session-scoped copy of the node tool config, extended with the
/// skill-granted extra paths and commands.
fn skill_tool_config(session: &Session, ctx: &WsContext) -> aura_exec_tools::ToolConfig {
    let mut tool_config = ctx.tool_config.clone();
    let has_sm = ctx.skill_manager.is_some();
    let has_aid = session.skill_agent_id.is_some();
    info!(
        session_id = %session.session_id,
        has_skill_manager = has_sm,
        has_skill_agent_id = has_aid,
        skill_agent_id = ?session.skill_agent_id,
        "Skill permission check starting"
    );
    if let (Some(ref sm), Some(ref agent_id)) = (&ctx.skill_manager, &session.skill_agent_id) {
        if let Ok(mgr) = sm.read() {
            let perms = mgr.agent_permissions(agent_id);
            info!(
                session_id = %session.session_id,
                extra_paths = ?perms.extra_paths,
                extra_commands = ?perms.extra_commands,
                "Skill permissions resolved"
            );
            if !perms.extra_paths.is_empty() {
                tool_config.extra_allowed_paths.extend(perms.extra_paths);
            }
            if !perms.extra_commands.is_empty() && !tool_config.command.command_allowlist.is_empty()
            {
                tool_config
                    .command
                    .command_allowlist
                    .extend(perms.extra_commands);
            }
        }
    }
    tool_config
}

async fn build_turn_kernel(
    session: &Session,
    ctx: &WsContext,
    tool_config: &aura_exec_tools::ToolConfig,
    outbound_tx: &mpsc::Sender<OutboundMessage>,
    cancel_token: &CancellationToken,
) -> Result<Arc<Kernel>, ErrorMsg> {
    match helpers::build_kernel_with_config(
        session,
        ctx,
        tool_config,
        Some(outbound_tx),
        Some(cancel_token),
    )
    .await
    {
        Ok(k) => Ok(k),
        Err(e) => {
            error!(session_id = %session.session_id, error = %e, "Failed to build kernel");
            Err(ErrorMsg {
                code: "kernel_error".into(),
                message: format!("Failed to build kernel: {e}"),
                recoverable: true,
                support_id: None,
            })
        }
    }
}

async fn record_user_prompt(
    kernel: &Kernel,
    session: &Session,
    content: &str,
) -> Result<(), ErrorMsg> {
    if let Err(e) = kernel
        .process_direct(aura_core_types::Transaction::user_prompt(
            session.agent_id,
            content.to_owned(),
        ))
        .await
    {
        error!(
            session_id = %session.session_id,
            error = %e,
            "Failed to record UserPrompt transaction through kernel"
        );
        return Err(ErrorMsg {
            code: "kernel_error".into(),
            message: format!("Failed to record user prompt: {e}"),
            recoverable: true,
            support_id: None,
        });
    }
    Ok(())
}

/// Record the subagent surface (chars + per-entry segments) on `config`.
fn apply_subagent_surface(config: &mut AgentLoopConfig) {
    // Subagent surface is constant for the bundled registry today. The
    // dispatch tool re-emits the registry's name + description per
    // entry on every turn, so the per-turn breakdown attributes those
    // chars to the "Subagents" bucket. Custom registries (e.g. tests)
    // would override this once they're plumbed through `WsContext`.
    let subagent_registry = aura_agent_subagent::registry::SubagentRegistry::bundled();
    config.subagents_chars = aura_agent_subagent::registry::registry_chars(&subagent_registry);
    // Parallel per-subagent text for the context-contents viewer: same
    // registry surface `registry_chars` counts tokens for.
    config.subagents_segments =
        aura_agent_subagent::registry::registry_segments(&subagent_registry);
}

/// Inject the agent's skills into the system prompt; returns the names of
/// the injected skills.
fn inject_skills(session: &Session, ctx: &WsContext, config: &mut AgentLoopConfig) -> Vec<String> {
    let mut active_skill_names: Vec<String> = Vec::new();
    if let (Some(ref sm), Some(ref agent_id)) = (&ctx.skill_manager, &session.skill_agent_id) {
        if let Ok(mgr) = sm.read() {
            // Capture the prompt length before injection so the "Skills"
            // bucket can be reported as the delta — the text the
            // skill-injection step actually appends to the system
            // prompt — without double-counting it under "System
            // prompt" downstream in `agent_loop::context::recompute_breakdown`.
            let pre_inject_chars = config.system_prompt.len();
            let injected = mgr.inject_agent_skills(agent_id, &mut config.system_prompt);
            let post_inject_chars = config.system_prompt.len();
            config.skills_chars = post_inject_chars.saturating_sub(pre_inject_chars);
            // Parallel per-skill text for the context-contents viewer.
            // The skill body is already embedded in the system prompt;
            // the "Skills" bucket carries only each skill's name +
            // summary so the text is not double-counted across buckets.
            config.skills_segments = injected
                .iter()
                .map(|s| (s.name.clone(), s.description.clone()))
                .collect();
            if !injected.is_empty() {
                active_skill_names = injected.iter().map(|s| s.name.clone()).collect();
                debug!(
                    session_id = %session.session_id,
                    skill_count = active_skill_names.len(),
                    skills_chars = config.skills_chars,
                    "Injected agent skills into prompt"
                );
            }
        }
    }
    active_skill_names
}

/// Inject recalled memory into the system prompt and register the
/// memory turn observer.
async fn attach_memory(
    mm: &Arc<aura_context_memory::MemoryManager>,
    session: &Session,
    config: &mut AgentLoopConfig,
    content: &str,
    active_skill_names: Vec<String>,
) {
    let mem_id = session.memory_agent_id();
    mm.prepare_context_with_query(
        mem_id,
        &mut config.system_prompt,
        aura_context_memory::MemoryQueryContext {
            text: content.to_owned(),
            active_skills: active_skill_names.clone(),
            access: aura_context_memory::MemoryAccessContext {
                project_id: session.project_id.clone(),
                user_id: (!session.user_id.trim().is_empty()).then(|| session.user_id.clone()),
                include_legacy: false,
            },
            ..Default::default()
        },
    )
    .await;
    config.observers.push(
        aura_engine::memory_observer::MemoryTurnObserver::new_with_request_context(
            Arc::clone(mm),
            mem_id,
            aura_context_memory::RefinementRequestContext {
                auth_token: session.auth_token.clone(),
                aura_project_id: session.project_id.clone(),
                aura_agent_id: session.aura_agent_id.clone(),
                aura_session_id: session.aura_session_id.clone(),
                aura_org_id: session.aura_org_id.clone(),
                user_id: (!session.user_id.trim().is_empty()).then(|| session.user_id.clone()),
            },
            active_skill_names,
            Some(session.session_id.clone()),
        ),
    );
}

/// Spawn the agent-loop background task and the matching event-forwarder
/// task, returning the [`AgentTurn`] handle the WebSocket loop polls on.
///
/// Cancellation: a single `CancellationToken` is cloned three times —
/// the original (returned in `AgentTurn`), one moved into the loop
/// future (so cooperative cancellation reaches the model call), and one
/// kept for the post-loop check that flips `timed_out = true` when the
/// run was cancelled rather than completing on its own.
fn dispatch_turn_to_agent(
    prepared: PreparedTurn,
    outbound_tx: &mpsc::Sender<OutboundMessage>,
    message_id: String,
    cancel_token: CancellationToken,
) -> AgentTurn {
    let PreparedTurn {
        model_gateway,
        tool_gateway,
        kernel,
        config,
        messages,
        tools,
        recorder,
        title,
    } = prepared;
    let agent_loop = AgentLoop::new(config);

    let cancel_for_loop = cancel_token.clone();
    let cancel_for_check = cancel_token.clone();

    let (event_tx, event_rx) = mpsc::channel::<AgentLoopEvent>(1024);

    let join_handle = tokio::spawn(async move {
        let mut result: anyhow::Result<AgentLoopResult> = agent_loop
            .run_with_events(
                &model_gateway,
                &tool_gateway,
                messages,
                tools,
                Some(event_tx),
                Some(cancel_for_loop),
            )
            .await
            .map_err(Into::into);

        if cancel_for_check.is_cancelled() {
            if let Ok(ref mut r) = result {
                r.timed_out = true;
            }
        }
        result
    });

    let outbound_for_stream = outbound_tx.clone();
    let stream_forward_handle = tokio::spawn(helpers::forward_and_record(
        event_rx,
        outbound_for_stream,
        recorder,
    ));

    AgentTurn {
        cancel_token,
        join_handle,
        stream_forward_handle,
        message_id,
        kernel,
        title,
    }
}

/// What a cloud agent's system prompt says about its git branch.
pub(super) fn cloud_git_note(branch: &str) -> String {
    format!(
        "\n\n<cloud_git>\nYou work on branch `{branch}` of a clone of this project's repository. \
         When your task is done, call git_ship: it commits every change, pulls main, pushes \
         this branch and puts it in the merge queue, which merges it into main once the build \
         and tests pass. git_commit and git_push also work, and only push this branch; every \
         push goes into the merge queue. Never push other branches. If the repo has a \
         `changelog.d/` folder, write your changelog entry as a new file there, named after \
         your branch; never edit CHANGELOG.md.\n</cloud_git>"
    )
}
