//! Gate's record of each turn: the category the classifier gives its
//! prompt, the `Outcome` written when it ends, and the later signals that
//! judge it again — the judge's finding on a turn that failed, a rewind,
//! the same prompt sent again on a stronger model. Each later signal
//! appends another `Outcome` for the same prompt;
//! [`aura_agent_gate::outcome::effective`] says which one counts.

use super::agents::AgentSession;
use super::gate_live::GateLive;
use aura_agent_gate::criteria::{self, Criteria};
use aura_agent_gate::router::verifiable;
use aura_agent_gate::turn_judge::{self, Answer, Failed, Unverified, Verdict};
use aura_agent_gate::{classify, outcome::is_retry_stronger, Candidate};
use aura_agent_kernel::Kernel;
use aura_config::ReviewMode;
use aura_core_types::{
    AgentId, Difficulty, Hash, OutcomePayload, OutcomeSignal, PromptCategory, PromptPayload,
    Transaction, TransactionType,
};
use aura_model::model_catalog::{infer_tier, ModelTier};
use aura_store_db::ReadStore;
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};

/// How long the classifier may take before the turn fails.
const CLASSIFY_TIMEOUT: Duration = Duration::from_secs(20);

/// How long writing a turn's acceptance criteria may take before the turn
/// goes on without them.
const CRITERIA_TIMEOUT: Duration = Duration::from_secs(60);

/// How many times the classifier is asked when its reply gives no label.
const CLASSIFY_ATTEMPTS: u32 = 2;

/// How long the judge of a failed turn may take before the turn keeps
/// its own signal.
const JUDGE_TIMEOUT: Duration = Duration::from_secs(90);

/// How long the reviewer of work about to stand may take: it reads the
/// whole change, and may reason before it answers.
const REVIEW_TIMEOUT: Duration = Duration::from_secs(300);

/// Entries scanned back from the head of a record to find a turn's
/// outcome.
const SCAN_LIMIT: usize = 2_000;

/// The judge's finding on work that failed.
#[derive(Debug, Clone, PartialEq)]
pub struct Judged {
    /// Model whose work was judged
    pub model: String,
    /// Model that judged it
    pub judge: String,
    pub verdict: Verdict,
    /// List-price cost of the judge's call in USD; 0 when the price is
    /// unknown. Not part of the work's own cost: it is what Gate pays to
    /// learn from the failure.
    pub cost_usd: f64,
    /// Whether the work ran no check and was judged for looking wrong
    /// rather than for failing.
    pub unverified: bool,
}

impl Judged {
    /// One line saying what the judge found.
    #[must_use]
    pub fn describe(&self) -> String {
        let label = |id: &str| aura_model::model_catalog::route_label(id);
        let found = match (self.unverified, self.verdict.at_fault) {
            (false, true) => format!("the failure is {}'s", label(&self.model)),
            (false, false) => format!("the failure is not {}'s doing", label(&self.model)),
            (true, true) => format!("{}'s unchecked work is not done", label(&self.model)),
            (true, false) => format!("{}'s unchecked work stands", label(&self.model)),
        };
        let reasons = self.verdict.reasons.trim();
        format!(
            "Gate's judge ({}, ${:.4}): {found}{}",
            label(&self.judge),
            self.cost_usd,
            if reasons.is_empty() {
                String::new()
            } else {
                format!(". {reasons}")
            }
        )
    }
}

/// The model that judges `ended`, the outcome of work Gate routed among
/// `candidates`: the strongest Opus among them (see
/// [`aura_agent_gate::compare::default_against`]). `None` when the work
/// did not fail, the judge is switched off, or the work ran on a model
/// as strong as the judge.
pub(crate) fn judge_for(
    live: &GateLive,
    candidates: &[Candidate],
    ended: &OutcomePayload,
) -> Option<String> {
    if !live.settings().judge
        || !matches!(
            ended.signal,
            OutcomeSignal::Error | OutcomeSignal::ChecksFailed
        )
    {
        return None;
    }
    let judge = aura_agent_gate::compare::default_against(candidates)?;
    turn_judge::judges(&judge.id, &ended.model, candidates).then(|| judge.id.clone())
}

