//! The judge's calls: whether a failure was the model's fault, the
//! verdict on work no check vouches for, and the review of work about to
//! stand.

use super::{reason_within, CallError};
use crate::record::record;
use crate::table::GateLive;
use aura_agent_kernel::Kernel;
use aura_core_types::{OutcomePayload, OutcomeSignal};
use aura_gate_judge::turn::{self, Answer, Failed, Unverified, Verdict};
use aura_gate_route::Candidate;
use aura_model::model_catalog::{infer_tier, route_label, ModelTier};
use aura_model::{ModelRequest, ReasonerError};
use std::time::Duration;
use tracing::{info, warn};

/// How long the judge of a failed turn may take before the turn keeps
/// its own signal.
const JUDGE_TIMEOUT: Duration = Duration::from_secs(90);

/// How long the reviewer of work about to stand may take: it reads the
/// whole change, and may reason before it answers.
const REVIEW_TIMEOUT: Duration = Duration::from_secs(300);

/// The judge's finding on work that failed.
#[derive(Debug, Clone, PartialEq)]
pub struct Judged {
    /// Model whose work was judged
    pub model: String,
    /// Model that judged it
    pub judge: String,
    pub verdict: Verdict,
    /// List-price cost of the judge's call in USD; `None` when the
    /// judge's price is unknown. Not part of the work's own cost: it is
    /// what Gate pays to learn from the failure.
    pub cost_usd: Option<f64>,
    /// Whether the work ran no check and was judged for looking wrong
    /// rather than for failing.
    pub unverified: bool,
}

/// A judge's call cost for a one-line note: `$0.0123`, or `price
/// unknown` when the judge has no known list price.
#[must_use]
pub fn cost_text(cost_usd: Option<f64>) -> String {
    cost_usd.map_or_else(|| "price unknown".to_string(), |usd| format!("${usd:.4}"))
}

