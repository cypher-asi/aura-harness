//! Starting an agent's work: opening its session, setting up the turn
//! and handing it to a background task.

use super::agent_events::AgentUi;
use super::gate::turn::{GateTurn, Routing};
use super::steering::Steering;
use super::turns::{self, Prompt, Task, Work};
use super::{agents, channels, handlers, modes, plan_mode, subagents, titles, trace};
use super::{AgentSession, LoopState};
use aura_agent::AgentLoop;
use aura_agent_trace::RunRecorder;
use aura_core_types::{AgentId, RouteReason};
use aura_model::model_catalog::local_model_name;
use aura_model::ToolDefinition;
use aura_ui::events::ProcessSummary;
use aura_ui::{PermissionMode, UiCommand};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

/// A task ready to run, with what the loop tracks about it while it runs.
struct Launch {
    task: Task,
    steering: Option<Steering>,
    row: Option<ProcessSummary>,
}

/// Run `work` on `agent_id` in the background.
pub(super) async fn start_work(state: &mut LoopState<'_>, agent_id: AgentId, work: Work) {
    let ui = AgentUi::new(state.commands.clone(), agent_id);
    let Some(mut session) = take_session(state, agent_id, &ui).await else {
        return;
    };

    // A session opened from the record has a kernel built in the startup
    // mode; the agent's own mode decides its calls.
    modes::apply(
        &session.kernel,
        state.hub.permissions_of(agent_id),
        &state.hub.tool_names(),
    );
    let Some(work) = plan_work(state, &mut session, agent_id, work).await else {
        state.idle.insert(agent_id, session);
        return;
    };
    let ui = ui.with_shown(state.hub.record_panel.clone());
    let cancel = CancellationToken::new();
    let launch = match work {
        Work::Prompt { prompt, shown } => {
            let Some(launch) = turn_task(state, &mut session, &ui, &cancel, prompt, shown).await
            else {
                state.idle.insert(agent_id, session);
                return;
            };
            launch
        }
        Work::Command { command, cols } => Launch {
            task: Task::Command { command, cols },
            steering: None,
            row: None,
        },
        Work::Compact { focus } => compact_task(state, &session, focus),
        Work::Build { .. } => return,
    };

    if let Some(row) = launch.row {
        state.processes.start(row, Some(cancel.clone()));
    }
    state.hub.start_running(
        agent_id,
        Arc::clone(&session.kernel),
        cancel.clone(),
        launch.steering,
    );
    state.in_flight += 1;
    turns::spawn(session, launch.task, ui, cancel, state.work_done.clone());
    agents::send_agent_list(state).await;
}

/// The idle session of `agent_id`, or one opened from the record; `None`
/// after telling `ui` why it couldn't be opened.
async fn take_session(
    state: &mut LoopState<'_>,
    agent_id: AgentId,
    ui: &AgentUi,
) -> Option<AgentSession> {
    match state.idle.remove(&agent_id) {
        Some(session) => Some(session),
        None => match agents::open_session(state, agent_id) {
            Ok(session) => Some(session),
            Err(problem) => {
                ui.send(UiCommand::ShowError(problem)).await;
                ui.send(UiCommand::Complete).await;
                None
            }
        },
    }
}

/// `work` with plan mode applied: a message asking to build the plan, or
/// a build, becomes the build turn; `None` when nothing is left to run.
async fn plan_work(
    state: &mut LoopState<'_>,
    session: &mut AgentSession,
    agent_id: AgentId,
    work: Work,
) -> Option<Work> {
    match work {
        Work::Prompt { prompt, shown }
            if plan_mode::asks_to_build(
                &prompt,
                session.active_plan.as_deref(),
                session.building_plan,
            ) =>
        {
            info!(%agent_id, text = %prompt.text, "Message asks to build the plan");
            build_work(state, session, None, shown).await
        }
        Work::Build { path } => build_work(state, session, path, false).await,
        Work::Prompt { prompt, shown } if state.hub.factory_flows.is_planning(agent_id) => {
            // A new task plans first, rather than carrying on an old plan.
            super::factory::hooks::plan_first(state, session).await;
            Some(Work::Prompt { prompt, shown })
        }
        Work::Prompt { prompt, shown } => {
            let prompt = plan_mode::continue_open_plan(state, session, prompt).await;
            Some(Work::Prompt { prompt, shown })
        }
        other => Some(other),
    }
}

