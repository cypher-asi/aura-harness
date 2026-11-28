//! Where the event loop applies the software factory's flow
//! ([`super::flow`]) to a channel member's turns, local or cloud.
//!
//! Shipping always goes through the agent's own kernel: the hub never runs
//! git for it, it asks the agent to call `git_ship` ([`SHIP_PROMPT`]).

use super::flow::{Next, TurnEnd};
use super::observe::{self, ObservedPush};
use super::turn;
use crate::event_loop::agent_events::AgentUi;
use crate::event_loop::agents::AgentSession;
use crate::event_loop::handlers::system_message;
use crate::event_loop::subagents::{lock, AgentHub};
use crate::event_loop::turns::{Prompt, Request, Work};
use crate::event_loop::{inbox, modes, LoopState, Permissions};
use aura_context_prompts::model_messages::build_plan::build_plan_prompt;
use aura_core_types::AgentId;
use aura_model::Message;
use aura_ui::PermissionMode;
use std::path::Path;
use std::sync::Arc;
use tracing::info;

/// What an agent is told once it built its plan without shipping.
pub(crate) const SHIP_PROMPT: &str = "Your plan is built. Ship it now: call git_ship with a \
    short commit message. It commits every change, pulls main, pushes your branch and puts it in \
    the merge queue. Then end your turn with your report.";

/// What a cloud agent is told instead when its machine's node offers no
/// `git_ship` (one from before the merge queue, until the hub updates
/// it). The hub queues the push itself ([`super::observe::push_of`]).
pub(crate) const FALLBACK_SHIP_PROMPT: &str = "Your plan is built. Ship it now: call \
    git_commit_push with a short commit message to commit every change and push your branch; the \
    hub puts the push in the merge queue, which merges it into main once the build and tests \
    pass. Do not open a pull request and never push main. Then end your turn with your report.";

/// The ship prompt for an agent whose node offers `git_ship` or not.
#[must_use]
pub(crate) const fn ship_prompt(offers_ship: bool) -> &'static str {
    if offers_ship {
        SHIP_PROMPT
    } else {
        FALLBACK_SHIP_PROMPT
    }
}

/// A task reached `agent` through its inbox: a channel member plans it
/// first, once its work starts.
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
    info!(%agent, "Software factory: the task starts with a plan");
    state.hub.factory_flows.assign(agent);
}

/// Before a local prompt of `session`: when its task plans, it works in
/// plan mode. Returns whether this turn plans.
pub(crate) async fn plan_first(state: &LoopState<'_>, session: &AgentSession) -> bool {
    let agent = session.agent_id();
    if !state.hub.factory_flows.begin(agent) {
        return false;
    }
    if state.hub.mode_of(agent) != PermissionMode::Plan {
        modes::switch_mode(state, agent, &session.kernel, PermissionMode::Plan).await;
    }
    true
}

/// Before a cloud turn of `agent`: plan mode, which the machine gets as
/// its plan-mode prompt, when its task plans.
pub(crate) fn plan_first_in_cloud(hub: &AgentHub, agent: AgentId) {
    if hub.factory_flows.begin(agent) && hub.mode_of(agent) != PermissionMode::Plan {
        let approval = hub.permissions_of(agent).approval;
        hub.set_permissions(agent, Permissions::new(PermissionMode::Plan, approval));
    }
}

/// The build of the plan `session` wrote could not start: the flow ends,
/// and what the planning turn was asked is answered now.
pub(crate) fn build_gave_up(state: &LoopState<'_>, session: &AgentSession) {
    let agent = session.agent_id();
    if state.hub.factory_flows.stage(agent) == Some(super::flow::Stage::Building) {
        info!(%agent, "Software factory: the plan could not be built; answering");
        state.hub.factory_flows.leave(agent);
        inbox::answer_seen(&state.hub, agent, &session.messages);
    }
}

/// How a turn ended, for the flow.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TurnState {
    /// It ended well (not failed or stopped)
    pub ok: bool,
    /// The user stopped it to carry on: the flow stays where it was, and
    /// the user's next turn continues it, as that turn answers the report
    /// still owed.
    pub interrupted: bool,
}

