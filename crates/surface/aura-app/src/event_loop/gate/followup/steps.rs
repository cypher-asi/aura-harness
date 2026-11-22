//! The steps an attempt may still take: which Gate's settings allow it,
//! what it changed, and the next of them once it ends.

use super::check::check_command;
use super::untested::untested;
use super::{hit_limit, At, Before, Changed, FollowUp};
use aura_agent::AgentLoopResult;
use aura_config::ReviewMode;
use aura_core_types::{OutcomeSignal, RouteLevel};
use aura_gate::attempts::Tier;
use aura_gate::evidence::transcript::reply::final_reply;
use aura_gate::evidence::{coverage, flags, own_check, verifiable, Evidence};
use aura_gate::followup::{self, Allowed, Ended, Step};
use aura_gate::judge::criteria::Criteria;
use aura_gate::live::{self, GateLive};
use aura_gate::route::Candidate;
use aura_model::ThinkingEffort;
use std::path::Path;
use std::sync::Arc;

impl FollowUp {
    /// The steps for an attempt that `ran` on a model (at the effort Gate
    /// chose, when it chose one), routed among `candidates`,
    /// working in `workspace`: the judge's verdict on unchecked work
    /// always, and when a candidate is a stronger model, so the work has
    /// somewhere to go, the others Gate's settings allow.
    pub(super) fn new(
        live: &GateLive,
        candidates: &[Candidate],
        ran: (&str, Option<ThinkingEffort>),
        workspace: &Path,
    ) -> Self {
        let judge = live::verdict_judge_for(candidates, ran.0);
        let judged = Self {
            allowed: Allowed {
                judge: judge.is_some(),
                review: ReviewMode::Off,
                criteria: ReviewMode::Off,
                ..Allowed::default()
            },
            judge,
            tiers: live.tiers(),
            workspace: workspace.to_path_buf(),
            ..Self::default()
        };
        let stronger =
            aura_gate::route::has_stronger_model(&live.stats(), RouteLevel::Turn, ran, candidates);
        if stronger {
            judged.allowing(live, candidates, ran.0)
        } else {
            judged
        }
    }

    /// The same steps with the others Gate's settings allow work on
    /// `model` that has a stronger model to go on to: a review that is
    /// only logged on the share of attempts they say.
    fn allowing(mut self, live: &GateLive, candidates: &[Candidate], model: &str) -> Self {
        let settings = live.settings();
        self.check = (settings.harness_check)
            .then(|| check_command(&self.workspace))
            .flatten();
        let reviewer = live::reviewer_for(live, candidates, model);
        let logs = [settings.review, settings.criteria].contains(&ReviewMode::Log);
        let logged = reviewer.is_some() && logs && live.sampled(settings.review_sample);
        // `log` differs from `off` only by the review it logs.
        let mode = |mode: ReviewMode| match mode {
            _ if reviewer.is_none() => ReviewMode::Off,
            ReviewMode::Log if !logged => ReviewMode::Off,
            mode => mode,
        };
        self.allowed = Allowed {
            nudge: settings.nudge,
            harness_check: self.check.is_some(),
            judge: self.judge.is_some(),
            review: mode(settings.review),
            criteria: mode(settings.criteria),
        };
        self.reviewer = reviewer;
        self
    }

    /// The same steps, reading what the attempt changed against
    /// `before`.
    pub(super) fn since(mut self, before: Option<Before>) -> Self {
        self.before = before;
        self
    }

    /// The same steps for work held to `criteria`, written down before
    /// it started.
    pub(super) fn held_to(mut self, criteria: Option<Arc<Criteria>>) -> Self {
        self.criteria = criteria;
        self
    }

    /// The same steps for an attempt that takes over from the one
    /// `earlier` followed: a turn's work is sent back to its model once,
    /// so if that attempt was, this one is not.
    pub(super) fn after(mut self, earlier: &Self) -> Self {
        self.taken.repaired = earlier.taken.repaired;
        self
    }

    /// What the attempt changed as of `result`, read once per run of the
    /// attempt; `None` when the workspace was not recorded before it.
    async fn changed_as_of(&mut self, result: &AgentLoopResult) -> Option<&Changed> {
        let as_of = result.messages.len();
        if self.changed.as_ref().is_none_or(|(at, _)| *at != as_of) {
            let changed = match &self.before {
                Some(before) => before.changed().await,
                None => None,
            };
            self.changed = Some((as_of, changed));
        }
        self.changed_read(result)
    }

    /// What the attempt changed as of `result`, when [`Self::next`] read
    /// it for it.
    pub(super) fn changed_read(&self, result: &AgentLoopResult) -> Option<&Changed> {
        self.changed
            .as_ref()
            .filter(|(at, _)| *at == result.messages.len())
            .and_then(|(_, changed)| changed.as_ref())
    }