/// Ask `judge` through `kernel` whether the model was at fault in
/// `ended`, the outcome of work that `failed`, and record the finding as
/// a later `Outcome` of the same prompt: `judged_fail` when it was,
/// `judged_excused` (which takes the failure back) when it was not.
/// Returns that outcome and the finding; `None` when the judge gave no
/// verdict, which leaves the work's own signal standing.
pub(crate) async fn judge_failed(
    kernel: &Kernel,
    judge: &str,
    ended: &OutcomePayload,
    failed: &Failed<'_>,
    auth_token: Option<String>,
) -> Option<(OutcomePayload, Judged)> {
    let request = turn_judge::request(judge, failed, auth_token);
    let (_, judged) = ask_judge(kernel, judge, ended, request, false).await?;
    let signal = if judged.verdict.at_fault {
        OutcomeSignal::JudgedFail
    } else {
        OutcomeSignal::JudgedExcused
    };
    info!(%judge, model = %ended.model, %signal, cost_usd = judged.cost_usd, "The judge reviewed failed work");
    let outcome = ended.with_signal(signal);
    record(kernel, kernel.agent_id, &outcome).await;
    Some((outcome, judged))
}

/// The model that gives the verdict on work done on `model`, Gate routing
/// among `candidates`, that no check vouches for, since every routed turn
/// ends with a verdict: `model` itself when it is a standard model or
/// stronger, so Sonnet judges Sonnet's work at Sonnet's price, and for a
/// weaker model's work the cheapest standard model, else the strongest
/// ([`aura_agent_gate::router::fixed_pick`]). Whether a failure was the
/// model's fault is still [`judge_for`]'s to say. `None` only when a weak
/// model's work has no candidates to be judged by.
pub(crate) fn verdict_judge_for(candidates: &[Candidate], model: &str) -> Option<String> {
    if tier_of(model, candidates) >= ModelTier::Standard {
        return Some(model.to_string());
    }
    aura_agent_gate::router::fixed_pick(candidates).map(|judge| judge.id.clone())
}

/// The model that reviews work on `model` before it stands, Gate routing
/// among `candidates`: the one that gives the verdict on its unchecked
/// work ([`verdict_judge_for`]). `None` when the judge is switched off or
/// the work ran on the strongest tier, whose work is not reviewed.
pub(crate) fn reviewer_for(
    live: &GateLive,
    candidates: &[Candidate],
    model: &str,
) -> Option<String> {
    if !live.settings().judge {
        return None;
    }
    let strongest = aura_agent_gate::compare::default_against(candidates)?;
    if !turn_judge::judges(&strongest.id, model, candidates) {
        return None;
    }
    verdict_judge_for(candidates, model)
}

/// The tier of `model`: its candidate's, else what its name says.
fn tier_of(model: &str, candidates: &[Candidate]) -> ModelTier {
    candidates
        .iter()
        .find(|c| c.id == model)
        .map_or_else(|| infer_tier(model), |c| c.tier)
}

/// Ask `judge` through `kernel` whether the work of `ended`, which no
/// check vouches for, is done, pointed to what looks wrong as
/// `unverified` says. Returns the verdict as the attempt's signal
/// (`judged_pass`, `judged_fail` or `judged_excused`, see
/// [`Answer::verdict`]) and the finding; the caller records it with the
/// attempt's outcome. `None` when the judge gave no verdict.
pub(crate) async fn judge_unverified(
    kernel: &Kernel,
    judge: &str,
    ended: &OutcomePayload,
    unverified: &Unverified<'_>,
    auth_token: Option<String>,
) -> Option<(OutcomeSignal, Judged)> {
    let request = turn_judge::unverified_request(judge, unverified, auth_token);
    let (answer, judged) = ask_judge(kernel, judge, ended, request, true).await?;
    let signal = answer.verdict();
    info!(%judge, model = %ended.model, %signal, cost_usd = judged.cost_usd, "The judge gave its verdict on unchecked work");
    Some((signal, judged))
}

