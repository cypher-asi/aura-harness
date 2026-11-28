//! Prompts, commands and `/new` for cloud agents.

use super::forward::{forward, title_sink, Titles, TurnEnd, TurnExtras};
use super::*;
use crate::cloud::hub::TurnAsk;
use crate::event_loop::turns::Prompt;
use aura_agent::checkpoint::{Checkpoints, SessionCheckpoint};
use tokio::sync::mpsc;

/// Prompts and commands for a cloud agent, or for one being moved.
/// Returns the work when it is for the local kernel after all. Work for
/// a moving agent, or a cloud agent with a turn running, waits in its
/// queue.
pub(in crate::event_loop) async fn submit(
    state: &mut LoopState<'_>,
    agent: AgentId,
    work: Work,
) -> Option<Work> {
    if let Some(root) = state.cloud.machine_root(agent) {
        let note = spawned_on_machine(state, agent, root);
        system(state, agent, note).await;
        return None;
    }
    if state.cloud.is_moving(agent) {
        info!(%agent, "Agent is moving between machines; queueing work");
        state.queued.push_back(agent, work);
        crate::event_loop::hud::send_queued(state, agent).await;
        return None;
    }
    if state.cloud.take_stale(agent) {
        state.idle.remove(&agent);
    }
    if !state.cloud.is_cloud(agent) {
        return Some(work);
    }
    let hub = state.cloud.hub()?;
    if let Some(refusal) = foreign_owner(state, &hub, agent) {
        system(state, agent, refusal).await;
        return None;
    }
    if state.cloud.is_running(agent) {
        info!(%agent, "Cloud agent is busy; queueing work");
        state.queued.push_back(agent, work);
        crate::event_loop::hud::send_queued(state, agent).await;
        return None;
    }
    run(state, hub, agent, work).await;
    None
}

/// Start the work queued for `agent` where it runs now, through
/// [`submit`]: on its machine an item at a time while it has no turn
/// running, as one that starts none (a command, a refusal) leaves the
/// next free to; else the first item on this computer. Nothing starts
/// while it moves.
pub(in crate::event_loop) async fn start_queued(state: &mut LoopState<'_>, agent: AgentId) {
    while !state.cloud.is_running(agent) && !state.cloud.is_moving(agent) {
        let Some(work) = state.queued.pop_front(agent) else {
            return;
        };
        crate::event_loop::hud::send_queued(state, agent).await;
        if let Some(work) = submit(state, agent, work).await {
            crate::event_loop::start_work::start_work(state, agent, work).await;
            return;
        }
    }
}

/// Run `work` on cloud agent `agent`, which has no turn running.
async fn run(state: &mut LoopState<'_>, hub: Arc<CloudHub>, agent: AgentId, work: Work) {
    match work {
        Work::Prompt { prompt, shown } => run_prompt(state, hub, agent, prompt, shown).await,
        Work::Command { command, .. } => run_command(state, hub, agent, command),
        Work::Compact { .. } | Work::Build { .. } => {
            system(state, agent, "Cloud agents summarize and build plans on their own machine; send it as a message instead.").await;
        }
    }
}

/// Send `prompt` to cloud agent `agent` as a turn, unless its model's plan
/// is not linked here.
async fn run_prompt(
    state: &mut LoopState<'_>,
    hub: Arc<CloudHub>,
    agent: AgentId,
    prompt: Prompt,
    shown: bool,
) {
    let ui = AgentUi::new(state.commands.clone(), agent);
    if !shown {
        ui.send(UiCommand::ShowMessage(MessageData::new(
            MessageRole::User,
            prompt.text.clone(),
        )))
        .await;
    }
    if !prompt.images.is_empty() {
        system(
            state,
            agent,
            "Images stay on this machine; the cloud agent gets the text.",
        )
        .await;
    }
    let current = state.models.current().to_string();
    let linked = state.auth.linked_plans(&state.auth.owner());
    if let Some(note) = plan_refusal(&current, &linked) {
        system(state, agent, note).await;
        return;
    }
    let model = (!current.is_empty() && current != "gate" && !current.starts_with("local/"))
        .then_some(current);
    let (title_model, titles) = turn_titles(state, &hub, agent, &prompt, &linked).await;
    if let Some(peers) = crate::event_loop::inbox::reach_of(&state.hub, agent).await {
        hub.set_reach(agent, peers);
    }
    let extras = TurnExtras {
        titles,
        context_limit: model.as_deref().map(aura_engine::context_window_for_model),
    };
    crate::event_loop::factory::hooks::plan_first_in_cloud(state, agent, &prompt).await;
    state.hub.factory_flows.note_prompt(agent, &prompt.text);
    let ask = TurnAsk {
        prompt: prompt.text,
        model,
        title_model,
        auth_jwt: zos_token(state),
        ..state.hub.cloud_turn(agent)
    };
    start_turn(state, &hub, agent, ui, ask, extras).await;
}

