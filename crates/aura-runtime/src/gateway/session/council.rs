//! AURA Council orchestrator.
//!
//! A council run convenes `members` model seats to answer the user's
//! question, then synthesizes one combined response. It is deliberately
//! built on the SAME canonical subagent path the `task` tool uses for
//! an ordinary "spawn N subagents in parallel, then summarize" turn,
//! rather than a bespoke hand-rolled fan-out. That makes the feature
//! reuse existing, already-working machinery end to end:
//!
//! - The PARENT run is created + registered through the SAME
//!   [`super::chat_run::spawn_chat_run`] path a `POST /v1/run` chat run
//!   uses (so `WS /stream/:run_id` attaches non-destructively), prepared
//!   with `members[0]`'s model — the synthesizer.
//! - Once the parent session is ready, the orchestrator injects ONE
//!   coordinator `user_message` instructing the synthesizer model to
//!   call the `task` tool once per member IN PARALLEL — each with that
//!   member's model id as the `model` override and the user's question
//!   verbatim as the `prompt` — and then to synthesize the members'
//!   answers into one combined response.
//! - Every `task` call therefore flows through the normal agent loop +
//!   [`super::subagent_stream::RuntimeSubagentObservabilityHook`] (wired
//!   for every per-turn chat build in [`super::helpers`]). So each
//!   member is announced as a `SubagentSpawned` carrying a REAL
//!   `parent_tool_use_id` (the model's tool-use id), streams live on its
//!   own child run, and renders as the standard subagent thread card.
//!   The synthesized answer is the parent model's normal text turn after
//!   the members return — no synthetic frames, no separate synthesis
//!   injection path.
//!
//! Because the members are real model-issued `task` calls, the council
//! reuses the exact "parallel task + synthesize" behavior that already
//! works for an arbitrary chat prompt; the only council-specific piece
//! is the coordinator instruction that names the per-member models.
//!
//! Cancellation: the parent run's driver cancels in-flight `task`
//! children on `shutdown` (each child token is forked from the parent
//! turn token by the observability hook), so a single
//! `POST /v1/run/:id/stop` (or a parent `Cancel`) aborts every in-flight
//! member AND the synthesizer.

use std::sync::Arc;
use std::time::Duration;

use aura_protocol::{ConversationMessage, CouncilMember, RuntimeRequest, RuntimeRequestType};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

use super::chat_run::{ChatEventChannel, ChatRunHandle};
use super::helpers::{prepare_chat_session, ChatRequestError};
use super::WsContext;
use crate::protocol::{InboundMessage, OutboundMessage, UserMessage};

/// Default cap on council members when `AURA_COUNCIL_MAX_MEMBERS` is
/// unset / unparsable. Extra members beyond the cap are silently
/// truncated (with a warning).
const DEFAULT_COUNCIL_MAX_MEMBERS: usize = 4;

/// Bundled subagent kind each council member runs as. `general_purpose`
/// is the full multi-step agent loop (read/write/run tools), so a member
/// answers the query like a real agent rather than a read-only explorer.
const COUNCIL_MEMBER_KIND: &str = "general_purpose";

/// Everything the detached coordinator task needs to kick a council off
/// once the parent run is registered + ready.
struct CouncilCoordinator {
    handle: Arc<ChatRunHandle>,
    members: Vec<CouncilMember>,
    query: String,
    run_id: String,
    shutdown: CancellationToken,
}