/// Send `request` to `judge` through `kernel` and read its verdict on
/// the work of `ended`. `None` when the request could not be built, the
/// call failed or timed out, or the reply held no verdict.
async fn ask_judge(
    kernel: &Kernel,
    judge: &str,
    ended: &OutcomePayload,
    request: Result<aura_model::ModelRequest, aura_model::ReasonerError>,
    unverified: bool,
) -> Option<(Answer, Judged)> {
    let (answer, reasons, cost_usd) = ask(kernel, judge, request, JUDGE_TIMEOUT).await?;
    let judged = Judged {
        model: ended.model.clone(),
        judge: judge.to_string(),
        verdict: Verdict {
            at_fault: answer == Answer::Model,
            reasons,
        },
        cost_usd,
        unverified,
    };
    Some((answer, judged))
}

/// Send `request`, a review of work about to stand
/// ([`turn_judge::review_request`]), to `judge` through `kernel`. Returns
/// its answer, what it found and the list-price cost of the call; `None`
/// when the request could not be built, the call failed or timed out, or
/// the reply held no verdict. Nothing is recorded.
pub(crate) async fn ask_review(
    kernel: &Kernel,
    judge: &str,
    request: Result<aura_model::ModelRequest, aura_model::ReasonerError>,
) -> Option<(Answer, String, f64)> {
    ask(kernel, judge, request, REVIEW_TIMEOUT).await
}

async fn ask(
    kernel: &Kernel,
    judge: &str,
    request: Result<aura_model::ModelRequest, aura_model::ReasonerError>,
    within: Duration,
) -> Option<(Answer, String, f64)> {
    let request = match request {
        Ok(request) => request,
        Err(e) => {
            warn!(%judge, error = %e, "Could not build the judge's request");
            return None;
        }
    };
    let response = match tokio::time::timeout(within, kernel.reason(request)).await {
        Ok(Ok(result)) => result.response,
        Ok(Err(e)) => {
            warn!(%judge, error = %e, "The judge's call failed");
            return None;
        }
        Err(_) => {
            warn!(%judge, "The judge's call timed out");
            return None;
        }
    };
    let Some((answer, reasons)) = turn_judge::parse_answer(&response.message.text_content()) else {
        warn!(%judge, "The judge gave no verdict");
        return None;
    };
    let cost_usd =
        aura_model::model_catalog::estimate_cost_usd(judge, &response.usage).unwrap_or(0.0);
    Some((answer, reasons, cost_usd))
}

/// Label `text` with its category and difficulty by asking `model`
/// through `kernel`, so the call is recorded. A reply without a label is
/// asked for again, up to [`CLASSIFY_ATTEMPTS`] calls in all.
///
/// # Errors
/// Says why when the request could not be built, a call failed or timed
/// out, or no reply gave both a category and a difficulty: Gate does not
/// route work it could not classify.
pub(crate) async fn classify_prompt(
    kernel: &Kernel,
    model: &str,
    text: &str,
    auth_token: Option<String>,
) -> Result<classify::Label, String> {
    let failed = |error: String| {
        warn!(%model, %error, "Could not classify the prompt");
        Err(format!(
            "Gate could not classify the prompt with {model}: {error}"
        ))
    };
    let mut reply = String::new();
    for attempt in 1..=CLASSIFY_ATTEMPTS {
        let request = match classify::request(model, text, auth_token.clone()) {
            Ok(request) => request,
            Err(e) => return failed(format!("could not build the request ({e})")),
        };
        let response = match tokio::time::timeout(CLASSIFY_TIMEOUT, kernel.reason(request)).await {
            Ok(Ok(result)) => result.response,
            Ok(Err(e)) => return failed(format!("the call failed ({e})")),
            Err(_) => {
                return failed(format!(
                    "the call took over {}s",
                    CLASSIFY_TIMEOUT.as_secs()
                ))
            }
        };
        if let Some(found) = classify::label_of(&response) {
            info!(%model, attempt, category = %found.category, difficulty = %found.difficulty, "Classified the prompt");
            return Ok(found);
        }
        reply = classify::reply_shown(&response.message);
        warn!(%model, attempt, %reply, "The classifier's reply gave no label");
    }
    failed(format!(
        "its reply gave no category and difficulty ({reply:?})"
    ))
}