/// Start `ask` on cloud agent `agent`'s machine and show it in `ui`'s
/// chat, with `extras`. The agent counts as working until the hub has
/// copied the turn's session here; the loop then settles what the turn
/// was asked from that copy.
async fn start_turn(
    state: &LoopState<'_>,
    hub: &Arc<CloudHub>,
    agent: AgentId,
    ui: AgentUi,
    ask: TurnAsk,
    mut extras: TurnExtras,
) {
    // Stopped from the chat or its PROCESSES row alike.
    let cancel = CancellationToken::new();
    state
        .cloud
        .shared
        .running
        .lock()
        .insert(agent, cancel.clone());
    lock(&state.hub.cloud_turns).insert(agent);
    crate::event_loop::agents::send_agent_list(state).await;
    let mut row = crate::event_loop::handlers::turn_summary(
        agent,
        &ask.prompt,
        ask.model.as_deref().unwrap_or_default(),
    );
    row.model.clone_from(&ask.model);
    state.processes.start(row, Some(cancel.clone()));
    let mut rx = hub.run_turn(agent, ask, cancel.clone());
    let shared = Arc::clone(&state.cloud.shared);
    let processes = Arc::clone(&state.processes);
    let sessions = state.hub.factory.checkpoints.clone();
    tokio::spawn(async move {
        let end = forward(&ui, &mut rx, &mut extras, &cancel).await;
        let stopped = cancel.is_cancelled();
        processes.finish(
            &crate::event_loop::processes::turn_id(agent),
            end.status(stopped),
        );
        let copied = copied_session(&ui, &mut rx, sessions, agent).await;
        let failure = match end {
            _ if stopped => Some("it was stopped".to_string()),
            TurnEnd::Done => None,
            TurnEnd::Failed(why) => Some(why),
        };
        let messages = copied.map(|checkpoint| checkpoint.messages);
        shared.end_turn(agent, Some(TurnOutcome { failure, messages }));
    });
}

/// The session of `agent`'s turn as copied here, with its totals shown
/// in `ui`. The hub copies it before it lets go of `rx`.
async fn copied_session(
    ui: &AgentUi,
    rx: &mut mpsc::Receiver<RemoteEvent>,
    sessions: Option<Checkpoints>,
    agent: AgentId,
) -> Option<SessionCheckpoint> {
    while rx.recv().await.is_some() {}
    let sessions = sessions?;
    let copied = tokio::task::spawn_blocking(move || sessions.load_current(agent))
        .await
        .ok()
        .flatten()?;
    ui.send(UiCommand::SetSessionTotals(
        crate::event_loop::hud::session_totals(&copied.usage),
    ))
    .await;
    Some(copied)
}

/// The model `agent`'s machine titles its chat with on this turn, if any,
/// and where the title goes.
async fn turn_titles(
    state: &LoopState<'_>,
    hub: &Arc<CloudHub>,
    agent: AgentId,
    prompt: &Prompt,
    linked: &[aura_core_auth::ProviderKind],
) -> (Option<String>, Option<Titles>) {
    // Titled like a local agent: on a typed prompt, while it still
    // has a placeholder name. Its machine makes the call, since only
    // the machine writes the agent's record while it is placed.
    let wants_title = state.hub.titles_turn(agent, prompt.mentions);
    let title_model = wants_title
        .then(|| crate::event_loop::titles::title_model(state.models.classifier_model()))
        .filter(|model| runs_on_machine(model, linked));
    let mut titles = wants_title.then(|| {
        Titles::new(
            title_sink(Arc::clone(&state.hub), Arc::clone(hub), agent),
            &prompt.text,
        )
    });
    if title_model.is_none() {
        if let Some(titles) = titles.as_mut() {
            titles.finish().await;
        }
    }
    (title_model, titles)
}

