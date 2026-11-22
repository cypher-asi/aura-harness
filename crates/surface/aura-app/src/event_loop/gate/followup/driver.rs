//! One pass of Gate's follow-up over an attempt that has just ended,
//! shared by the terminal's turns, `/compare` runs and `aura -p`: the
//! steps [`FollowUp::next`] gives are carried out until one hands the
//! work back to its model or none is left.

use super::check::{check_review, check_signal, harness_check};
use super::judge::{judge_unchecked, Unchecked};
use super::messages::{
    nudge_message, nudge_notice, repair_message, repair_notice, sign_repair_message,
    sign_repair_notice,
};
use super::report::{Found, Report};
use super::review::Reviewing;
use super::{At, Before, FollowUp};
use crate::event_loop::agents::AgentSession;
use aura_agent::AgentLoopResult;
use aura_core_types::{Hash, OutcomeSignal};
use aura_gate::evidence::Flag;
use aura_gate::followup::{Step, Suspicion, Why};
use aura_gate::judge::criteria::Criteria;
use aura_gate::live::{GateLive, Judged};
use aura_gate::route::Candidate;
use aura_model::ThinkingEffort;
use std::path::Path;
use std::sync::Arc;

/// An attempt at a routed turn that has just ended, as its follow-up
/// sees it.
pub(crate) struct Attempt<'a> {
    pub session: &'a AgentSession,
    pub live: &'a GateLive,
    /// The models Gate routes the turn among
    pub candidates: &'a [Candidate],
    /// What the user asked
    pub task: &'a str,
    /// The model it ran on, at the effort Gate chose when it chose one
    pub ran: (&'a str, Option<ThinkingEffort>),
    /// How it ended
    pub result: &'a AgentLoopResult,
    pub at: At,
    /// The recorded prompt it answers, whose outcome the judge's verdict
    /// becomes; neither the judge nor the reviewer is asked without one
    pub prompt_tx: Option<Hash>,
    /// Where it works
    pub workspace: &'a Path,
    /// The workspace before the turn, when the harness recorded it
    pub before: Option<&'a Before>,
    /// What the work is held to, written down before it started
    pub criteria: Option<&'a Arc<Criteria>>,
    pub auth_token: Option<String>,
}

impl Attempt<'_> {
    /// What the attempt's file tools changed, as a unified diff.
    fn edits(&self) -> String {
        let (session, workspace) = (self.session, self.workspace);
        (session.file_checkpoints.as_ref())
            .map(|files| files.attempt(session.agent_id()).diff(Some(workspace)))
            .unwrap_or_default()
    }
}

/// What an attempt's follow-up found.
#[derive(Debug, Default)]
pub(crate) struct Followed {
    /// The attempt's signal after it; `None` while nothing says how it
    /// went
    pub signal: Option<OutcomeSignal>,
    /// What its model is told when it carries on with the work
    pub carry_on: Option<CarryOn>,
    /// What the harness's check printed when it failed
    pub harness: Option<String>,
    /// What the judge or the reviewer found wrong with the work
    pub wrong: Option<String>,
    /// The judges' findings, the reviews' among them, in the order given
    pub judged: Vec<Judged>,
}

/// What the same model is told when it carries on with its work, and
/// what the user is told of that.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CarryOn {
    pub message: String,
    pub notice: String,
    /// Whether it is asked to check its change, rather than sent back
    /// with what is wrong
    pub nudge: bool,
}

impl CarryOn {
    /// `model` asked to check its change, or to test what `uncovered`
    /// names when there is any.
    fn nudge(model: &str, uncovered: &[String]) -> Self {
        Self {
            message: nudge_message(uncovered),
            notice: nudge_notice(model, uncovered),
            nudge: true,
        }
    }

    /// `model`'s work sent back for the `signs` it shows.
    fn signs(model: &str, signs: &[Flag]) -> Self {
        Self {
            message: sign_repair_message(signs),
            notice: sign_repair_notice(model, signs),
            nudge: false,
        }
    }

    /// `model`'s work sent back with what a review found.
    fn findings(model: &str, findings: &str) -> Self {
        Self {
            message: repair_message(findings),
            notice: repair_notice(model),
            nudge: false,
        }
    }
}

/// The follow-up of a turn's attempts: the steps the attempt on `model`
/// has taken. An attempt on another model starts them afresh.
#[derive(Debug, Clone, Default)]
pub(crate) struct Following {
    model: String,
    /// The steps of the latest attempt
    pub steps: FollowUp,
}