/// The turn task for `prompt`; `None` when the local model can't run it.
async fn turn_task(
    state: &mut LoopState<'_>,
    session: &mut AgentSession,
    ui: &AgentUi,
    cancel: &CancellationToken,
    mut prompt: Prompt,
    shown: bool,
) -> Option<Launch> {
    let agent_id = session.agent_id();
    info!(%agent_id, text = %prompt.text, "Processing user message");
    // A local model with no server used to sit there until the
    // 10-minute request timeout; the probe answers in well under a
    // second.
    if local_model_unavailable(state, ui).await {
        return None;
    }
    // In the prompt rather than the system prompt, which stays the
    // same between turns so the provider's prompt cache covers it.
    if prompt.mentions {
        prompt.preamble = channels::take_digest(&state.hub, agent_id).await;
    }
    ui.send(UiCommand::TurnStarted {
        prompt: (!shown).then(|| prompt.text.clone()),
    })
    .await;
    let (agent_loop, tools, recorder, lead) = prepare_turn(state, session).await;
    let gate = gate_turn(state, &agent_loop, lead).map(Box::new);
    let routing = Box::new(routing(state, &agent_loop));
    if let Some(rec) = &recorder {
        rec.begin_turn(&prompt.text);
    }
    let row = Some(handlers::turn_summary(
        agent_id,
        &prompt.text,
        &agent_loop.config().model,
    ));
    let turn_steering = Steering::new(cancel);
    let steering = Some(turn_steering.clone());
    spawn_title(state, session, &agent_loop, &prompt);
    let task = Task::Turn {
        prompt,
        planning: state.hub.mode_of(agent_id) == PermissionMode::Plan,
        agent_loop: Box::new(agent_loop),
        tools,
        steering: turn_steering,
        limits: state.hub.long_run,
        recorder,
        gate,
        routing,
    };
    Some(Launch {
        task,
        steering,
        row,
    })
}

/// Name the agent of `session` from `text` when it still needs a title.
fn spawn_title(
    state: &LoopState<'_>,
    session: &AgentSession,
    agent_loop: &AgentLoop,
    prompt: &Prompt,
) {
    let agent_id = session.agent_id();
    if state.hub.titles_turn(agent_id, prompt.mentions) {
        titles::spawn(
            Arc::clone(&state.hub),
            agent_id,
            aura_agent::KernelModelGateway::new(Arc::clone(&session.kernel)),
            agent_loop.config().clone(),
            titles::title_model(state.models.classifier_model()),
            prompt.text.clone(),
        );
    }
}

/// The task summarizing older history of `session`.
fn compact_task(
    state: &mut LoopState<'_>,
    session: &AgentSession,
    focus: Option<String>,
) -> Launch {
    let agent_id = session.agent_id();
    state.sync_turn_base();
    let (config, _) = state.hub.turn(
        session.definition.as_deref(),
        &session.kernel.agent_workspace(),
    );
    let row = Some(handlers::turn_summary(
        agent_id,
        "Summarizing older history",
        &config.model,
    ));
    Launch {
        task: Task::Compact {
            agent_loop: Box::new(AgentLoop::new(config)),
            focus,
        },
        steering: None,
        row,
    }
}

