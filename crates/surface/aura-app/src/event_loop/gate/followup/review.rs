//! The review of work about to stand: the judge goes through the request
//! against everything the attempt changed, and what it found is logged.

use super::FollowUp;
use crate::event_loop::agents::AgentSession;
use crate::event_loop::processes::now_ms;
use aura_agent::AgentLoopResult;
use aura_core_types::{Hash, OutcomeSignal, PromptCategory};
use aura_gate::attempts::Review;
use aura_gate::evidence::flags;
use aura_gate::evidence::transcript::reply::final_reply;
use aura_gate::followup::Why;
use aura_gate::judge::criteria::{checked, tally, Checked, Criteria};
use aura_gate::judge::turn::{self, shown_to_reviewer, Answer, ReviewVariant, Standing, Verdict};
use aura_gate::live::{self, GateLive, Judged};
use aura_model::model_catalog::route_label;
use aura_model::{ModelRequest, ReasonerError};
use tracing::info;

/// How the reviewer of work about to stand is asked while the app runs.
const LIVE_REVIEW: ReviewVariant = ReviewVariant::Plain;

/// What the reviewer made of work that was about to stand.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Reviewed {
    /// Model whose work was reviewed
    pub model: String,
    /// Model that reviewed it
    pub judge: String,
    pub answer: Answer,
    /// What it found
    pub findings: String,
    /// List-price cost of the review in USD
    pub cost_usd: f64,
    /// Whether Gate acts on the answer
    pub act: bool,
    /// The acceptance criteria it was held to, each with what it found
    pub checked: Vec<Checked>,
}

impl Reviewed {
    /// Whether the work goes back to its model or on to a stronger one.
    pub(crate) fn unfinished(&self) -> bool {
        self.act && self.answer == Answer::Model
    }

    /// The verdict a review Gate acts on gives work no check vouched
    /// for, standing in for the judge's; `None` when it is only logged.
    pub(super) fn verdict(&self) -> Option<OutcomeSignal> {
        self.act.then(|| self.answer.verdict())
    }

    /// One line saying what the reviewer found.
    pub(super) fn describe(&self) -> String {
        let model = route_label(&self.model);
        let findings = self.findings.trim();
        let setting = if self.checked.is_empty() {
            "AURA_GATE_REVIEW"
        } else {
            "AURA_GATE_CRITERIA"
        };
        let found = match self.answer {
            Answer::Model if self.act => format!("{model}'s work is not finished.\n{findings}"),
            Answer::Model => format!(
                "{model}'s work looks unfinished (logged only; {setting}=on acts on it).\n{findings}"
            ),
            Answer::Ok => format!("found nothing missing in {model}'s work"),
            Answer::Other => format!("{model}'s work was held up by something outside it.\n{findings}"),
        };
        format!(
            "Gate's review ({}, ${:.4}{}): {}",
            route_label(&self.judge),
            self.cost_usd,
            tally(&self.checked),
            found.trim_end()
        )
    }

    /// The review as a judge's finding, for reports that list those.
    pub(crate) fn judged(&self) -> Judged {
        Judged {
            model: self.model.clone(),
            judge: self.judge.clone(),
            verdict: Verdict {
                at_fault: self.answer == Answer::Model,
                reasons: self.findings.clone(),
            },
            cost_usd: self.cost_usd,
            unverified: true,
        }
    }
}

/// What the reviewer of work about to stand is shown of an attempt.
pub(super) struct Reviewing<'a> {
    /// What the user asked
    pub task: &'a str,
    /// How it ended
    pub result: &'a AgentLoopResult,
    /// Where its own messages start in `result`
    pub turn_start: usize,
    pub category: PromptCategory,
    /// What its file tools changed, as a unified diff: shown when the
    /// harness could not compare the workspace before and after
    pub edits: &'a str,
    pub why: &'a [Why],
    /// Whether Gate acts on the answer
    pub act: bool,
}

impl FollowUp {
    /// Have the judge go through the work of `model` on the prompt
    /// `prompt_tx`, shown as `shown`, against the request before it
    /// stands, through `session`'s kernel, and log what it made of it to
    /// `live`'s attempt log. `None` when no model reviews or it gave no
    /// verdict.
    pub(super) async fn review(
        &self,
        session: &AgentSession,
        live: &GateLive,
        model: &str,
        prompt_tx: Hash,
        shown: &Reviewing<'_>,
        auth_token: Option<String>,
    ) -> Option<Reviewed> {
        let judge = self.reviewer.as_deref()?;
        // Only a review the criteria called for is held to them, so one
        // acted on for other reasons asks what it always asked.
        let criteria = (self.criteria.as_deref()).filter(|_| shown.why.contains(&Why::Criteria));
        let request = self.review_request(judge, shown, criteria, auth_token);
        let (answer, findings, cost_usd) =
            live::ask_review(&session.kernel, judge, request).await?;
        info!(%judge, %model, ?answer, cost_usd, act = shown.act, "The judge reviewed work about to stand");
        let reviewed = Reviewed {
            model: model.to_string(),
            judge: judge.to_string(),
            answer,
            checked: criteria.map(|c| checked(c, &findings)).unwrap_or_default(),
            findings,
            cost_usd,
            act: shown.act,
        };
        let line = self.review_line(session, prompt_tx, shown, &reviewed, criteria.is_some());
        live.log_review(line);
        Some(reviewed)
    }

    /// The request that has `judge` review the attempt shown as `shown`,
    /// held to `criteria` when there are any.
    fn review_request(
        &self,
        judge: &str,
        shown: &Reviewing<'_>,
        criteria: Option<&Criteria>,
        auth_token: Option<String>,
    ) -> Result<ModelRequest, ReasonerError> {
        let messages = (shown.result.messages)
            .get(shown.turn_start..)
            .unwrap_or_default();
        let whole = self.changed_read(shown.result);
        let reply = final_reply(&shown.result.messages);
        let (commands, checks) = shown_to_reviewer(messages);
        let why: Vec<String> = (shown.why.iter())
            .map(|why| why.describe().to_string())
            .collect();
        let standing = Standing {
            task: shown.task,
            diff: whole.map_or(shown.edits, |changed| changed.diff.as_str()),
            whole: whole.is_some(),
            commands: &commands,
            checks: &checks,
            reply: &reply,
            why: &why,
            criteria,
        };
        turn::review_request(judge, &standing, LIVE_REVIEW, auth_token)
    }

    /// The attempt log's line for `reviewed`, a review of the attempt on
    /// the prompt `prompt_tx`, shown as `shown`, and whether it was held
    /// to criteria.
    fn review_line(
        &self,
        session: &AgentSession,
        prompt_tx: Hash,
        shown: &Reviewing<'_>,
        reviewed: &Reviewed,
        held: bool,
    ) -> Review {
        let variant = if held {
            format!("{}+criteria", LIVE_REVIEW.name())
        } else {
            LIVE_REVIEW.name().to_string()
        };
        Review {
            at_ms: now_ms(),
            agent: session.agent_id().to_hex(),
            prompt_tx: prompt_tx.to_hex(),
            why: shown.why.iter().map(|why| why.name().to_string()).collect(),
            signs: flags(
                &self.evidence(shown.result, shown.turn_start),
                shown.category,
            ),
            verdict: reviewed.answer,
            findings: reviewed.findings.clone(),
            judge: reviewed.judge.clone(),
            cost_usd: reviewed.cost_usd,
            variant,
            criteria: reviewed.checked.clone(),
            acted: shown.act,
            replayed: false,
        }
    }
}
