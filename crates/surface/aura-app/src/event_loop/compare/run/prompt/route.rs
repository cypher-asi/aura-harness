//! A prompt before its first attempt: classified, routed as the run's arm
//! routes it (by Gate, by the arm's model for its difficulty, or to the
//! model the arm is pinned to), announced in the run's chat, and recorded
//! with route reason `compare`.

use super::attempts::Asked;
use super::{Prompt, Turned};
use crate::event_loop::agents::AgentSession;
use crate::event_loop::compare::run::arm::short_label;
use crate::event_loop::compare::run::turn::RunParts;
use crate::event_loop::gate::followup;
use crate::event_loop::handlers::system_message;
use crate::event_loop::subagents::{ArmRouting, LeadProfile};
use crate::event_loop::turns::record_prompt;
use crate::model_selection::routed_effort;
use aura_agent::AgentLoopConfig;
use aura_core_types::time::now_ms;
use aura_core_types::{
    Difficulty, Hash, OutcomeSignal, PromptCategory, PromptPayload, Route, RouteReason, RunTurn,
};
use aura_gate::judge::classify::Label;
use aura_gate::judge::criteria::Criteria;
use aura_gate::live;
use aura_gate::route::{Decision, Work};
use aura_model::model_catalog::route_label;
use aura_model::ToolDefinition;
use aura_ui_terminal::UiCommand;
use std::sync::Arc;
use std::time::Instant;
use tracing::warn;

/// A prompt as it was routed and recorded, before its first attempt.
pub(super) struct Routed<'a> {
    pub asked: Asked<'a>,
    /// What its first attempt runs with
    pub config: AgentLoopConfig,
    pub tools: Vec<ToolDefinition>,
}

/// Classify and route `prompt` on `session`, then announce and record it.
/// A pinned arm's prompt that could not be classified goes on
/// unclassified; on other arms it errs with the turn as it ended.
pub(super) async fn route<'a>(
    parts: &RunParts<'_>,
    session: &mut AgentSession,
    prompt: Prompt<'a>,
) -> Result<Routed<'a>, Turned> {
    let text = prompt.text;
    let (mut config, mut tools, lead) = turn_config(parts, session, text);
    let label = classify(parts, session, text, &config).await?;
    let category = label.as_ref().map(|label| label.category);
    let mut difficulty = label.as_ref().map(|label| label.difficulty);
    let assessment = label.and_then(|label| label.assessment);
    let criteria = criteria(parts, session, category, &mut difficulty, text, &config).await;
    let work = category.map(|category| Work::turn(category).of(difficulty));
    let decision = decide(parts, &mut config, work.as_ref(), prompt.refused);
    let model = config.model.clone();
    let led = lead.is_some_and(|lead| {
        let led = leads(parts, session, (category, difficulty), &model);
        session.lead = lead.apply(led, &mut config, &mut tools);
        led
    });
    let payload = PromptPayload {
        category,
        classifier: parts.ctx.plan.classifier.clone(),
        route: route_of(&model, decision.as_ref()),
        led: Some(led),
        difficulty,
        assessment: assessment.clone(),
        ..PromptPayload::new(text.to_string())
    };
    announce(parts, session, &prompt, &config, &model, category).await;
    let (started, started_ms) = (Instant::now(), now_ms());
    let prompt_tx = record(session, &payload).await;
    let asked = Asked {
        text,
        category,
        difficulty,
        assessment,
        led,
        criteria,
        before: prompt.before,
        refused: prompt.refused,
        effort: effort_of(&config),
        model,
        prompt_tx,
        started,
        started_ms,
    };
    Ok(Routed {
        asked,
        config,
        tools,
    })
}

/// Whether Gate has `model` lead the work of `category` at `difficulty`
/// on `session` rather than do it itself; never for unclassified work.
fn leads(
    parts: &RunParts<'_>,
    session: &AgentSession,
    (category, difficulty): (Option<PromptCategory>, Option<Difficulty>),
    model: &str,
) -> bool {
    parts.gate.note_model(session.agent_id(), model);
    category.is_some_and(|category| parts.gate.way((category, difficulty), model, false).led)
}