/// Start an AURA Council run.
///
/// Mirrors [`super::chat_run::spawn_chat_run`]'s setup to create +
/// register the PARENT run (hosting the synthesizer, `members[0]`), then
/// detaches a coordinator task that — once the session is ready —
/// injects a single instruction turn telling the synthesizer model to
/// fan the members out as parallel `task` calls and synthesize their
/// answers. Returns the registered `run_id` (the caller turns it into
/// `{ run_id, event_stream_url }`).
///
/// Errors mirror [`prepare_chat_session`] plus council-specific
/// validation (`council_no_members`, `invalid_council_request`).
pub(crate) async fn start_council_run(
    req: RuntimeRequest,
    ctx: WsContext,
) -> Result<String, ChatRequestError> {
    let (members, conversation_messages) = match req.r#type {
        RuntimeRequestType::Council {
            ref members,
            ref conversation_messages,
        } => (members.clone(), conversation_messages.clone()),
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
    let members = truncate_members(members, council_max_members());
    let query = latest_user_query(&conversation_messages);

    // The PARENT run hosts the synthesizer: prepare it with members[0]'s
    // model so the synthesis turn (and the coordinator turn that issues
    // the `task` calls) runs on the first model.
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

    let run_id = Uuid::new_v4().to_string();
    // Register + drive the parent run through the shared chat-run path.
    // The chat driver wires the subagent observability hook for every
    // per-turn build, so the `task` calls the coordinator turn triggers
    // are announced + streamed exactly like any other parallel-`task`
    // chat turn.
    let handle = super::spawn_chat_run(session, ctx, run_id.clone(), registry);
    let shutdown = handle.shutdown.clone();

    info!(
        run_id = %run_id,
        member_count = members.len(),
        "AURA Council run started"
    );

    tokio::spawn(run_council_coordinator(CouncilCoordinator {
        handle,
        members,
        query,
        run_id: run_id.clone(),
        shutdown,
    }));

    Ok(run_id)
}

/// Drive a council: wait for the parent session to be ready, then inject
/// the single coordinator instruction turn. The synthesizer model owns
/// the rest — it issues the parallel `task` calls and synthesizes their
/// results — so there is no hand-rolled fan-out or synthesis injection
/// here.
async fn run_council_coordinator(coordinator: CouncilCoordinator) {
    let CouncilCoordinator {
        handle,
        members,
        query,
        run_id,
        shutdown,
    } = coordinator;

    // Wait for the parent driver's `SessionReady` before injecting the
    // coordinator turn so the parent identity is registered in the
    // scheduler before the synthesizer spawns members off it (otherwise
    // members fall back to a bare config and the router buckets them as
    // anonymous traffic). Bounded so a stuck bootstrap never wedges the
    // coordinator.
    wait_for_session_ready(&handle.events, &shutdown).await;
    if shutdown.is_cancelled() {
        return;
    }

    let prompt = build_coordinator_prompt(&query, &members);
    if handle
        .commands
        .send(InboundMessage::UserMessage(UserMessage {
            content: prompt,
            // Steer the kickoff turn toward the `task` tool so the
            // synthesizer reaches for it first. `tool_hints` only scopes
            // which tools are visible (tool_choice stays auto) and
            // synthesis itself is plain text, so this never blocks the
            // follow-up synthesis turn.
            tool_hints: Some(vec!["task".to_string()]),
            attachments: None,
        }))
        .await
        .is_err()
    {
        warn!(
            run_id = %run_id,
            "AURA Council: parent run gone before the coordinator turn could start"
        );
    }
}

