//! What Gate judges and records as an attempt at a subtask ends: the
//! judge's verdict on work Gate routed that nothing checked, the attempt's
//! outcome, and the judge's review of a failed first attempt before it is
//! retried.

use super::SubtaskGate;
use crate::event_loop::agent_events::AgentUi;
use crate::event_loop::agents::AgentSession;
use crate::event_loop::handlers::system_message;
use crate::file_checkpoints::AttemptFiles;
use aura_core_types::{Hash, OutcomePayload, OutcomeSignal, PromptCategory};
use aura_gate::followup;
use aura_gate::judge::turn::{Failed, Unverified, Verdict};
use aura_gate::live::{self, Judged};
use std::time::Instant;

/// How an attempt at a subtask ended, as Gate judges and records it.
pub(crate) struct SubtaskEnd<'a> {
    /// The attempt's session, on whose record its outcome goes
    pub session: &'a AgentSession,
    /// The attempt's chat, which shows what the judge found
    pub ui: &'a AgentUi,
    /// What the subtask was asked
    pub task: &'a str,
    /// Model it ran on
    pub model: &'a str,
    /// The sign-in its model calls used, which the judge's calls use too
    pub auth_token: Option<&'a str>,
    /// The recorded prompt and its category, when Gate labelled it
    pub prompt: Option<(Hash, PromptCategory)>,
    /// What the turn's end and its check say about how it went; `None`
    /// when nothing does, and the judge has to
    pub signal: Option<OutcomeSignal>,
    /// List-price cost of its model calls
    pub cost_usd: f64,
    /// When it started
    pub started: Instant,
    /// Its final reply; empty when it has none
    pub reply: &'a str,
    /// How many files its file tools changed
    pub files_changed: usize,
    /// The commands it ran and its last test or build, as the judge of
    /// unchecked work is shown them
    pub shown: &'a (String, String),
    /// What the files it changed held before, when it may be retried
    pub files_before: Option<&'a AttemptFiles>,
    /// How it failed, when its check or reply says it did
    pub failure: Option<&'a str>,
}

/// What Gate made of an attempt at a subtask as it ended.
#[derive(Default)]
pub(crate) struct Judgement {
    /// Its outcome as recorded, with what the judge found of it; `None`
    /// when Gate did not label its prompt
    pub outcome: Option<OutcomePayload>,
    /// What Gate's judge found, when the attempt failed and was reviewed
    pub judged: Option<Verdict>,
    /// Why the attempt failed when its check and reply did not say so:
    /// the judge found its unchecked work unfinished
    pub unfinished: Option<String>,
}

impl SubtaskEnd<'_> {
    /// The attempt's outcome before any judge's say, when Gate labelled
    /// its prompt.
    fn outcome(&self) -> Option<OutcomePayload> {
        let (prompt_tx, category) = self.prompt?;
        Some(OutcomePayload {
            prompt_tx,
            category,
            model: self.model.to_string(),
            success: None,
            signal: self.signal.unwrap_or(OutcomeSignal::Unavailable),
            cost_usd: self.cost_usd,
            subtask_cost_usd: 0.0,
            latency_ms: u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX),
        })
    }

    /// The attempt's file changes as a unified diff; empty when nothing
    /// kept what its files held before.
    fn diff(&self) -> String {
        self.files_before.map_or_else(String::new, |files| {
            files.diff(Some(&self.session.kernel.agent_workspace()))
        })
    }
}

impl SubtaskGate {
    /// Judge and record the attempt `end` describes: a subtask Gate
    /// labelled has its outcome recorded, with the judge's verdict when
    /// Gate routed it and nothing checked its work; and a failed first
    /// attempt Gate routed is reviewed before it is retried.
    pub(crate) async fn finish(&self, end: SubtaskEnd<'_>) -> Judgement {
        let mut judgement = Judgement::default();
        let Some(mut outcome) = end.outcome() else {
            return judgement;
        };
        if let Some((verdict, found)) = self.judge_unchecked(&end, &outcome).await {
            end.ui.send(system_message(found.describe())).await;
            outcome.signal = verdict;
            if verdict == OutcomeSignal::JudgedFail {
                judgement.unfinished =
                    Some("Gate's judge found its unchecked work unfinished.".to_string());
                judgement.judged = Some(found.verdict);
            }
        }
        outcome.success = outcome.signal.success();
        live::record(&end.session.kernel, end.session.agent_id(), &outcome).await;
        if let Some((judged_as, found)) = self.review_failure(&end, &outcome).await {
            end.ui.send(system_message(found.describe())).await;
            outcome = judged_as;
            judgement.judged = Some(found.verdict);
        }
        judgement.outcome = Some(outcome);
        judgement
    }

    /// The judge's verdict on the work of `end`, ended as `outcome`, when
    /// Gate routed it and nothing says how it went; `None` when there is
    /// no judge for it or the judge gave no verdict.
    async fn judge_unchecked(
        &self,
        end: &SubtaskEnd<'_>,
        outcome: &OutcomePayload,
    ) -> Option<(OutcomeSignal, Judged)> {
        let candidates = self
            .candidates
            .as_deref()
            .filter(|_| end.signal.is_none())?;
        let judge = live::verdict_judge_for(candidates, end.model)?;
        let diff = end.diff();
        let suspicions = followup::suspicions(&followup::Ended {
            signal: None,
            category: outcome.category,
            files_changed: end.files_changed,
            reply: end.reply,
            hit_limit: false,
            difficulty: None,
            review_signs: &[],
            acting_signs: &[],
            uncovered: &[],
            ran_own_check: false,
            criteria: false,
        });
        let unverified = Unverified {
            task: end.task,
            diff: &diff,
            reply: end.reply,
            commands: &end.shown.0,
            last_check: &end.shown.1,
            suspicions: &suspicions,
        };
        let auth_token = end.auth_token.map(str::to_string);
        live::judge_unverified(
            &end.session.kernel,
            &judge,
            &outcome.model,
            &unverified,
            auth_token,
        )
        .await
    }

    /// The judge's review of a first attempt Gate routed that failed, and
    /// ended as `outcome`: whether the model was at fault, as the outcome
    /// recorded for it and the finding. `None` when the attempt did not
    /// fail, is itself a retry, has no judge, or the judge gave no
    /// verdict.
    async fn review_failure(
        &self,
        end: &SubtaskEnd<'_>,
        outcome: &OutcomePayload,
    ) -> Option<(OutcomePayload, Judged)> {
        let failure = end.failure?;
        let candidates = self
            .candidates
            .as_deref()
            .filter(|_| self.escalation.is_none())?;
        let judge = live::judge_for(&self.live, candidates, outcome)?;
        let diff = end.diff();
        let failed = Failed {
            task: end.task,
            diff: &diff,
            reply: end.reply,
            failure,
        };
        let auth_token = end.auth_token.map(str::to_string);
        live::judge_failed(&end.session.kernel, &judge, outcome, &failed, auth_token).await
    }
}