/// The turn that builds the plan at `path`, or `session`'s plan, once
/// [`plan_mode::start_build`] got it ready; `None` when it can't. `shown`
/// is whether the chat already shows what asked for it: a typed
/// "implement" is, so the long build prompt is not shown after it.
async fn build_work(
    state: &mut LoopState<'_>,
    session: &mut AgentSession,
    path: Option<String>,
    shown: bool,
) -> Option<Work> {
    let text = plan_mode::start_build(state, session, path).await?;
    Some(Work::Prompt {
        prompt: Prompt::build(text),
        shown,
    })
}

/// The agent loop and tools for a turn of `session`, the recorder of its
/// conversation's run when tracing is on, and how the turn leads when
/// its prompt asks for work a lead takes.
async fn prepare_turn(
    state: &mut LoopState<'_>,
    session: &mut AgentSession,
) -> (
    AgentLoop,
    Vec<ToolDefinition>,
    Option<Arc<RunRecorder>>,
    Option<subagents::LeadProfile>,
) {
    for problem in subagents::lock(&state.hub.definitions).reload() {
        warn!(%problem, "Skipping agent definition");
    }
    state.hub.send_definitions().await;
    state.sync_turn_base();
    let workspace = session.kernel.agent_workspace();
    let agent_id = session.agent_id();
    let (mut config, tools, lead) = state.hub.user_turn(session);
    state.hub.send_instructions(agent_id, &workspace).await;
    state
        .hub
        .register_root(agent_id, &session.identity.name, workspace, &config);
    config.initial_todos.clone_from(&session.todos);
    if let Some(ref mm) = state.memory_manager {
        mm.prepare_context(agent_id, &mut config.system_prompt)
            .await;
        config.observers.clear();
        config
            .observers
            .push(aura_engine::memory_observer::MemoryTurnObserver::new(
                Arc::clone(mm),
                session.agent_id(),
                None,
                Vec::new(),
                None,
            ));
    }
    let recorder = trace::recorder(state, session, &config.model);
    if let Some(rec) = &recorder {
        config.trace_blobs = rec.blob_store();
    }
    (AgentLoop::new(config), tools, recorder, lead)
}

/// When Gate routes the turn `agent_loop` is set up for (Gate is selected
/// and no permanent agent names its own model), what it routes among once
/// the prompt is classified. The turn shows Gate's choice when it makes
/// it, and leads as `lead` says when the prompt calls for a lead.
fn gate_turn(
    state: &LoopState<'_>,
    agent_loop: &AgentLoop,
    lead: Option<subagents::LeadProfile>,
) -> Option<GateTurn> {
    state
        .models
        .route()
        .filter(|preview| preview.id == agent_loop.config().model)?;
    Some(GateTurn {
        candidates: state.models.gate_candidates(),
        limits: state.models.limits(),
        live: Arc::clone(&state.hub.gate),
        models: state.models.ui_command(),
        lead,
        hub: Arc::clone(&state.hub),
    })
}

/// How Gate records the prompt of a turn on `agent_loop`: the turn's model
/// is Gate's default when Gate is selected and the agent doesn't name its
/// own model (Gate's pick, or the local default when nothing else can
/// run), else pinned.
fn routing(state: &LoopState<'_>, agent_loop: &AgentLoop) -> Routing {
    let gated = state.models.is_gate() && agent_loop.config().model == state.models.turn_model();
    Routing {
        classifier: state.models.classifier_model(),
        reason: if gated {
            RouteReason::Default
        } else {
            RouteReason::Pinned
        },
    }
}

/// Re-probe Ollama when the selected model is local. Returns `true` after
/// telling `ui` why it can't run and ending its turn.
pub(super) async fn local_model_unavailable(state: &mut LoopState<'_>, ui: &AgentUi) -> bool {
    if local_model_name(&state.models.turn_model()).is_none() {
        return false;
    }
    state.models.refresh_local().await;
    let _ = state.commands.send(state.models.ui_command()).await;
    let Some(hint) = state.models.local_hint() else {
        return false;
    };
    info!("Skipping local turn; model server is not ready");
    ui.send(handlers::system_message(hint)).await;
    ui.send(UiCommand::Complete).await;
    true
}