/// What the run's turn on `text` starts from: the user's turn on
/// `session` (see
/// [`AgentHub::user_turn`](crate::event_loop::subagents::AgentHub::user_turn))
/// with no observers, carrying on the session's todo list, and begun in
/// the run's trace.
fn turn_config(
    parts: &RunParts<'_>,
    session: &mut AgentSession,
    text: &str,
) -> (AgentLoopConfig, Vec<ToolDefinition>, Option<LeadProfile>) {
    let (mut config, tools, lead) = parts.ctx.hub.user_turn(session);
    config.observers.clear();
    config.initial_todos.clone_from(&session.todos);
    if let Some(recorder) = &parts.recorder {
        recorder.begin_turn(text);
    }
    (config, tools, lead)
}

/// What `text` asks for and how hard it is, as the comparison's
/// classifier labels it through the comparison's own kernel, signed in as
/// `config` is: once for the whole comparison, whichever run sends it
/// first (see [`Labels`](super::super::labels::Labels)), and paid for by
/// neither arm. `None` when a pinned arm's prompt could not be labelled,
/// which goes on unclassified; on other arms that errs with the turn as
/// it ended.
async fn classify(
    parts: &RunParts<'_>,
    session: &AgentSession,
    text: &str,
    config: &AgentLoopConfig,
) -> Result<Option<Label>, Turned> {
    let classifier = &parts.ctx.plan.classifier;
    let labelled = if classifier.is_empty() {
        Err("Gate has no model to classify the prompt".to_string())
    } else {
        let auth_token = config.auth_token.clone();
        let ask = || live::classify_prompt(&parts.ctx.kernel, classifier, text, auth_token);
        parts.ctx.labels.of(text, ask).await
    };
    match labelled {
        Ok(label) => Ok(Some(label)),
        Err(problem) if matches!(parts.routing, ArmRouting::Pinned(_)) => {
            goes_on_unclassified(parts, session, problem).await;
            Ok(None)
        }
        Err(problem) => Err(unclassified(parts, problem).await),
    }
}

/// Tell the run's chat that its prompt, which could not be classified
/// for `problem`, goes on unclassified. A pinned arm needs no label to
/// pick its model, as a turn on a pinned model outside a benchmark does
/// not; the run then leaves no outcome for Gate, with no category to
/// count it under.
async fn goes_on_unclassified(parts: &RunParts<'_>, session: &AgentSession, problem: String) {
    let agent_id = session.agent_id();
    warn!(%agent_id, %problem, "A pinned compare run goes on unclassified");
    let line = format!("{problem} · the run goes on unclassified");
    parts.ui.send(system_message(line)).await;
}

/// The turn of a Gate or by-difficulty prompt that could not be
/// classified, for `problem`, which the run's chat shows.
async fn unclassified(parts: &RunParts<'_>, problem: String) -> Turned {
    parts.ui.send(UiCommand::ShowWarning(problem.clone())).await;
    parts.ui.send(UiCommand::Complete).await;
    // No model was routed to, so the run is named for its arm: the
    // session's own model is no model of the arm.
    let model = short_label(parts.spec()).into_owned();
    Turned {
        prompt_tx: None,
        reply: String::new(),
        error: Some(problem),
        ended: None,
        cut_short: false,
        turn: RunTurn::new(model, 0.0, 0, OutcomeSignal::Error),
    }
}

/// What a Gate run holds the work `text` asks for, of `category`, to:
/// criteria written down before it starts (see
/// [`live::hold_to_criteria`]), signed in as `config` is, which may
/// change its `difficulty`. `None` on other arms, or when they could not
/// be written, which the run's chat is told.
async fn criteria(
    parts: &RunParts<'_>,
    session: &AgentSession,
    category: Option<PromptCategory>,
    difficulty: &mut Option<Difficulty>,
    text: &str,
    config: &AgentLoopConfig,
) -> Option<Arc<Criteria>> {
    let category = category.filter(|_| parts.gate_arm())?;
    let plan = &parts.ctx.plan;
    let held = live::hold_to_criteria(
        &session.kernel,
        (parts.gate, &plan.candidates),
        category,
        difficulty,
        text,
        config.auth_token.clone(),
    );
    match held.await {
        Ok(criteria) => criteria,
        Err(problem) => {
            parts.ui.send(system_message(problem)).await;
            None
        }
    }
}