/// The model that writes the acceptance criteria of a turn asking for
/// `category`, Gate routing it among `candidates`
/// ([`criteria::writer`]). `None` when the criteria are switched off, the
/// work is not code, or no model could review the work against them.
pub(crate) fn criteria_writer_for(
    live: &GateLive,
    candidates: &[Candidate],
    category: PromptCategory,
) -> Option<String> {
    let settings = live.settings();
    if settings.criteria == ReviewMode::Off || !settings.judge || !verifiable(category) {
        return None;
    }
    aura_agent_gate::compare::default_against(candidates)?;
    criteria::writer(candidates).map(|writer| writer.id.clone())
}

/// Ask `model` through `kernel`, so the call is recorded, for the
/// acceptance criteria of `text`. `Ok(None)` when the reply gave none:
/// the work is then reviewed as it would be without them.
///
/// # Errors
/// Says why, as a line for the chat, when the request could not be built
/// or the call failed or timed out. The work goes on without criteria
/// then too.
pub(crate) async fn write_criteria(
    kernel: &Kernel,
    model: &str,
    text: &str,
    auth_token: Option<String>,
) -> Result<Option<Arc<Criteria>>, String> {
    let failed = |error: String| {
        warn!(%model, %error, "Could not write the acceptance criteria");
        Err(format!(
            "gate: {} wrote no acceptance criteria: {error}",
            aura_model::model_catalog::route_label(model)
        ))
    };
    let request = match criteria::request(model, text, auth_token) {
        Ok(request) => request,
        Err(e) => return failed(format!("could not build the request ({e})")),
    };
    let response = match tokio::time::timeout(CRITERIA_TIMEOUT, kernel.reason(request)).await {
        Ok(Ok(result)) => result.response,
        Ok(Err(e)) => return failed(format!("the call failed ({e})")),
        Err(_) => {
            return failed(format!(
                "the call took over {}s",
                CRITERIA_TIMEOUT.as_secs()
            ))
        }
    };
    let Some(found) = criteria::criteria_of(&response) else {
        warn!(%model, "The reply gave no acceptance criteria");
        return Ok(None);
    };
    info!(%model, criteria = found.items.len(), strict = found.strict, "Wrote the acceptance criteria");
    Ok(Some(Arc::new(found)))
}

/// The acceptance criteria of `text`, a prompt asking for `category` that
/// Gate routes among `candidates`, written through `kernel` when Gate's
/// settings call for them ([`criteria_writer_for`]). Work they find
/// exacting is rated hard in `difficulty`, with criteria `log` as with
/// `on`, so it starts on a model that can meet them. `Ok(None)` when none
/// are due or the writer gave none.
///
/// # Errors
/// Says why, as a line for the chat, when the writer could not be asked
/// ([`write_criteria`]).
pub(crate) async fn hold_to_criteria(
    kernel: &Kernel,
    (live, candidates): (&GateLive, &[Candidate]),
    category: PromptCategory,
    difficulty: &mut Option<Difficulty>,
    text: &str,
    auth_token: Option<String>,
) -> Result<Option<Arc<Criteria>>, String> {
    let Some(writer) = criteria_writer_for(live, candidates, category) else {
        return Ok(None);
    };
    let Some(found) = write_criteria(kernel, &writer, text, auth_token).await? else {
        return Ok(None);
    };
    if found.exacting() && *difficulty != Some(Difficulty::Hard) {
        info!(was = ?difficulty, "The acceptance criteria rate the work hard");
        *difficulty = Some(Difficulty::Hard);
    }
    Ok(Some(found))
}

/// What the worker is told of `criteria` after its prompt, with criteria
/// `on`; `None` when it is not told.
pub(crate) fn worker_note_for(live: &GateLive, criteria: Option<&Criteria>) -> Option<String> {
    let criteria = criteria.filter(|_| live.settings().criteria == ReviewMode::On)?;
    Some(format!("\n\n{}", criteria::worker_note(criteria)))
}

