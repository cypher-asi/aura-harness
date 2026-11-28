//! `/plan` in a channel's chat. A coordinator can't read the code or write
//! a plan, so a new member of its channel plans instead, with its own chat
//! on screen. Its request is a task like any member's (see
//! [`crate::event_loop::factory`]): it plans it in plan mode, then builds
//! the plan and ships it to the merge queue on its own. Nobody has to say
//! "implement". The request is also kept as a plan post of the channel,
//! for its HISTORY ([`post_plan_request`]).

use super::*;
use crate::event_loop::agent_events::AgentUi;
use crate::event_loop::handlers::system_message;
use crate::event_loop::steering::SteerSource;
use crate::event_loop::{agents, deliver_with_images, modes};
use aura_ui::events::ImageAttachment;

/// The permanent agent a planner is an instance of: the default agent,
/// which can build the plan it writes.
pub(crate) const PLANNER_DEFINITION: &str = "engineer";

/// What the chat of a planner in a channel whose agents run in the cloud
/// says about where it works.
pub(crate) const PLANS_HERE: &str = "Plan mode holds only on this computer, so this planner \
     works here instead of on a cloud machine, and builds its plan here too.";

/// What the chat of a planner started without a request says.
pub(crate) const PLANNER_WAITS: &str = "Tell this planner what to plan. It writes the plan, \
     then builds it and ships it to the merge queue without waiting for approval.";

/// Whether `agent_id` is a channel's coordinator, which never plans
/// itself.
pub(crate) fn is_coordinator_agent(hub: &AgentHub, agent_id: AgentId) -> bool {
    lock(&hub.registry)
        .find(agent_id)
        .is_some_and(|agent| is_coordinator(agent.definition.as_deref()))
}

/// Plan `text` in a new member of the channel `coordinator` runs: add the
/// member, put its chat on screen, and give it `text` with `images` as
/// its task, which plans first in plan mode, then is built and shipped
/// without anyone asking. An empty `text` leaves it waiting in plan mode
/// for the request. Plan mode holds only on this computer, so in a
/// channel whose agents run in the cloud the planner stays here, and its
/// chat says so.
///
/// The channels list goes out before the member comes on screen: the UI
/// keeps a channel open only for one of its members, so it must know the
/// planner is one. What fails is shown as an error; a member that was
/// added stays in the channel.
pub(crate) async fn start_planner(
    state: &mut LoopState<'_>,
    coordinator: AgentId,
    text: String,
    images: Vec<ImageAttachment>,
) {
    let planner = match add_planner(&state.hub, coordinator).await {
        Ok(planner) => planner.agent_id(),
        Err(error) => {
            handlers::show_error(state, format!("Could not start a planner: {error:#}")).await;
            return;
        }
    };
    info!(
        coordinator = %coordinator.to_hex(),
        planner = %planner.to_hex(),
        "Planning in a new member of the channel"
    );
    state.hub.send_agent_list().await;
    agents::handle_select_agent(state, &planner.to_hex()).await;
    // A session that would not open was reported by the switch.
    let Some(kernel) = state.kernel_for(planner) else {
        return;
    };
    // A planner plans here, in plan mode, in any channel.
    lock(&state.hub.cloud_members).remove(&planner);
    if lock(&state.hub.cloud_starts).remove(&planner) {
        AgentUi::new(state.commands.clone(), planner)
            .send(system_message(PLANS_HERE.to_string()))
            .await;
    }
    state.hub.factory_flows.assign(planner);
    if text.trim().is_empty() {
        modes::enter_plan_mode(state, planner, &kernel, PLANNER_WAITS).await;
    } else {
        // Kept for the channel's HISTORY, which the planner's chat is not.
        // Its work switches it to plan mode as it starts.
        post_plan_request(state, coordinator, planner, &text).await;
        deliver_with_images(state, planner, SteerSource::User, text, images, false).await;
    }
}

/// Add a new member to the channel `coordinator` runs, an instance of
/// [`PLANNER_DEFINITION`] ([`add_agent`], on a blocking thread).
async fn add_planner(hub: &Arc<AgentHub>, coordinator: AgentId) -> anyhow::Result<RegisteredAgent> {
    if lock(&hub.definitions).get(PLANNER_DEFINITION).is_none() {
        bail!("there is no permanent agent `{PLANNER_DEFINITION}` to plan with");
    }
    let hub = Arc::clone(hub);
    tokio::task::spawn_blocking(move || add_agent(&hub, coordinator, PLANNER_DEFINITION))
        .await
        .context("the task adding the planner stopped")?
}
