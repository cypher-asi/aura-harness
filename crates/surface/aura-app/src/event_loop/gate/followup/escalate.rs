//! What follows an attempt at routed work that failed, shared by the
//! terminal's turns, `/compare` runs and `aura -p`: Gate's judge says
//! whether its model was at fault ([`Judging`]), and the next stronger
//! model ([`stronger`]) takes the work over in the same conversation,
//! told what was found wrong ([`Takeover`]), under a prompt of its own
//! recorded with route reason `escalate`. Work whose model's route
//! refused the user's sign-in goes to Gate's next choice instead
//! ([`reroute`]).

use super::messages::{from_harness, takeover};
use super::{Found, Report};
use crate::event_loop::agents::AgentSession;
use crate::event_loop::turns::record_prompt;
use aura_agent_trace::RunRecorder;
use aura_core_types::RouteLevel;
use aura_core_types::{Hash, OutcomePayload, OutcomeSignal, PromptCategory, PromptPayload};
use aura_gate::judge::turn::Failed;
use aura_gate::live::{self, GateLive, Judged};
use aura_gate::route::{Candidate, Decision, Work};
use aura_model::ThinkingEffort;

/// A failed attempt at work Gate routed, as its judge is asked about it.
pub(crate) struct Judging<'a> {
    pub session: &'a AgentSession,
    /// Model that judges the failure (see [`live::judge_for`])
    pub judge: &'a str,
    /// The attempt's recorded outcome
    pub ended: &'a OutcomePayload,
    /// What the judge is shown of the attempt
    pub shown: Failed<'a>,
    pub auth_token: Option<String>,
}

impl Judging<'_> {
    /// Ask the judge through the session's kernel whether the model was
    /// at fault, record its finding as a later outcome of the attempt's
    /// prompt (see [`live::judge_failed`]) and show it on `report`.
    /// Returns that outcome and the finding; `None` when the judge gave
    /// no verdict.
    pub(crate) async fn ask(self, report: &impl Report) -> Option<(OutcomePayload, Judged)> {
        let kernel = &self.session.kernel;
        let judged =
            live::judge_failed(kernel, self.judge, self.ended, &self.shown, self.auth_token)
                .await?;
        report.report(Found::Judged(&judged.1)).await;
        Some(judged)
    }
}

/// The model that takes over work asking for `category` whose attempt
/// `ran` on a model, at the effort Gate chose when it chose one, and
/// ended with `signal`: the next stronger one among `candidates` (see
/// [`GateLive::escalate`]). `None` when nothing is stronger.
pub(crate) fn stronger(
    live: &GateLive,
    candidates: &[Candidate],
    category: PromptCategory,
    ran: (&str, Option<ThinkingEffort>),
    signal: OutcomeSignal,
) -> Option<Decision> {
    live.escalate(RouteLevel::Turn, category, ran, signal, candidates)
}

/// A stronger model taking over work that failed: where the work goes,
/// and what the model is told.
pub(crate) struct Takeover {
    /// The stronger model and its effort, decided for route reason
    /// `escalate`
    pub next: Decision,
    /// What the model is told, not marked as the harness's
    pub text: String,
}

impl Takeover {
    /// `next` taking over the work of `model`, which ended with
    /// `signal`, told what was `found` wrong with it.
    pub(crate) fn new(
        model: &str,
        next: Decision,
        signal: OutcomeSignal,
        found: Option<&str>,
    ) -> Self {
        let text = takeover(model, &next.model.id, signal, found);
        Self { next, text }
    }

    /// The takeover marked as the harness's, as the model gets it, and
    /// recorded as [`takeover_prompt`] says.
    pub(crate) fn prompt(&self, category: PromptCategory, classifier: String) -> PromptPayload {
        takeover_prompt(from_harness(&self.text), category, &self.next, classifier)
    }
}

/// The prompt `text` is recorded as when it hands work asking for
/// `category` to the stronger model `next` decided on: with the route
/// reason `escalate`, and classified by `classifier`, empty when the
/// work records none for it.
pub(crate) fn takeover_prompt(
    text: String,
    category: PromptCategory,
    next: &Decision,
    classifier: String,
) -> PromptPayload {
    PromptPayload {
        category: Some(category),
        classifier,
        route: next.route(RouteLevel::Turn),
        ..PromptPayload::new(text)
    }
}

/// Record `prompt`, Gate's message to the model, on `session`, and in
/// `recorder`'s trace as a message Gate sent into the turn; returns the
/// prompt's transaction hash.
///
/// # Errors
/// Says why when the prompt could not be recorded.
pub(crate) async fn record_message(
    session: &AgentSession,
    prompt: &PromptPayload,
    recorder: Option<&RunRecorder>,
) -> Result<Hash, String> {
    if let Some(recorder) = recorder {
        recorder.follow_up(&prompt.text);
    }
    let (tx, _) = record_prompt(session, prompt).await?;
    Ok(tx.hash)
}

/// Gate's choice for `work` once the routes of the models in `refused`
/// turned the user's sign-in down: its pick among the other
/// `candidates`, without exploring. `None` when none is left.
pub(crate) fn reroute(
    live: &GateLive,
    work: &Work<'_>,
    candidates: &[Candidate],
    refused: &[String],
) -> Option<Decision> {
    let left: Vec<Candidate> = (candidates.iter())
        .filter(|candidate| !refused.contains(&candidate.id))
        .cloned()
        .collect();
    live.redecide(work, &left)
}

/// What the user is told when the route of `model` refused their sign-in
/// and the work goes where `route`, the line saying where Gate sent it,
/// says.
pub(crate) fn refused_notice(model: &str, route: &str) -> String {
    format!("{model} refused your sign-in; Gate skips it until your sign-ins change.\n{route}")
}
