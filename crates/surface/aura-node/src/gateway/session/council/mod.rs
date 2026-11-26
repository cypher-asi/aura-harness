//! AURA Council orchestrator.
//!
//! A council run convenes `members` model seats to answer the user's
//! question, then synthesizes one combined response.
//!
//! The fan-out is **deterministic and harness-orchestrated** rather than
//! model-driven: instead of prompting the synthesizer model to "please
//! call the `task` tool N times" (which is non-deterministic, slow, and
//! emits ordinary task spawns the UI can't group), the coordinator
//! dispatches the N member child runs DIRECTLY through the same
//! [`super::subagent_stream::RuntimeSubagentObservabilityHook`] +
//! [`aura_engine::AgentHooks`] the `task` tool uses.
//! That reuses all the existing, already-working subagent machinery
//! (child-run registration, live WS-attachable threads, status frames)
//! while giving the council exactly the shape the UI expects:
//!
//! - The PARENT run is created + registered through the SAME
//!   [`super::chat_run::spawn_chat_run`] path a `POST /v1/run` chat run
//!   uses (so `WS /stream/:run_id` attaches non-destructively), prepared
//!   with `members[0]`'s model — the synthesizer.
//! - Once the parent session is ready, the coordinator dispatches every
//!   member as a council-tagged subagent (`council_index = Some(i)`,
//!   `model_override = member.model`, prompt = the user's question)
//!   IN PARALLEL. Each emits a `SubagentSpawned { council_index, model,
//!   parent_tool_use_id }` on the parent stream IMMEDIATELY — so all N
//!   member columns appear at once — and streams live on its own child
//!   run. All members share one synthetic `council_parent_tool_use_id`
//!   so the UI folds them into a single council panel (N columns), while
//!   each still dispatches with its own `tool_call_id` so the
//!   per-`(parent, tool_call_id)` dedupe never collapses two members.
//! - When every member returns, the coordinator injects ONE synthesis
//!   `user_message` carrying the members' answers; the synthesizer's
//!   normal text turn is the combined answer rendered below the panel.
//!
//! Cancellation: each member's child token is forked from the parent
//! run's `shutdown` token, so a single `POST /v1/run/:id/stop` (or a
//! parent `Cancel`) aborts every in-flight member; the coordinator also
//! bails before injecting synthesis if `shutdown` fired.

use std::sync::Arc;
use std::time::Duration;

use aura_core_protocol::{
    ConversationMessage, CouncilMechanism, CouncilMember, CouncilMemberRole, RuntimeRequest,
    RuntimeRequestType,
};
use aura_core_types::{AgentId, AgentPermissions, AgentToolPermissions, UserToolDefaults};
use aura_engine::AgentHooks;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

use super::chat_run::{ChatEventChannel, ChatRunHandle, ChatRunRegistry};
use super::helpers::{prepare_chat_session, register_session_agent, ChatRequestError};
use super::WsContext;
use crate::protocol::{InboundMessage, OutboundMessage, UserMessage};

mod dispatch;
mod prompts;

use dispatch::dispatch_members;
use prompts::{build_second_opinion_prompt, build_synthesis_prompt};

/// Default cap on council members when `AURA_COUNCIL_MAX_MEMBERS` is
/// unset / unparsable. Extra members beyond the cap are silently
/// truncated (with a warning).
const DEFAULT_COUNCIL_MAX_MEMBERS: usize = 4;

/// Bundled subagent kind each council member runs as. `general_purpose`
/// is the full multi-step agent loop (read/write/run tools), so a member
/// answers the query like a real agent rather than a read-only explorer.
const COUNCIL_MEMBER_KIND: &str = "general_purpose";

/// Hermes-style advisor contract for Second Opinion references. This is
/// carried as a child system-prompt addendum and paired with an empty
/// `override_tool_subset`, so the runtime policy and prompt agree: the
/// reference model advises only; it is not the acting/final agent.
const SECOND_OPINION_REFERENCE_ADVISOR_PROMPT: &str = "\
Second Opinion advisor contract. This addendum overrides any generic subagent wording above.\n\n\
You are a reference advisor in Aura Second Opinion. You are NOT the acting/final agent and you do \
NOT execute anything: no tools are available to you, and you must not claim to have read files, run \
commands, browsed, edited code, or inspected external state unless that evidence is explicitly \
included in the prompt. Do not apologize for lack of access.\n\n\
Give private guidance for the final model. Base every claim on the provided conversation and user \
request. Separate known facts from assumptions. Flag unsupported claims, missing context, likely \
risks, disagreements, and concrete next steps. Return advice only; do not answer as if you are the \
user-facing assistant.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CouncilRunKind {
    Council,
    SecondOpinion,
}