/// Write `outcome` to the record of `agent_id` through `kernel`. A failure
/// is logged.
pub(crate) async fn record(kernel: &Kernel, agent_id: AgentId, outcome: &OutcomePayload) -> bool {
    match kernel
        .process_direct(Transaction::outcome(agent_id, outcome))
        .await
    {
        Ok(_) => {
            info!(
                %agent_id,
                signal = %outcome.signal,
                category = %outcome.category,
                model = %outcome.model,
                "Recorded turn outcome"
            );
            true
        }
        Err(e) => {
            warn!(%agent_id, error = %e, "Failed to record turn outcome");
            false
        }
    }
}

/// The latest turn on a record: the outcome written when it ended, and its
/// prompt when the scan still reaches it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LastTurn {
    pub outcome: OutcomePayload,
    pub prompt: Option<PromptPayload>,
}

/// The latest turn recorded for `agent_id`, if one ended within the last
/// [`SCAN_LIMIT`] entries.
pub(crate) fn last_turn<S: ReadStore + ?Sized>(store: &S, agent_id: AgentId) -> Option<LastTurn> {
    let entries = newest_first(store, agent_id);
    let outcome = entries
        .iter()
        .filter_map(|entry| entry.tx.outcome_payload())
        .find(|outcome| outcome.signal.is_turn_end())?;
    let prompt = entries
        .iter()
        .find(|entry| {
            entry.tx.tx_type == TransactionType::UserPrompt && entry.tx.hash == outcome.prompt_tx
        })
        .and_then(|entry| entry.tx.prompt_payload());
    Some(LastTurn { outcome, prompt })
}

/// The outcome written when the turn of each prompt in `prompts` ended,
/// for those the scan reaches, in the order of `prompts`.
pub(crate) fn turn_outcomes<S: ReadStore + ?Sized>(
    store: &S,
    agent_id: AgentId,
    prompts: &[Hash],
) -> Vec<OutcomePayload> {
    let ended: Vec<OutcomePayload> = newest_first(store, agent_id)
        .iter()
        .filter_map(|entry| entry.tx.outcome_payload())
        .filter(|outcome| outcome.signal.is_turn_end())
        .collect();
    prompts
        .iter()
        .filter_map(|prompt| ended.iter().find(|o| o.prompt_tx == *prompt).cloned())
        .collect()
}

fn newest_first<S: ReadStore + ?Sized>(
    store: &S,
    agent_id: AgentId,
) -> Vec<aura_core_types::RecordEntry> {
    let head = match store.get_head_seq(agent_id) {
        Ok(head) => head,
        Err(e) => {
            warn!(%agent_id, error = %e, "Failed to read the record head");
            return Vec::new();
        }
    };
    store
        .scan_record_descending(agent_id, head, SCAN_LIMIT)
        .unwrap_or_else(|e| {
            warn!(%agent_id, error = %e, "Failed to scan the record for outcomes");
            Vec::new()
        })
}

/// Before a turn of `session` answers `text` on `model`: when `text`
/// repeats the previous turn's prompt on a stronger model, the previous
/// turn failed.
pub(super) async fn note_retry(session: &AgentSession, text: &str, model: &str) {
    let agent_id = session.agent_id();
    let Some(LastTurn {
        outcome,
        prompt: Some(prompt),
    }) = last_turn(session.kernel.store().as_ref(), agent_id)
    else {
        return;
    };
    if is_retry_stronger(&prompt.text, &outcome.model, text, model) {
        info!(%agent_id, from = %outcome.model, to = %model, "Prompt retried on a stronger model");
        record(
            &session.kernel,
            agent_id,
            &outcome.with_signal(OutcomeSignal::RetryStronger),
        )
        .await;
    }
}

/// After `/rewind` undid the turns whose prompts are `prompts`: each of
/// them failed.
pub(super) async fn note_rewind(session: &AgentSession, prompts: &[Hash]) {
    let agent_id = session.agent_id();
    for outcome in turn_outcomes(session.kernel.store().as_ref(), agent_id, prompts) {
        record(
            &session.kernel,
            agent_id,
            &outcome.with_signal(OutcomeSignal::Rewind),
        )
        .await;
    }
}