    /// What the attempt added as of `result` that no test in its
    /// workspace names, read once per run of the attempt; empty when a
    /// test names any of it, or when what it changed is not known.
    async fn uncovered_as_of(&mut self, result: &AgentLoopResult) -> Vec<String> {
        let as_of = result.messages.len();
        if let Some((at, names)) = &self.uncovered {
            if *at == as_of {
                return names.clone();
            }
        }
        let added = match self.changed_as_of(result).await {
            Some(changed) => coverage::added_names(&changed.diff),
            None => Vec::new(),
        };
        let names = if added.is_empty() {
            added
        } else {
            untested(self.workspace.clone(), added).await
        };
        self.uncovered = Some((as_of, names.clone()));
        names
    }

    /// The files the attempt changed by any means as of `result`, when
    /// [`Self::next`] read them for it.
    pub(crate) fn changed_paths(&self, result: &AgentLoopResult) -> Option<&[String]> {
        self.changed_read(result)
            .map(|changed| changed.paths.as_slice())
    }

    /// The attempt's change as of `result` as a judge is shown it:
    /// everything it changed when the workspace was recorded before it,
    /// else `edits`, what its file tools changed.
    pub(crate) async fn diff(&mut self, result: &AgentLoopResult, edits: String) -> String {
        match self.changed_as_of(result).await {
            Some(changed) => changed.diff.clone(),
            None => edits,
        }
    }

    /// The evidence the attempt that ended in `result` left, its
    /// messages starting at `turn_start`, with the files it changed by
    /// any means when they were read.
    pub(crate) fn evidence(&self, result: &AgentLoopResult, turn_start: usize) -> Evidence {
        live::evidence_of(result, turn_start, self.changed_paths(result))
    }

    /// The next step for an attempt that ended with `result` as `at`
    /// says; the step counts as taken.
    pub(super) async fn next(&mut self, result: &AgentLoopResult, at: At) -> Step {
        if at
            .signal
            .is_some_and(|signal| signal != OutcomeSignal::ChecksPassed)
        {
            return Step::Stop;
        }
        let reported = self.changed_before + result.file_changes.len();
        let changed = self.changed_as_of(result).await.map(|c| c.paths.len());
        // A file the tools reported counts even when it reads as
        // unchanged in the end.
        let files_changed = changed.map_or(reported, |changed| changed.max(reported));
        let raised = flags(&self.evidence(result, at.turn_start), at.category);
        let review_signs = self.tiers.in_tier(&raised, Tier::Review);
        let acting_signs = self.tiers.in_tier(&raised, Tier::Act);
        let checked = at.signal == Some(OutcomeSignal::ChecksPassed);
        let uncovered = if checked && verifiable(at.category) {
            self.uncovered_as_of(result).await
        } else {
            Vec::new()
        };
        let reply = final_reply(&result.messages);
        let ended = Ended {
            signal: at.signal,
            category: at.category,
            files_changed,
            reply: &reply,
            hit_limit: hit_limit(result),
            difficulty: at.difficulty,
            review_signs: &review_signs,
            acting_signs: &acting_signs,
            uncovered: &uncovered,
            ran_own_check: own_check::ran(result.messages.get(at.turn_start..).unwrap_or_default()),
            criteria: self.criteria.is_some(),
        };
        let step = followup::next(&ended, self.taken, self.allowed);
        self.take(&step, reported);
        step
    }

    /// Count `step` as taken, the attempt's file tools having reported
    /// `reported` files changed so far.
    fn take(&mut self, step: &Step, reported: usize) {
        match step {
            Step::Nudge(_) => {
                self.taken.nudged = true;
                self.changed_before = reported;
            }
            Step::HarnessCheck => self.taken.checked = true,
            Step::JudgeUnverified(_) => self.taken.judged = true,
            Step::Review { .. } => self.taken.reviews += 1,
            Step::Repair(_) => {
                self.taken.repaired = true;
                self.changed_before = reported;
            }
            Step::Stop => {}
        }
    }

    /// Whether work a review found unfinished, which ended in `result`,
    /// goes back to its model: once per attempt, and not when the
    /// attempt ran out of room. Counts as sent back when it does.
    pub(super) fn send_back(&mut self, result: &AgentLoopResult) -> bool {
        if self.taken.repaired || hit_limit(result) {
            return false;
        }
        self.taken.repaired = true;
        self.changed_before += result.file_changes.len();
        true
    }

    /// The test command the harness runs, when it may run one.
    pub(super) fn check(&self) -> Option<&str> {
        self.check.as_deref()
    }

    /// The model that gives the verdict on unchecked work.
    pub(super) fn judge(&self) -> Option<&str> {
        self.judge.as_deref()
    }
}