/// Parent-derived inputs the coordinator needs to build each member's
/// [`SubagentDispatchRequest`]. Snapshotted from the prepared
/// [`super::Session`] BEFORE it is moved into the parent run driver
/// (mirrors the fields the `task` tool pulls off its `ToolContext`).
struct CouncilDispatchParams {
    parent_agent_id: AgentId,
    parent_permissions: AgentPermissions,
    parent_tool_permissions: Option<AgentToolPermissions>,
    user_tool_defaults: UserToolDefaults,
    originating_user_id: String,
    parent_model_id: String,
    chat_runs: ChatRunRegistry,
    dispatcher: AgentHooks,
    /// Combine mechanism stamped onto every member's `SubagentSpawned`
    /// so the UI can label the council panel with the active mechanism.
    mechanism: CouncilMechanism,
}

/// Everything the detached coordinator task needs to fan a council out
/// once the parent run is registered + ready.
struct CouncilCoordinator {
    handle: Arc<ChatRunHandle>,
    members: Vec<CouncilMember>,
    mechanism: CouncilMechanism,
    run_kind: CouncilRunKind,
    query: String,
    run_id: String,
    shutdown: CancellationToken,
    params: CouncilDispatchParams,
}

/// Start an AURA Council run.
///
/// Mirrors [`super::chat_run::spawn_chat_run`]'s setup to create +
/// register the PARENT run (hosting the synthesizer, `members[0]`), then
/// detaches a coordinator task that — once the session is ready —
/// directly dispatches the members as council-tagged subagents and, when
/// they return, injects the synthesis turn. Returns the registered
/// `run_id` (the caller turns it into `{ run_id, event_stream_url }`).
///
/// Errors mirror [`prepare_chat_session`] plus council-specific
/// validation (`council_no_members`, `invalid_council_request`).
pub(crate) async fn start_council_run(
    req: RuntimeRequest,
    ctx: WsContext,
) -> Result<String, ChatRequestError> {
    let (members, mechanism, conversation_messages) = council_request_fields(&req)?;
    let members = truncate_members(members, council_max_members());
    let run_kind = classify_council_members(&members)?;
    let query = latest_user_query(&conversation_messages);

    // The PARENT run hosts the synthesizer/final model: prepare it with
    // members[0]'s model so the synthesis/final turn runs on the first
    // model. For Second Opinion, slot 0 is validated as the aggregator
    // and reference slots are advisory-only children.
    let synth_model = members[0].model.clone();
    let registry = ctx.chat_runs.clone();

    let chat_req = RuntimeRequest {
        r#type: RuntimeRequestType::Chat {
            conversation_messages,
        },
        model: synth_model,
        ..req
    };

    let session = prepare_chat_session(chat_req, &ctx).await?;

    let params = snapshot_dispatch_params(&session, &ctx, registry.clone(), mechanism)?;

    let run_id = Uuid::new_v4().to_string();
    let handle = super::spawn_chat_run(session, ctx, run_id.clone(), registry);
    let shutdown = handle.shutdown.clone();

    info!(
        run_id = %run_id,
        member_count = members.len(),
        mechanism = mechanism.as_wire(),
        run_kind = ?run_kind,
        "AURA Council run started"
    );

    tokio::spawn(run_council_coordinator(CouncilCoordinator {
        handle,
        members,
        mechanism,
        run_kind,
        query,
        run_id: run_id.clone(),
        shutdown,
        params,
    }));

    Ok(run_id)
}

/// Pull members / mechanism / conversation out of a council request,
/// rejecting non-council requests and empty member lists.
fn council_request_fields(
    req: &RuntimeRequest,
) -> Result<
    (
        Vec<CouncilMember>,
        CouncilMechanism,
        Vec<ConversationMessage>,
    ),
    ChatRequestError,
> {
    let (members, mechanism, conversation_messages) = match req.r#type {
        RuntimeRequestType::Council {
            ref members,
            mechanism,
            ref conversation_messages,
        } => (members.clone(), mechanism, conversation_messages.clone()),
        _ => {
            return Err(ChatRequestError {
                code: "invalid_council_request",
                message: "start_council_run requires a RuntimeRequestType::Council request"
                    .to_string(),
            });
        }
    };

    if members.is_empty() {
        return Err(ChatRequestError {
            code: "council_no_members",
            message: "council run requires at least one member".to_string(),
        });
    }
    Ok((members, mechanism, conversation_messages))
}

/// Register the parent as a root agent and snapshot its identity /
/// permissions BEFORE `session` + `ctx` are moved into the driver.
/// Members dispatch through the SAME hooks the `task` tool uses, so
/// they inherit the parent's identity / permissions / workspace and
/// run the full real-agent loop with only their model overridden.
fn snapshot_dispatch_params(
    session: &super::Session,
    ctx: &WsContext,
    registry: ChatRunRegistry,
    mechanism: CouncilMechanism,
) -> Result<CouncilDispatchParams, ChatRequestError> {
    let dispatcher = register_session_agent(session, ctx).map_err(|e| ChatRequestError {
        code: "council_dispatcher_build_failed",
        message: format!("failed to build council member dispatcher: {e}"),
    })?;
    let user_tool_defaults =
        super::helpers::session_user_defaults(session, ctx).map_err(|e| ChatRequestError {
            code: "council_user_defaults_failed",
            message: format!("failed to resolve council user tool defaults: {e}"),
        })?;
    Ok(CouncilDispatchParams {
        parent_agent_id: session.agent_id,
        parent_permissions: session.agent_permissions.clone(),
        parent_tool_permissions: session.tool_permissions.clone(),
        user_tool_defaults,
        originating_user_id: session.user_id.clone(),
        parent_model_id: session.model.clone(),
        chat_runs: registry,
        dispatcher,
        mechanism,
    })
}