/// Where the run's arm sends `work`: Gate's choice, exploring only when
/// the comparison learns, or else, once the routes of the models in
/// `refused` turned the user's sign-in down, its re-route among the
/// others (see [`followup::reroute`]); or the arm's model for its
/// difficulty. `config` is pointed at it; a pinned arm's `config` is
/// pointed at its model instead.
fn decide(
    parts: &RunParts<'_>,
    config: &mut AgentLoopConfig,
    work: Option<&Work<'_>>,
    refused: &[String],
) -> Option<Decision> {
    let (hub, plan, gate) = (&parts.ctx.hub, &parts.ctx.plan, parts.gate);
    let decision = match (parts.routing, work) {
        (ArmRouting::Gate { .. }, Some(work)) if !refused.is_empty() => {
            followup::reroute(gate, work, &plan.candidates, refused)
        }
        (ArmRouting::Gate { .. }, Some(work)) if plan.learn => gate.decide(work, &plan.candidates),
        (ArmRouting::Gate { .. }, Some(work)) => gate.redecide(work, &plan.candidates),
        (ArmRouting::ByDifficulty(routes), Some(work)) => Some(routes.decide(work)),
        _ => None,
    };
    match (parts.routing, &decision) {
        (ArmRouting::Gate { .. } | ArmRouting::ByDifficulty(_), Some(decision)) => {
            hub.model_limits().route(config, decision);
        }
        (ArmRouting::Pinned(model), _) if *model != config.model => {
            hub.model_limits().configure(config, model);
        }
        _ => {}
    }
    decision
}

/// The route a prompt sent to `model` is recorded with: route reason
/// `compare`, and what `decision` explored, how likely it was and at what
/// effort, when there was one.
fn route_of(model: &str, decision: Option<&Decision>) -> Route {
    Route {
        explored: decision.is_some_and(|d| d.explored),
        propensity: decision.map_or(1.0, |d| d.propensity),
        effort: decision.and_then(|d| d.effort.reasoning_effort_wire().map(str::to_string)),
        ..Route::turn(model.to_string(), RouteReason::Compare)
    }
}

/// Show in the run's chat that `prompt`'s turn starts on `model` for
/// `category`, if it was classified; the first prompt also lists the run
/// among the agents, as it runs on `config`.
async fn announce(
    parts: &RunParts<'_>,
    session: &AgentSession,
    prompt: &Prompt<'_>,
    config: &AgentLoopConfig,
    model: &str,
    category: Option<PromptCategory>,
) {
    let (ctx, run, ui) = (parts.ctx, parts.run, &parts.ui);
    if prompt.i == 0 {
        let workdir = session.kernel.agent_workspace();
        ctx.hub
            .register_root(session.agent_id(), &run.name, workdir, config);
        ctx.hub.send_agent_list().await;
    }
    let started = UiCommand::TurnStarted {
        prompt: Some(prompt.text.to_string()),
    };
    ui.send(started).await;
    let step = if prompt.of > 1 {
        format!(" · prompt {} of {}", prompt.i + 1, prompt.of)
    } else {
        String::new()
    };
    let label = short_label(parts.spec());
    let line = format!(
        "{} {} · {label} → {} · {}{step}",
        ctx.host.name,
        run.name,
        route_label(model),
        category.map_or("unclassified", PromptCategory::as_str),
    );
    ui.send(system_message(line)).await;
}

/// Record `payload` on `session`; `None`, with the problem logged, when
/// it could not be recorded.
async fn record(session: &AgentSession, payload: &PromptPayload) -> Option<Hash> {
    match record_prompt(session, payload).await {
        Ok((tx, _)) => Some(tx.hash),
        Err(problem) => {
            let agent_id = session.agent_id();
            warn!(%agent_id, %problem, "Failed to record a compare run's prompt");
            None
        }
    }
}

/// The effort `config`'s requests are routed at, as a run's record names
/// it.
pub(super) fn effort_of(config: &AgentLoopConfig) -> Option<String> {
    routed_effort(config).and_then(|e| e.reasoning_effort_wire().map(str::to_string))
}
