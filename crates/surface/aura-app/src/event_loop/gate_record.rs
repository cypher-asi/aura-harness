//! Gate's record of each turn: the category the classifier gives its
//! prompt, the `Outcome` written when it ends, and the later signals that
//! judge it again — the judge's finding on a turn that failed, a rewind,
//! the same prompt sent again on a stronger model. Each later signal
//! appends another `Outcome` for the same prompt;
//! [`aura_agent_gate::outcome::effective`] says which one counts.

use super::agents::AgentSession;
use super::gate_live::GateLive;
use aura_agent_gate::turn_judge::{self, Failed, Unverified, Verdict};
use aura_agent_gate::{classify, outcome::is_retry_stronger, Candidate};
use aura_agent_kernel::Kernel;
use aura_core_types::{
    AgentId, Hash, OutcomePayload, OutcomeSignal, PromptPayload, Transaction, TransactionType,
};
use aura_store_db::ReadStore;
use std::time::Duration;
use tracing::{info, warn};

/// How long the classifier may take before the prompt goes unclassified.
const CLASSIFY_TIMEOUT: Duration = Duration::from_secs(20);

/// How long the judge of a failed turn may take before the turn keeps
/// its own signal.
const JUDGE_TIMEOUT: Duration = Duration::from_secs(90);

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
    let judged = ask_judge(kernel, judge, ended, request, false).await?;
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

/// The judge of unverified work for `ended`, the outcome of work Gate
/// routed among `candidates` that ran no check: the same model as
/// [`judge_for`]'s. `None` when the judge or this review of it is
/// switched off, or the work ran on a model as strong as the judge.
pub(crate) fn unverified_judge_for(
    live: &GateLive,
    candidates: &[Candidate],
    model: &str,
) -> Option<String> {
    let settings = live.settings();
    if !settings.judge || !settings.suspect_judge {
        return None;
    }
    let judge = aura_agent_gate::compare::default_against(candidates)?;
    turn_judge::judges(&judge.id, model, candidates).then(|| judge.id.clone())
}

/// Ask `judge` through `kernel` whether the work of `ended`, which ran no
/// check and looks wrong as `unverified` says, is done. Nothing is
/// recorded: work found unfinished fails with `judged_fail` where the
/// caller records the attempt's outcome, and work found done records
/// nothing, as no check proved it. `None` when the judge gave no verdict.
pub(crate) async fn judge_unverified(
    kernel: &Kernel,
    judge: &str,
    ended: &OutcomePayload,
    unverified: &Unverified<'_>,
    auth_token: Option<String>,
) -> Option<Judged> {
    let request = turn_judge::unverified_request(judge, unverified, auth_token);
    let judged = ask_judge(kernel, judge, ended, request, true).await?;
    info!(%judge, model = %ended.model, at_fault = judged.verdict.at_fault, cost_usd = judged.cost_usd, "The judge reviewed unchecked work");
    Some(judged)
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
) -> Option<Judged> {
    let request = match request {
        Ok(request) => request,
        Err(e) => {
            warn!(%judge, error = %e, "Could not build the judge's request");
            return None;
        }
    };
    let response = match tokio::time::timeout(JUDGE_TIMEOUT, kernel.reason(request)).await {
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
    let Some(verdict) = turn_judge::parse(&response.message.text_content()) else {
        warn!(%judge, "The judge gave no verdict");
        return None;
    };
    let cost_usd =
        aura_model::model_catalog::estimate_cost_usd(judge, &response.usage).unwrap_or(0.0);
    Some(Judged {
        model: ended.model.clone(),
        judge: judge.to_string(),
        verdict,
        cost_usd,
        unverified,
    })
}

/// Label `text` with its category and difficulty by asking `model`
/// through `kernel`, so the call is recorded. Any failure leaves it
/// unclassified.
pub(crate) async fn classify_prompt(
    kernel: &Kernel,
    model: &str,
    text: &str,
    auth_token: Option<String>,
) -> classify::Label {
    let request = match classify::request(model, text, auth_token) {
        Ok(request) => request,
        Err(e) => {
            warn!(%model, error = %e, "Could not build the classifier request");
            return classify::Label::UNCLASSIFIED;
        }
    };
    match tokio::time::timeout(CLASSIFY_TIMEOUT, kernel.reason(request)).await {
        Ok(Ok(result)) => {
            let label = classify::label_of(&result.response);
            info!(%model, category = %label.category, difficulty = ?label.difficulty, "Classified the prompt");
            label
        }
        Ok(Err(e)) => {
            warn!(%model, error = %e, "Classifier call failed");
            classify::Label::UNCLASSIFIED
        }
        Err(_) => {
            warn!(%model, "Classifier call timed out");
            classify::Label::UNCLASSIFIED
        }
    }
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