impl Judged {
    /// One line saying what the judge found.
    #[must_use]
    pub fn describe(&self) -> String {
        let found = match (self.unverified, self.verdict.at_fault) {
            (false, true) => format!("the failure is {}'s", route_label(&self.model)),
            (false, false) => format!("the failure is not {}'s doing", route_label(&self.model)),
            (true, true) => format!("{}'s unchecked work is not done", route_label(&self.model)),
            (true, false) => format!("{}'s unchecked work stands", route_label(&self.model)),
        };
        let reasons = self.verdict.reasons.trim();
        format!(
            "Gate's judge ({}, {}): {found}{}",
            route_label(&self.judge),
            cost_text(self.cost_usd),
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
/// [`aura_gate_compare::default_against`]). `None` when the work did not
/// fail, the judge is switched off, or the work ran on a model as strong
/// as the judge.
pub fn judge_for(
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
    let judge = aura_gate_compare::default_against(candidates)?;
    turn::judges(&judge.id, &ended.model, candidates).then(|| judge.id.clone())
}

/// Ask `judge` through `kernel` whether the model was at fault in
/// `ended`, the outcome of work that `failed`, and record the finding as
/// a later `Outcome` of the same prompt: `judged_fail` when it was,
/// `judged_excused` (which takes the failure back) when it was not.
/// Returns that outcome and the finding; `None` when the judge gave no
/// verdict, which leaves the work's own signal standing.
pub async fn judge_failed(
    kernel: &Kernel,
    judge: &str,
    ended: &OutcomePayload,
    failed: &Failed<'_>,
    auth_token: Option<String>,
) -> Option<(OutcomePayload, Judged)> {
    let request = turn::request(judge, failed, auth_token);
    let (_, judged) = ask_judge(kernel, judge, &ended.model, request, false).await?;
    let signal = if judged.verdict.at_fault {
        OutcomeSignal::JudgedFail
    } else {
        OutcomeSignal::JudgedExcused
    };
    info!(%judge, model = %ended.model, %signal, cost_usd = ?judged.cost_usd, "The judge reviewed failed work");
    let outcome = ended.with_signal(signal);
    record(kernel, kernel.agent_id, &outcome).await;
    Some((outcome, judged))
}

/// The model that gives the verdict on work done on `model`, Gate routing
/// among `candidates`, that no check vouches for, since every routed turn
/// ends with a verdict: `model` itself when it is a standard model or
/// stronger, so Sonnet judges Sonnet's work at Sonnet's price, and for a
/// weaker model's work the cheapest standard model, else the strongest
/// ([`aura_gate_route::fixed_pick`]). Whether a failure was the model's
/// fault is still [`judge_for`]'s to say. `None` only when a weak model's
/// work has no candidates to be judged by.
pub fn verdict_judge_for(candidates: &[Candidate], model: &str) -> Option<String> {
    if tier_of(model, candidates) >= ModelTier::Standard {
        return Some(model.to_string());
    }
    aura_gate_route::fixed_pick(candidates).map(|judge| judge.id.clone())
}

/// The model that reviews work on `model` before it stands, Gate routing
/// among `candidates`: the one that gives the verdict on its unchecked
/// work ([`verdict_judge_for`]). `None` when the judge is switched off or
/// the work ran on the strongest tier, whose work is not reviewed.
pub fn reviewer_for(live: &GateLive, candidates: &[Candidate], model: &str) -> Option<String> {
    if !live.settings().judge {
        return None;
    }
    let strongest = aura_gate_compare::default_against(candidates)?;
    if !turn::judges(&strongest.id, model, candidates) {
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

/// Ask `judge` through `kernel` whether the work of `model`, which no
/// check vouches for, is done, pointed to what looks wrong as
/// `unverified` says. Returns the verdict as the attempt's signal
/// (`judged_pass`, `judged_fail` or `judged_excused`, see
/// [`Answer::verdict`]) and the finding; the caller records it with the
/// attempt's outcome. `None` when the judge gave no verdict.
pub async fn judge_unverified(
    kernel: &Kernel,
    judge: &str,
    model: &str,
    unverified: &Unverified<'_>,
    auth_token: Option<String>,
) -> Option<(OutcomeSignal, Judged)> {
    let request = turn::unverified_request(judge, unverified, auth_token);
    let (answer, judged) = ask_judge(kernel, judge, model, request, true).await?;
    let signal = answer.verdict();
    info!(%judge, %model, %signal, cost_usd = ?judged.cost_usd, "The judge gave its verdict on unchecked work");
    Some((signal, judged))
}

/// Send `request` to `judge` through `kernel` and read its verdict on
/// the work of `model`. `None` when the request could not be built, the
/// call failed or timed out, or the reply held no verdict.
async fn ask_judge(
    kernel: &Kernel,
    judge: &str,
    model: &str,
    request: Result<ModelRequest, ReasonerError>,
    unverified: bool,
) -> Option<(Answer, Judged)> {
    let (answer, reasons, cost_usd) = ask(kernel, judge, request, JUDGE_TIMEOUT).await?;
    let judged = Judged {
        model: model.to_string(),
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
/// ([`turn::review_request`]), to `judge` through `kernel`. Returns its
/// answer, what it found and the list-price cost of the call (`None` in
/// the cost when the judge's price is unknown); `None` when
/// the request could not be built, the call failed or timed out, or the
/// reply held no verdict. Nothing is recorded.
pub async fn ask_review(
    kernel: &Kernel,
    judge: &str,
    request: Result<ModelRequest, ReasonerError>,
) -> Option<(Answer, String, Option<f64>)> {
    ask(kernel, judge, request, REVIEW_TIMEOUT).await
}

async fn ask(
    kernel: &Kernel,
    judge: &str,
    request: Result<ModelRequest, ReasonerError>,
    within: Duration,
) -> Option<(Answer, String, Option<f64>)> {
    let request = match request {
        Ok(request) => request,
        Err(e) => {
            warn!(%judge, error = %e, "Could not build the judge's request");
            return None;
        }
    };
    let response = match reason_within(kernel, request, within).await {
        Ok(response) => response,
        Err(CallError::Failed(e)) => {
            warn!(%judge, error = %e, "The judge's call failed");
            return None;
        }
        Err(CallError::TimedOut(_)) => {
            warn!(%judge, "The judge's call timed out");
            return None;
        }
    };
    let Some((answer, reasons)) = turn::parse_answer(&response.message.text_content()) else {
        warn!(%judge, "The judge gave no verdict");
        return None;
    };
    // Local judges cost nothing; an unknown price stays unknown rather
    // than reading as free.
    let cost_usd = aura_gate_compare::list_price(judge, &response.usage);
    Some((answer, reasons, cost_usd))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn judged(cost_usd: Option<f64>) -> Judged {
        Judged {
            model: "claude/claude-haiku-4-5".to_string(),
            judge: "claude/claude-opus-4-7".to_string(),
            verdict: Verdict {
                at_fault: true,
                reasons: String::new(),
            },
            cost_usd,
            unverified: false,
        }
    }

    #[test]
    fn a_judge_with_no_known_price_is_not_shown_as_free() {
        assert_eq!(cost_text(Some(0.0123)), "$0.0123");
        assert_eq!(cost_text(None), "price unknown");
        assert!(judged(Some(0.0123)).describe().contains(", $0.0123): "));
        let unknown = judged(None).describe();
        assert!(unknown.contains(", price unknown): "), "{unknown}");
        assert!(!unknown.contains("$0"), "{unknown}");
    }
}
