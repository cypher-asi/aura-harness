//! Where the event loop applies the software factory's flow
//! ([`super::flow`]) to a channel member's turns, local or cloud.

use super::flow::{Next, TurnEnd};
use super::observe::{self, ObservedPush};
use crate::event_loop::agent_events::AgentUi;
use crate::event_loop::agents::AgentSession;
use crate::event_loop::handlers::system_message;
use crate::event_loop::notices::Notice;
use crate::event_loop::subagents::{lock, AgentHub};
use crate::event_loop::turns::{Prompt, Work};
use crate::event_loop::{modes, LoopState, Permissions};
use aura_context_prompts::model_messages::build_plan::build_plan_prompt;
use aura_core_types::AgentId;
use aura_exec_merge_queue::{QueueConfig, QueuedPush, RepoPaths};
use aura_exec_tools::git_tool::{git_ship_impl, PushPolicy, PushTarget, ShipRequest};
use aura_exec_tools::plan_tools::plan_path_from_output;
use aura_model::{ContentBlock, Message, Role, ToolResultContent};
use aura_ui::PermissionMode;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tracing::info;

/// What a cloud agent is told once it built its plan without shipping.
pub(crate) const SHIP_PROMPT: &str = "Your plan is built. Ship it now: call git_ship with a \
    short commit message. It commits every change, pulls main, pushes your branch and puts it in \
    the merge queue. Then end your turn with your report.";

/// Longest a git step of the hub's own `git_ship` may take.
const SHIP_TIMEOUT: Duration = Duration::from_secs(120);

/// Agents whose branch names the hub ships on its own: branches only an
/// agent works on.
const AGENT_BRANCH_PREFIX: &str = "aura/";

/// A task reached `agent` through its inbox: a channel member plans it
/// first.
pub(crate) fn on_assignment(state: &LoopState<'_>, agent: AgentId) {
    if !lock(&state.hub.channel_members).contains(&agent) {
        return;
    }
    let definition = lock(&state.hub.registry)
        .find(agent)
        .and_then(|found| found.definition.clone());
    if crate::event_loop::channels::is_coordinator(definition.as_deref()) {
        return;
    }
    let before = state
        .idle
        .get(&agent)
        .and_then(|session| session.active_plan.clone());
    info!(%agent, "Software factory: the task starts with a plan");
    state.hub.factory_flows.start(agent, before);
}

/// Before a local turn of `session`: an agent whose task starts with a
/// plan works in plan mode.
pub(crate) async fn plan_first(state: &LoopState<'_>, session: &AgentSession) {
    let agent = session.agent_id();
    if state.hub.factory_flows.is_planning(agent)
        && state.hub.mode_of(agent) != PermissionMode::Plan
    {
        modes::switch_mode(state, agent, &session.kernel, PermissionMode::Plan).await;
    }
}

/// Before a cloud turn of `agent`: plan mode, which the machine gets as
/// its plan-mode prompt.
pub(crate) fn plan_first_in_cloud(hub: &AgentHub, agent: AgentId) {
    if hub.factory_flows.is_planning(agent) && hub.mode_of(agent) != PermissionMode::Plan {
        let approval = hub.permissions_of(agent).approval;
        hub.set_permissions(agent, Permissions::new(PermissionMode::Plan, approval));
    }
}

/// After a local turn of `agent` that ended `ok` or not: start what the
/// flow does next. Returns whether the turn's answer waits for it.
pub(crate) fn after_local_turn(state: &mut LoopState<'_>, agent: AgentId, ok: bool) -> bool {
    let Some(session) = state.idle.get(&agent) else {
        return false;
    };
    let plan = session.active_plan.clone();
    let folder = session.kernel.agent_workspace();
    let end = TurnEnd {
        ok,
        plan: plan.as_deref(),
        shipped: false,
        cloud: false,
    };
    let next = state.hub.factory_flows.turn_ended(agent, end);
    match &next {
        Next::Build(path) => {
            info!(%agent, %path, "Software factory: plan written; building it");
            state.queued.push_front(
                agent,
                Work::Build {
                    path: Some(path.clone()),
                },
            );
        }
        Next::Ship => {
            let ship = LocalShip {
                hub: Arc::clone(&state.hub),
                ui: AgentUi::new(state.commands.clone(), agent),
                agent,
                folder,
                message: ship_message(plan.as_deref()),
            };
            tokio::spawn(ship.run());
        }
        Next::Normal | Next::AskToShip => {}
    }
    next.defers_answer()
}

/// After a cloud turn of `agent` that ended `ok` or not with `messages`,
/// its conversation: start what the flow does next. Returns whether the
/// turn's answer waits for it.
pub(crate) fn after_cloud_turn(
    state: &mut LoopState<'_>,
    agent: AgentId,
    ok: bool,
    messages: &[Message],
) -> bool {
    let results = turn_results(messages);
    let plan = results.iter().find_map(|text| plan_path_from_output(text));
    let shipped = results
        .iter()
        .any(|text| QueuedPush::from_tool_output(text).is_some());
    let end = TurnEnd {
        ok,
        plan,
        shipped,
        cloud: true,
    };
    let next = state.hub.factory_flows.turn_ended(agent, end);
    let prompt = match &next {
        Next::Build(path) => {
            let back = state.hub.exit_plan_mode(agent);
            let approval = state.hub.permissions_of(agent).approval;
            state
                .hub
                .set_permissions(agent, Permissions::new(back, approval));
            Some(Prompt::build(build_plan_prompt(path)))
        }
        Next::AskToShip => Some(Prompt::build(SHIP_PROMPT.to_string())),
        Next::Normal | Next::Ship => None,
    };
    if let Some(prompt) = prompt {
        info!(%agent, ?next, "Software factory: the cloud agent carries on");
        state.queued.push_front(
            agent,
            Work::Prompt {
                prompt,
                shown: false,
            },
        );
    }
    next.defers_answer()
}