/// After a local turn of `agent` that ended as `how`: start what the
/// flow does next. Returns whether the turn's answer waits for it.
pub(crate) async fn after_local_turn(
    state: &mut LoopState<'_>,
    agent: AgentId,
    how: TurnState,
) -> bool {
    let TurnState { ok, interrupted } = how;
    if interrupted && !ok {
        return false;
    }
    let Some(session) = state.idle.get(&agent) else {
        return false;
    };
    let prompt = state.hub.factory_flows.prompt_of(agent);
    let wrote = turn::wrote_plan(&session.messages, prompt.as_deref());
    let shipped = !turn::pushes(&session.messages, prompt.as_deref()).is_empty();
    let kernel = Arc::clone(&session.kernel);
    let folder = kernel.agent_workspace();
    let end = TurnEnd {
        ok,
        wrote_plan: wrote.as_deref(),
        shipped,
    };
    let next = state.hub.factory_flows.turn_ended(agent, end);
    let work = match &next {
        Next::Build(path) => Some(Work::Build {
            path: Some(path.clone()),
        }),
        Next::AskToShip if shares_folder(&state.hub, agent, &folder) => {
            state.hub.factory_flows.leave(agent);
            let line = "Built. Not shipped on its own: other members work in this folder, \
                        so a ship would commit their work too. Call git_ship when it is yours alone.";
            AgentUi::new(state.commands.clone(), agent)
                .send(system_message(line.to_string()))
                .await;
            return false;
        }
        // A local agent runs this aura, which has `git_ship`.
        Next::AskToShip => Some(ship_work(true)),
        Next::LeftPlanning => {
            if state.hub.mode_of(agent) == PermissionMode::Plan {
                let back = state.hub.exit_plan_mode(agent);
                modes::switch_mode(state, agent, &kernel, back).await;
            }
            None
        }
        Next::Normal => None,
    };
    if let Some(work) = work {
        info!(%agent, ?next, "Software factory: the agent carries on");
        state.queued.push_front(agent, work);
    }
    next.defers_answer()
}

/// After a cloud turn of `agent` that ended `ok` or not with `messages`,
/// its conversation: queue the pushes it made (again: the stream may have
/// lost one), then start what the flow does next. Returns whether the
/// turn's answer waits for it.
pub(crate) fn after_cloud_turn(
    state: &mut LoopState<'_>,
    agent: AgentId,
    how: TurnState,
    messages: &[Message],
) -> bool {
    let TurnState { ok, interrupted } = how;
    let prompt = state.hub.factory_flows.prompt_of(agent);
    let pushes = turn::pushes(messages, prompt.as_deref());
    let shipped = !pushes.is_empty();
    for push in pushes {
        observe::report(ObservedPush { agent, push });
    }
    if interrupted && !ok {
        return false;
    }
    let wrote = turn::wrote_plan(messages, prompt.as_deref());
    let end = TurnEnd {
        ok,
        wrote_plan: wrote.as_deref(),
        shipped,
    };
    let next = state.hub.factory_flows.turn_ended(agent, end);
    let work = match &next {
        Next::Build(path) => {
            leave_plan_mode(&state.hub, agent);
            Some(Work::Prompt {
                prompt: Prompt::build(build_plan_prompt(path)),
                shown: false,
            })
        }
        Next::AskToShip => {
            let offers = crate::event_loop::cloud::node_offers_ship(state, agent);
            Some(ship_work(offers))
        }
        Next::LeftPlanning => {
            leave_plan_mode(&state.hub, agent);
            None
        }
        Next::Normal => None,
    };
    if let Some(work) = work {
        info!(%agent, ?next, "Software factory: the cloud agent carries on");
        state.queued.push_front(agent, work);
    }
    next.defers_answer()
}

/// The turn asking an agent to ship what it built, with `git_ship` when
/// its node `offers_ship`, else with `git_commit_push`.
fn ship_work(offers_ship: bool) -> Work {
    Work::Prompt {
        prompt: Prompt::inbox(ship_prompt(offers_ship).to_string(), Request::Resume),
        shown: false,
    }
}

/// Put cloud agent `agent` back in the mode it had before plan mode.
fn leave_plan_mode(hub: &AgentHub, agent: AgentId) {
    if hub.mode_of(agent) == PermissionMode::Plan {
        let back = hub.exit_plan_mode(agent);
        let approval = hub.permissions_of(agent).approval;
        hub.set_permissions(agent, Permissions::new(back, approval));
    }
}

/// Whether another member of a channel, not its coordinator, works in
/// `folder` too.
fn shares_folder(hub: &AgentHub, agent: AgentId, folder: &Path) -> bool {
    let members: Vec<AgentId> = lock(&hub.channel_members).iter().copied().collect();
    let registry = lock(&hub.registry);
    members
        .into_iter()
        .filter(|other| *other != agent)
        .any(|other| {
            let coordinator = registry.find(other).is_some_and(|found| {
                crate::event_loop::channels::is_coordinator(found.definition.as_deref())
            });
            !coordinator && registry.project_folder(other).as_deref() == Some(folder)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_node_without_git_ship_is_asked_to_ship_with_git_commit_push() {
        let fallback = ship_prompt(false);
        assert!(fallback.contains("git_commit_push"), "{fallback}");
        assert!(
            !fallback.contains("git_ship"),
            "it has no git_ship: {fallback}"
        );
        assert!(fallback.contains("merge queue"));
        assert!(ship_prompt(true).contains("call git_ship"));
    }
}