/// `/new` for a cloud agent: its node writes the `SessionStart`, waking
/// the machine if it sleeps, and the hub copies it here. Returns whether
/// the node was asked; a failure is said in the agent's chat.
pub(in crate::event_loop) async fn new_session(state: &mut LoopState<'_>, agent: AgentId) -> bool {
    if let Some(root) = state.cloud.machine_root(agent) {
        let note = spawned_on_machine(state, agent, root);
        system(state, agent, note).await;
        return false;
    }
    if state.cloud.is_moving(agent) {
        system(
            state,
            agent,
            "This agent is moving between machines; start a new session when the move is done.",
        )
        .await;
        return false;
    }
    let Some(hub) = state.cloud.hub() else {
        return false;
    };
    if let Some(refusal) = foreign_owner(state, &hub, agent) {
        system(state, agent, refusal).await;
        return false;
    }
    if state.cloud.is_running(agent) {
        system(
            state,
            agent,
            "The cloud agent is still working; wait or stop it first.",
        )
        .await;
        return false;
    }
    start_new_session(state, hub, agent);
    true
}

/// Ask `agent`'s node for a new session. Prompts wait while it starts
/// one, so none runs in the old one.
fn start_new_session(state: &LoopState<'_>, hub: Arc<CloudHub>, agent: AgentId) {
    state
        .cloud
        .shared
        .running
        .lock()
        .insert(agent, CancellationToken::new());
    let shared = Arc::clone(&state.cloud.shared);
    let ui = AgentUi::new(state.commands.clone(), agent);
    tokio::spawn(async move {
        let result = hub.new_session(agent).await;
        shared.end_turn(agent, None);
        if let Err(err) = result {
            warn!(%agent, error = %err, "the cloud node did not start a new session");
            ui.send(UiCommand::ShowMessage(MessageData::new(
                MessageRole::System,
                format!("Could not start a new session on the cloud machine; the agent keeps its conversation: {err}"),
            )))
            .await;
        }
    });
}

/// Whether `model` can run on a cloud machine: not Gate or a model on
/// this computer, and a plan model only when its plan is `linked`.
pub(super) fn runs_on_machine(model: &str, linked: &[aura_core_auth::ProviderKind]) -> bool {
    !model.is_empty()
        && model != "gate"
        && !model.starts_with("local/")
        && plan_of_model(model).is_none_or(|kind| linked.contains(&kind))
}

/// The access token of the zOS session signed in here, if any.
fn zos_token(state: &LoopState<'_>) -> Option<String> {
    match state.auth.identity() {
        aura_auth::Identity::Zos(session) => Some(session.access_token),
        aura_auth::Identity::Local => None,
    }
}

/// Why `model` can't be used from cloud agents yet: a plan model runs on
/// the owner's plan, through the boxd secret their machine holds, so its
/// plan must be `linked` here.
fn plan_refusal(model: &str, linked: &[aura_core_auth::ProviderKind]) -> Option<String> {
    let kind = plan_of_model(model)?;
    (!linked.contains(&kind)).then(|| {
        format!(
            "Sign in to {} (`/login {}`) to use it from cloud agents.",
            kind.label(),
            kind.id()
        )
    })
}

/// Run `command` on cloud agent `agent`'s machine and show its output.
fn run_command(state: &LoopState<'_>, hub: Arc<CloudHub>, agent: AgentId, command: String) {
    let ui = AgentUi::new(state.commands.clone(), agent);
    tokio::spawn(async move {
        let note = match hub.exec(agent, &command).await {
            Ok(out) => format!(
                "$ {command}  (cloud, exit {})\n{}{}",
                out.exit_code,
                out.stdout_text(),
                out.stderr_tail(4000)
            ),
            Err(err) => format!("$ {command}: {err}"),
        };
        ui.send(UiCommand::ShowMessage(MessageData::new(
            MessageRole::System,
            note,
        )))
        .await;
    });
}
