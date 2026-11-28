//! After a restart: who carries on, and how. The peers are told first,
//! then each turn the restart cut short is followed again on its cloud
//! machine or resumed here with a prompt that names what it still owes,
//! then every idle agent that owes reports is asked to carry on, and
//! last the restored queues start.
//!
//! # Invariants
//!
//! * Only channel members, coordinators and agents that owe reports
//!   resume on their own. Any other agent whose turn the quit cut short
//!   (the user's own chat) is told so in its chat instead: the user may
//!   have quit to stop it.
//! * Agents the user parked stay parked.
//! * A resume prompt names only reports whose message the agent has
//!   seen: one still queued or undelivered starts its own turn, which
//!   answers it.

use super::queue_items::dropped_images_note;
use super::*;
use crate::event_loop::handlers::system_message;
use file::SavedRunning;
use std::collections::HashSet;

/// What a chat says of a turn the quit cut short that doesn't resume on
/// its own.
const CUT_SHORT: &str = "Its turn was cut short when the app quit. Send a message to carry on.";

/// Once the loop has started, the cloud hub up: carry on with what the
/// restart cut short ([`Resume`]), then start the restored queues.
pub(in crate::event_loop) async fn resume_saved(state: &mut LoopState<'_>, resume: Resume) {
    let Resume {
        queued,
        running,
        images_dropped,
    } = resume;
    // Routing between members needs their peers before anything runs.
    crate::event_loop::cloud::refresh_reach(state).await;
    let mut resumed = HashSet::new();
    for (agent, saved) in running {
        if resumed.insert(agent) && !is_busy(state, agent) {
            resume_turn(state, agent, saved).await;
        }
    }
    let owing: Vec<AgentId> = state
        .hub
        .inbox
        .owing()
        .into_iter()
        .map(|(a, _)| a)
        .collect();
    for agent in owing {
        let waits = state.queued.len(agent) > 0;
        if !waits && resumed.insert(agent) && !is_busy(state, agent) {
            carry_on(state, agent, false).await;
        }
    }
    for (agent, dropped) in images_dropped {
        note(state, agent, dropped_images_note(dropped)).await;
    }
    for agent in queued {
        if !is_busy(state, agent) {
            crate::event_loop::start_next(state, agent).await;
        }
        crate::event_loop::hud::send_queued(state, agent).await;
    }
}

/// `agent`'s turn was under way when the app quit, as `saved` says:
/// follow it again on its machine, else have it carry on, else say so.
async fn resume_turn(state: &mut LoopState<'_>, agent: AgentId, saved: SavedRunning) {
    // Parked, or its factory work is queued again and starts it.
    if lock(&state.hub.inbox.state).parked.contains(&agent)
        || state.hub.factory_flows.resumes_after_restart(agent)
    {
        return;
    }
    if let Some(run) = saved.run.filter(|_| saved.cloud) {
        let resume = resume_prompt_for(state, agent, true);
        if crate::event_loop::cloud::reattach::reattach(state, agent, run, resume).await {
            return;
        }
    }
    if resumes_on_its_own(state, agent) {
        carry_on(state, agent, true).await;
    } else {
        note(state, agent, CUT_SHORT.to_string()).await;
    }
}

/// Ask `agent` to carry on, its turn `cut_short` by the restart or not,
/// naming what it still owes.
async fn carry_on(state: &mut LoopState<'_>, agent: AgentId, cut_short: bool) {
    let prompt = resume_prompt_for(state, agent, cut_short);
    info!(%agent, cut_short, "After the restart: asking it to carry on");
    crate::event_loop::delivery::deliver(
        state,
        agent,
        crate::event_loop::steering::SteerSource::Harness,
        prompt,
        false,
    )
    .await;
}

/// The prompt asking `agent` to carry on after the restart, naming the
/// reports it owes whose messages it has seen: not those still in its
/// queue or on their way to it.
pub(in crate::event_loop) fn resume_prompt_for(
    state: &LoopState<'_>,
    agent: AgentId,
    cut_short: bool,
) -> String {
    let mut pending: Vec<String> = state
        .queued
        .items()
        .into_iter()
        .filter(|(queued, _)| *queued == agent)
        .filter_map(|(_, work)| match work {
            Work::Prompt { prompt, .. } => Some(prompt.text.clone()),
            _ => None,
        })
        .collect();
    let to = agent.to_hex();
    pending.extend(
        lock(&state.hub.inbox.saved.outbox)
            .iter()
            .filter(|saved| saved.to == to)
            .map(|saved| saved.prompt.clone()),
    );
    let asks: Vec<(String, String)> = state
        .hub
        .inbox
        .owing()
        .into_iter()
        .filter(|(owing, _)| *owing == agent)
        .flat_map(|(_, asks)| asks)
        .filter(|(_, marker)| !pending.iter().any(|text| text.contains(marker)))
        .collect();
    resume_prompt(&asks, cut_short)
}

/// The prompt asking an agent to carry on after the app restarted, its
/// turn `cut_short` or not, owing `asks` (subject and marker each).
pub(super) fn resume_prompt(asks: &[(String, String)], cut_short: bool) -> String {
    let mut text = if cut_short {
        String::from(
            "The app restarted while you were working, which cut your turn short. Carry on where \
             you left off.",
        )
    } else {
        String::from("The app restarted while you still owed reports. Carry on where you left off.")
    };
    if !asks.is_empty() {
        text.push_str("\n\nYou still owe these reports:\n");
        for (subject, marker) in asks {
            text.push_str(&format!("- {marker}\"{subject}\"\n"));
        }
        text.push_str(
            "\nFinish that work, then end your turn with your report: it goes back on its own. If \
             you are blocked, end your turn saying what blocks you.",
        );
    }
    text
}

/// Whether `agent` carries on after a restart without being asked: a
/// channel's member or coordinator, or an agent that owes reports.
fn resumes_on_its_own(state: &LoopState<'_>, agent: AgentId) -> bool {
    lock(&state.hub.channel_members).contains(&agent)
        || crate::event_loop::channels::is_coordinator_agent(&state.hub, agent)
        || state.hub.inbox.owed_by(agent) > 0
}

/// Whether `agent` has work running, here or in the cloud, or moves.
fn is_busy(state: &LoopState<'_>, agent: AgentId) -> bool {
    state.hub.is_running(agent) || state.cloud.is_running(agent) || state.cloud.is_moving(agent)
}

/// Say `text` in `agent`'s chat.
async fn note(state: &LoopState<'_>, agent: AgentId, text: String) {
    crate::event_loop::agent_events::AgentUi::new(state.commands.clone(), agent)
        .send(system_message(text))
        .await;
}