/// Drive a council: wait for the parent session to be ready, dispatch
/// every member in parallel as a council-tagged subagent, then inject a
/// synthesis turn built from their answers.
async fn run_council_coordinator(coordinator: CouncilCoordinator) {
    let CouncilCoordinator {
        handle,
        members,
        mechanism,
        run_kind,
        query,
        run_id,
        shutdown,
        params,
    } = coordinator;

    // Wait for the parent driver's `SessionReady` before dispatching so
    // the parent identity is registered in the scheduler before members
    // spawn off it (otherwise they fall back to a bare config and the
    // router buckets them as anonymous traffic). Bounded so a stuck
    // bootstrap never wedges the coordinator.
    wait_for_session_ready(&handle.events, &shutdown).await;
    if shutdown.is_cancelled() {
        return;
    }

    let answers = dispatch_members(
        &handle, &members, run_kind, &query, &run_id, &shutdown, &params,
    )
    .await;
    if shutdown.is_cancelled() {
        return;
    }

    // Inject the final turn. The parent model's normal text turn is the
    // combined/final answer the UI renders below the panel.
    let prompt = match run_kind {
        CouncilRunKind::Council => build_synthesis_prompt(&query, &answers, mechanism),
        CouncilRunKind::SecondOpinion => build_second_opinion_prompt(&query, &answers),
    };
    if handle
        .commands
        .send(InboundMessage::UserMessage(UserMessage {
            content: prompt,
            tool_hints: None,
            attachments: None,
        }))
        .await
        .is_err()
    {
        warn!(
            run_id = %run_id,
            "AURA Council: parent run gone before the synthesis turn could start"
        );
    }
}

fn classify_council_members(members: &[CouncilMember]) -> Result<CouncilRunKind, ChatRequestError> {
    let role_count = members.iter().filter(|m| m.role.is_some()).count();
    if role_count == 0 {
        return Ok(CouncilRunKind::Council);
    }
    if members.first().and_then(|m| m.role) != Some(CouncilMemberRole::Aggregator) {
        return Err(ChatRequestError {
            code: "invalid_second_opinion_request",
            message: "Second Opinion requires member 0 to have role `aggregator`".to_string(),
        });
    }
    if members.len() < 2 {
        return Err(ChatRequestError {
            code: "second_opinion_no_references",
            message: "Second Opinion requires at least one reference member".to_string(),
        });
    }
    for (index, member) in members.iter().enumerate().skip(1) {
        if member.role != Some(CouncilMemberRole::Reference) {
            return Err(ChatRequestError {
                code: "invalid_second_opinion_request",
                message: format!("Second Opinion member {index} must have role `reference`"),
            });
        }
    }
    Ok(CouncilRunKind::SecondOpinion)
}

/// Resolve the council member cap from `AURA_COUNCIL_MAX_MEMBERS`,
/// falling back to [`DEFAULT_COUNCIL_MAX_MEMBERS`] when unset / invalid /
/// zero.
fn council_max_members() -> usize {
    std::env::var("AURA_COUNCIL_MAX_MEMBERS")
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(DEFAULT_COUNCIL_MAX_MEMBERS)
}

/// Silently truncate members beyond `max` (logging a warning). Keeps the
/// first `max` (so `members[0]`, the synthesizer, always survives).
fn truncate_members(mut members: Vec<CouncilMember>, max: usize) -> Vec<CouncilMember> {
    if members.len() > max {
        warn!(
            requested = members.len(),
            max, "AURA Council member count exceeds cap; truncating extras"
        );
        members.truncate(max);
    }
    members
}

/// The user's query a council fans out = the most recent `user` message
/// in the hydrated conversation. Empty when there is none.
fn latest_user_query(messages: &[ConversationMessage]) -> String {
    messages
        .iter()
        .rev()
        .find(|m| m.role == "user")
        .map(|m| m.content.clone())
        .unwrap_or_default()
}

/// Poll the parent run's replay history for `SessionReady`, bailing on
/// shutdown or after a bounded number of attempts.
async fn wait_for_session_ready(events: &Arc<ChatEventChannel>, shutdown: &CancellationToken) {
    for _ in 0..200 {
        if shutdown.is_cancelled() {
            return;
        }
        if events
            .subscribe()
            .history
            .iter()
            .any(|m| matches!(m, OutboundMessage::SessionReady(_)))
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[cfg(test)]
mod tests;