/// Build the coordinator instruction turn: embed the user's question,
/// name every member's model id for the per-call `model` override, and
/// direct the synthesizer to (1) spawn one parallel `task` per member,
/// then (2) synthesize their answers. This is the only council-specific
/// logic — everything downstream is the canonical parallel-`task` path.
fn build_coordinator_prompt(query: &str, members: &[CouncilMember]) -> String {
    let n = members.len();
    let mut prompt = String::new();
    prompt.push_str(&format!(
        "You are the AURA Council coordinator. Convene a council of {n} member models to answer \
         the user's question, then synthesize their answers into one combined response.\n\n"
    ));

    prompt.push_str("## User question\n\n");
    prompt.push_str(query.trim());

    prompt.push_str("\n\n## Step 1 — fan out the members (do this FIRST)\n\n");
    prompt.push_str(&format!(
        "In a SINGLE assistant message, call the `task` tool {n} times IN PARALLEL — one call per \
         council member listed below. Issue all {n} calls together so the members run \
         concurrently; do NOT call `task` sequentially and do NOT answer the question yourself \
         first. For EVERY call set:\n\
         - `subagent_type`: \"{COUNCIL_MEMBER_KIND}\"\n\
         - `prompt`: the user's question above, verbatim\n\
         - `model`: the member's model id below (copy it EXACTLY)\n\n"
    ));
    prompt.push_str("Council members:\n");
    for (idx, member) in members.iter().enumerate() {
        let model = member.model.id.as_deref().unwrap_or("(default model)");
        prompt.push_str(&format!("- Member {idx}: model `{model}`\n"));
    }

    prompt.push_str("\n## Step 2 — synthesize\n\n");
    prompt.push_str(
        "After ALL members return, write ONE combined answer. Explicitly call out where the \
         members AGREE and where they DISAGREE; when they disagree, weigh the trade-offs and \
         state your single best recommendation. Integrate their answers — do not merely list \
         them.",
    );
    prompt
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
mod tests {
    use super::*;
    use aura_protocol::ModelSelection;

    fn test_members(model_ids: &[&str]) -> Vec<CouncilMember> {
        model_ids
            .iter()
            .enumerate()
            .map(|(i, id)| CouncilMember {
                id: i.to_string(),
                model: ModelSelection {
                    id: Some((*id).to_string()),
                    ..ModelSelection::default()
                },
            })
            .collect()
    }

    #[test]
    fn truncate_members_caps_and_keeps_synthesizer() {
        let members = test_members(&["a", "b", "c", "d", "e", "f"]);
        let capped = truncate_members(members, 4);
        let ids: Vec<String> = capped
            .iter()
            .map(|m| m.model.id.clone().unwrap_or_default())
            .collect();
        assert_eq!(ids, vec!["a", "b", "c", "d"]);
    }

    #[test]
    fn truncate_members_keeps_all_under_cap() {
        let members = test_members(&["a", "b"]);
        assert_eq!(truncate_members(members, 4).len(), 2);
    }

    #[test]
    fn latest_user_query_returns_most_recent_user_message() {
        let messages = vec![
            ConversationMessage {
                role: "user".to_string(),
                content: "first".to_string(),
            },
            ConversationMessage {
                role: "assistant".to_string(),
                content: "reply".to_string(),
            },
            ConversationMessage {
                role: "user".to_string(),
                content: "latest question".to_string(),
            },
        ];
        assert_eq!(latest_user_query(&messages), "latest question");
        assert_eq!(latest_user_query(&[]), "");
    }

    /// The coordinator prompt must embed the question, name every
    /// member's model id (so the synthesizer can override per call), and
    /// steer the canonical "parallel `task`, then synthesize" behavior.
    #[test]
    fn coordinator_prompt_lists_each_member_model_and_demands_parallel_tasks() {
        let members = test_members(&["model-a", "model-b", "model-c"]);
        let prompt = build_coordinator_prompt("what is rust?", &members);

        assert!(prompt.contains("what is rust?"), "embeds the question");
        for model in ["model-a", "model-b", "model-c"] {
            assert!(prompt.contains(model), "lists member model {model}");
        }
        assert!(prompt.contains("`task`"), "names the task tool");
        assert!(
            prompt.to_lowercase().contains("parallel"),
            "demands parallel fan-out"
        );
        assert!(
            prompt.to_lowercase().contains("synthesize"),
            "asks for synthesis"
        );
        assert!(
            prompt.contains(COUNCIL_MEMBER_KIND),
            "names the member subagent kind"
        );
    }

    #[test]
    fn coordinator_prompt_counts_members() {
        let two = build_coordinator_prompt("q", &test_members(&["x", "y"]));
        assert!(two.contains("council of 2 member"));
        assert!(two.contains("Member 0"));
        assert!(two.contains("Member 1"));
        assert!(!two.contains("Member 2"));
    }
}