/// The text of every tool result since the last prompt in `messages`.
pub(crate) fn turn_results(messages: &[Message]) -> Vec<String> {
    let start = messages
        .iter()
        .rposition(|message| {
            message.role == Role::User
                && message
                    .content
                    .iter()
                    .any(|block| matches!(block, ContentBlock::Text { .. }))
        })
        .unwrap_or(0);
    messages
        .get(start..)
        .unwrap_or_default()
        .iter()
        .flat_map(|message| message.content.iter())
        .filter_map(|block| match block {
            ContentBlock::ToolResult {
                content,
                is_error: false,
                ..
            } => Some(match content {
                ToolResultContent::Text(text) => text.clone(),
                ToolResultContent::Json(value) => value.to_string(),
            }),
            _ => None,
        })
        .collect()
}

/// The commit message of the hub's ship after the plan at `plan`.
fn ship_message(plan: Option<&str>) -> String {
    let name = plan
        .and_then(|path| std::path::Path::new(path).file_name())
        .and_then(|name| name.to_str())
        .map(|name| name.trim_end_matches(".plan.md").replace(['-', '_'], " "));
    match name {
        Some(name) if !name.trim().is_empty() => format!("Build plan: {}", name.trim()),
        _ => "Finish task".to_string(),
    }
}

/// The hub shipping a local agent's workspace after it built its plan.
struct LocalShip {
    hub: Arc<AgentHub>,
    ui: AgentUi,
    agent: AgentId,
    folder: PathBuf,
    message: String,
}

impl LocalShip {
    async fn run(self) {
        match self.ship().await {
            Ok(Some(push)) => {
                let line = format!(
                    "Shipped {} at {}.",
                    push.branch,
                    aura_exec_merge_queue::short_sha(&push.head)
                );
                self.ui.send(system_message(line)).await;
                observe::report(ObservedPush {
                    agent: self.agent,
                    push,
                });
            }
            Ok(None) => {}
            Err(why) => {
                let notice = Notice {
                    to: self.agent,
                    shown: format!("Could not ship: {why}"),
                    prompt: format!(
                        "Shipping your finished work failed: {why}\n\nFix it, then call git_ship."
                    ),
                    inbox: None,
                };
                let _ = self.hub.notices.send(notice);
            }
        }
    }

    /// Ship the workspace when it is on an agent's own branch; `None`
    /// (with a line in the chat) when it is on another.
    async fn ship(&self) -> Result<Option<QueuedPush>, String> {
        let timeout = Duration::from_secs(15);
        let branch = aura_exec_merge_queue::git::ok(
            &self.folder,
            &["rev-parse", "--abbrev-ref", "HEAD"],
            timeout,
        )
        .await
        .map_err(|e| e.to_string())?;
        if !branch.starts_with(AGENT_BRANCH_PREFIX) {
            let line = format!(
                "Not shipping on its own: the work is on `{branch}`, not an agent's own \
                 `{AGENT_BRANCH_PREFIX}*` branch. Call git_ship from a branch of its own."
            );
            self.ui.send(system_message(line)).await;
            return Ok(None);
        }
        let repo = RepoPaths::of(&self.folder)
            .await
            .map_err(|e| e.to_string())?;
        let top = repo.top.clone();
        let target = tokio::task::spawn_blocking(move || QueueConfig::load(&top))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?
            .target;
        let request = ShipRequest {
            message: &self.message,
            target: &target,
            push: PushTarget::default(),
            timeout: SHIP_TIMEOUT,
            push_policy: PushPolicy::from_config(&aura_exec_tools::ToolConfig::default()),
        };
        let shipped = git_ship_impl(&self.folder, request)
            .await
            .map_err(|e| e.to_string())?;
        Ok(Some(shipped.marker()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(role: Role, text: &str) -> Message {
        Message {
            role,
            content: vec![ContentBlock::Text {
                text: text.to_string(),
            }],
        }
    }

    fn result(text: &str, is_error: bool) -> Message {
        Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "t".to_string(),
                content: ToolResultContent::Text(text.to_string()),
                is_error,
            }],
        }
    }

    #[test]
    fn only_the_last_turns_good_results_count() {
        let messages = vec![
            text(Role::User, "first task"),
            result("old", false),
            text(Role::User, "second task"),
            text(Role::Assistant, "on it"),
            result("new", false),
            result("broken", true),
        ];
        assert_eq!(turn_results(&messages), ["new"]);
    }

    #[test]
    fn the_ship_commit_is_named_after_the_plan() {
        assert_eq!(
            ship_message(Some(".aura/plans/merge-queue_tab.plan.md")),
            "Build plan: merge queue tab"
        );
        assert_eq!(ship_message(None), "Finish task");
    }
}