impl Following {
    /// Carry out the follow-up of `attempt` until a step hands the work
    /// back to its model or none is left, showing on `report` what each
    /// step finds.
    pub(crate) async fn run(&mut self, attempt: &Attempt<'_>, report: &impl Report) -> Followed {
        self.renew(attempt);
        let model = attempt.ran.0;
        let mut followed = Followed {
            signal: attempt.at.signal,
            ..Followed::default()
        };
        while followed.carry_on.is_none() {
            let at = At {
                signal: followed.signal,
                ..attempt.at
            };
            match self.steps.next(attempt.result, at).await {
                Step::Stop => break,
                Step::Nudge(uncovered) => {
                    followed.carry_on = Some(CarryOn::nudge(model, &uncovered));
                }
                Step::Repair(signs) => followed.carry_on = Some(CarryOn::signs(model, &signs)),
                Step::Review { why, act } => {
                    self.review(attempt, (&why, act), &mut followed, report)
                        .await;
                }
                Step::HarnessCheck => self.check(attempt, &mut followed, report).await,
                Step::JudgeUnverified(suspicions) => {
                    self.judge(attempt, &suspicions, &mut followed, report)
                        .await;
                }
            }
        }
        followed
    }

    /// Start the steps afresh when `attempt` ran on another model than
    /// the attempt before it.
    fn renew(&mut self, attempt: &Attempt<'_>) {
        let (model, _) = attempt.ran;
        if self.model == model {
            return;
        }
        let steps = FollowUp::new(
            attempt.live,
            attempt.candidates,
            attempt.ran,
            attempt.workspace,
        )
        .since(attempt.before.cloned())
        .held_to(attempt.criteria.cloned())
        .after(&self.steps);
        *self = Self {
            model: model.to_string(),
            steps,
        };
    }

    /// The review of work about to stand, asked for `why` and acted on
    /// when `act` says.
    async fn review(
        &mut self,
        attempt: &Attempt<'_>,
        (why, act): (&[Why], bool),
        followed: &mut Followed,
        report: &impl Report,
    ) {
        let Some(prompt_tx) = attempt.prompt_tx else {
            return;
        };
        let edits = attempt.edits();
        let shown = Reviewing {
            task: attempt.task,
            result: attempt.result,
            turn_start: attempt.at.turn_start,
            category: attempt.at.category,
            edits: &edits,
            why,
            act,
        };
        let (session, live, model) = (attempt.session, attempt.live, attempt.ran.0);
        let token = attempt.auth_token.clone();
        let reviewed = (self.steps)
            .review(session, live, model, prompt_tx, &shown, token)
            .await;
        let Some(reviewed) = reviewed else {
            return;
        };
        report.report(Found::Reviewed(&reviewed)).await;
        followed.judged.push(reviewed.judged());
        if !reviewed.unfinished() {
            if followed.signal.is_none() {
                followed.signal = reviewed.verdict();
            }
        } else if self.steps.send_back(attempt.result) {
            followed.carry_on = Some(CarryOn::findings(model, &reviewed.findings));
        } else {
            followed.signal = Some(OutcomeSignal::JudgedFail);
            followed.wrong = Some(reviewed.findings);
        }
    }

    /// The harness's own check of the change.
    async fn check(&self, attempt: &Attempt<'_>, followed: &mut Followed, report: &impl Report) {
        let Some(command) = self.steps.check() else {
            return;
        };
        report.report(Found::Checking(command)).await;
        let check = harness_check(attempt.session, command).await;
        let signal = check_signal(&check);
        let model = attempt.ran.0;
        let checked = Found::Checked {
            model,
            command,
            check: &check,
            signal,
        };
        report.report(checked).await;
        if let Some(signal) = signal {
            followed.signal = Some(signal);
            followed.harness =
                (signal == OutcomeSignal::ChecksFailed).then(|| check_review(&check));
        }
    }

    /// The judge's verdict on work no check vouches for, told of
    /// `suspicions`.
    async fn judge(
        &mut self,
        attempt: &Attempt<'_>,
        suspicions: &[Suspicion],
        followed: &mut Followed,
        report: &impl Report,
    ) {
        let judge = self.steps.judge().filter(|_| attempt.prompt_tx.is_some());
        let Some(judge) = judge.map(str::to_string) else {
            return;
        };
        let diff = self.steps.diff(attempt.result, attempt.edits()).await;
        let shown = Unchecked {
            task: attempt.task,
            diff: &diff,
            result: attempt.result,
            turn_start: attempt.at.turn_start,
            suspicions,
        };
        let (session, model) = (attempt.session, attempt.ran.0);
        let token = attempt.auth_token.clone();
        let judged = judge_unchecked(session, &judge, model, &shown, token).await;
        let Some((verdict, judged)) = judged else {
            return;
        };
        report.report(Found::Judged(&judged)).await;
        let failed = verdict == OutcomeSignal::JudgedFail;
        if failed || followed.signal.is_none() {
            followed.signal = Some(verdict);
        }
        if failed {
            followed.wrong = Some(judged.verdict.reasons.clone());
        }
        followed.judged.push(judged);
    }
}
