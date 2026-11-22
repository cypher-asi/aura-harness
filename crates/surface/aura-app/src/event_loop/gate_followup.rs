//! What a turn Gate routed does when it ends with no check of its own
//! (see [`aura_agent_gate::followup`]), shared by the terminal's turns,
//! `/compare` runs and `aura -p`.
//!
//! Each of them asks [`FollowUp::next`] once an attempt ends and carries
//! the step out: a nudge goes on in the same conversation on the same
//! model, told [`nudge_prompt`]; the harness check runs here
//! ([`harness_check`]); and so does the judge of unchecked work
//! ([`judge_unchecked`]), whose verdict is the outcome of work nothing
//! else checked. What either finds wrong sends the work on to a stronger
//! model as a failed check of the turn's own does.
//!
//! Work that is about to stand may be reviewed first
//! ([`FollowUp::review`]): the judge goes through the request against
//! everything the attempt changed. To show it that, the harness compares
//! the workspace with how it was before the attempt ([`Before`]): a
//! `/compare` run's copy against its baseline, a git working tree against
//! a snapshot taken when the turn started. Changes made by the commands
//! the attempt ran are then in the diff with those its file tools made.
//! Work the review finds unfinished goes back to its model once
//! ([`repair_message`]) and on to a stronger model after that.

use super::agents::AgentSession;
use super::gate_live::GateLive;
use super::gate_log;
use super::gate_record::{self, Judged};
use super::processes::now_ms;
use super::steering::SteerSource;
use super::task_check::{run_script_within, CheckRun};
use super::tasks::TASKS_DIR;
use crate::isolation::changes;
use aura_agent::AgentLoopResult;
use aura_agent_gate::attempt_log::{CopyAt, Review, Tier, Tiers};
use aura_agent_gate::checks::{checks_seen, command_line, last_check_seen, CheckSeen};
use aura_agent_gate::coverage;
use aura_agent_gate::criteria::{checked, Checked, Criteria, Status};
use aura_agent_gate::evidence::{flags, is_test_path, Flag};
use aura_agent_gate::followup::{self, Allowed, Ended, Step, Suspicion, Taken, Why, NUDGE_PROMPT};
use aura_agent_gate::router::verifiable;
use aura_agent_gate::turn_judge::{self, Answer, ReviewVariant, Standing, Unverified, Verdict};
use aura_agent_gate::Candidate;
use aura_config::ReviewMode;
use aura_core_types::{
    CompareCopy, Difficulty, OutcomePayload, OutcomeSignal, PromptCategory, RouteLevel,
};
use aura_model::{ContentBlock, Message, Role, ThinkingEffort};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};

/// `ToolProposal::source` of the harness's checks.
const CHECK_SOURCE: &str = "gate_check";

/// Longest the harness's check may run. One that takes longer says
/// nothing, so a slow suite costs the user at most this long.
const CHECK_TIMEOUT_SECS: u64 = 5 * 60;

/// Exit codes of a command the shell could not run (126: not
/// executable, 127: not found).
const NOT_RUN_EXIT_CODES: [i32; 2] = [126, 127];

/// Exit codes of pytest that say nothing about the change: 4 a usage
/// error, 5 no tests collected.
const PYTEST_NOT_RUN_EXIT_CODES: [i32; 2] = [4, 5];

/// What a check printed when its tools are missing, not its tests
/// failing.
const NOT_RUN_OUTPUT: &[&str] = &[
    "command not found",
    "No module named pytest",
    "Missing script",
    "is not recognized as",
];

/// Commands shown to the judge of unchecked work, latest last.
const SHOWN_COMMANDS: usize = 15;

/// Commands shown to the reviewer of work about to stand, latest last.
const REVIEWED_COMMANDS: usize = 40;

/// Checks shown to the reviewer of work about to stand, latest last.
const REVIEWED_CHECKS: usize = 3;

/// How the reviewer of work about to stand is asked while the app runs.
pub(crate) const LIVE_REVIEW: ReviewVariant = ReviewVariant::Plain;

/// How long recording a workspace before a turn may take; a workspace
/// that takes longer is not watched.
const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(5);

/// How long reading what an attempt changed may take.
const DIFF_TIMEOUT: Duration = Duration::from_secs(30);

/// Most bytes of untracked files a workspace may hold and still be
/// recorded before a turn: recording stores a copy of each in the
/// repository's objects.
const MAX_UNTRACKED_BYTES: u64 = 64 * 1024 * 1024;

/// A workspace as it was before an attempt, to read everything the
/// attempt changed from.
#[derive(Debug, Clone)]
pub(crate) enum Before {
    /// A `/compare` run's copy and the tree it started from
    Copy(CopyAt),
    /// A git working tree as it was when the turn started
    Snapshot(Arc<changes::Snapshot>),
}

/// Everything an attempt changed, by its file tools and by the commands
/// it ran.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Changed {
    /// As a unified diff
    pub diff: String,
    /// The files, relative to the root of the working tree or copy
    pub paths: Vec<String>,
}

impl Before {
    /// The copy a `/compare` run works in, as it was before the run.
    pub(crate) fn of_copy(copy: &CompareCopy) -> Self {
        Self::Copy(gate_log::copy_at(copy))
    }

    /// The copy `copy` as it is now, part of the way through its run.
    /// `None` when git could not record it.
    pub(crate) async fn of_copy_now(copy: &CompareCopy) -> Option<Self> {
        let mut at = gate_log::copy_at(copy);
        let (root, git_dir) = (at.root.clone(), at.git_dir.clone());
        let tree =
            tokio::task::spawn_blocking(move || changes::baseline(&root, git_dir.as_deref()));
        match tokio::time::timeout(SNAPSHOT_TIMEOUT, tree).await {
            Ok(Ok(Ok(tree))) => {
                at.baseline = tree;
                Some(Self::Copy(at))
            }
            _ => None,
        }
    }

    /// `workspace` as it is now, when it is in a git working tree that
    /// can be recorded within [`SNAPSHOT_TIMEOUT`] and holds no more
    /// than [`MAX_UNTRACKED_BYTES`] of untracked files. Only objects are
    /// written to the repository (see [`changes::snapshot`]).
    pub(crate) async fn of_workspace(workspace: PathBuf) -> Option<Self> {
        let taken = tokio::task::spawn_blocking(move || {
            if changes::untracked_bytes(&workspace)? > MAX_UNTRACKED_BYTES {
                info!(workspace = %workspace.display(), "Too many untracked bytes to record the workspace before the turn");
                return None;
            }
            match changes::snapshot(&workspace) {
                Ok(snapshot) => snapshot,
                Err(e) => {
                    warn!(workspace = %workspace.display(), error = %e, "Could not record the workspace before the turn");
                    None
                }
            }
        });
        match tokio::time::timeout(SNAPSHOT_TIMEOUT, taken).await {
            Ok(Ok(Some(snapshot))) => Some(Self::Snapshot(Arc::new(snapshot))),
            Ok(_) => None,
            Err(_) => {
                warn!("Recording the workspace before the turn took too long");
                None
            }
        }
    }

    /// Everything that changed since. `None` when git could not say
    /// within [`DIFF_TIMEOUT`].
    pub(crate) async fn changed(&self) -> Option<Changed> {
        let before = self.clone();
        let read = tokio::task::spawn_blocking(move || match before {
            Self::Copy(copy) => changes::readable_diff_since(
                &copy.root,
                copy.git_dir.as_deref(),
                &copy.baseline,
                &[TASKS_DIR],
            ),
            Self::Snapshot(snapshot) => snapshot.diff(),
        });
        match tokio::time::timeout(DIFF_TIMEOUT, read).await {
            Ok(Ok(Ok(diff))) => Some(Changed {
                paths: changes::paths_in(&diff),
                diff,
            }),
            Ok(Ok(Err(e))) => {
                warn!(error = %e, "Could not read what the attempt changed");
                None
            }
            _ => {
                warn!("Reading what the attempt changed took too long");
                None
            }
        }
    }
}

/// Whether a turn on `model`, routed among `candidates`, may have its
/// work reviewed before it stands, so the workspace is worth recording
/// before it starts.
pub(crate) fn may_review(live: &GateLive, candidates: &[Candidate], model: &str) -> bool {
    let settings = live.settings();
    (settings.review != ReviewMode::Off || settings.criteria != ReviewMode::Off)
        && gate_record::reviewer_for(live, candidates, model).is_some()
}

/// How an attempt ended, for [`FollowUp::next`].
#[derive(Debug, Clone, Copy)]
pub(crate) struct At {
    pub category: PromptCategory,
    /// How hard the classifier judged the work
    pub difficulty: Option<Difficulty>,
    /// Its signal so far; `None` while nothing says how it went
    pub signal: Option<OutcomeSignal>,
    /// Where the attempt's own messages start in its result
    pub turn_start: usize,
}

/// The steps one attempt at a routed turn may still take.
#[derive(Debug, Clone, Default)]
pub(crate) struct FollowUp {
    allowed: Allowed,
    taken: Taken,
    /// Test command the harness runs
    check: Option<String>,
    /// Model that gives the verdict on unchecked work
    judge: Option<String>,
    /// Model that reviews work about to stand
    reviewer: Option<String>,
    /// Files changed before the attempt was last sent on (asked to check
    /// its change, or sent back), as its file tools reported them
    changed_before: usize,
    /// The workspace before the attempt, when the harness recorded it
    before: Option<Before>,
    /// What each sign of failure has earned in the attempt log
    tiers: Tiers,
    /// What the attempt changed as of the run that left this many
    /// messages
    changed: Option<(usize, Option<Changed>)>,
    /// Where the attempt works, whose tests are searched for what it
    /// added
    workspace: PathBuf,
    /// What the attempt added that no test names, as of the run that
    /// left this many messages
    uncovered: Option<(usize, Vec<String>)>,
    /// What the work is checked against, written down before it started
    criteria: Option<Arc<Criteria>>,
}

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
    pub(crate) fn verdict(&self) -> Option<OutcomeSignal> {
        self.act.then(|| self.answer.verdict())
    }

    /// One line saying what the reviewer found.
    pub(crate) fn describe(&self) -> String {
        let label = |id: &str| aura_model::model_catalog::route_label(id);
        let model = label(&self.model);
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
            label(&self.judge),
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

/// How many of the criteria `checked` the reviewer found met, e.g.
/// `, criteria 3/5 met, 1 unmet, 1 untested`; empty when there were none.
fn tally(checked: &[Checked]) -> String {
    if checked.is_empty() {
        return String::new();
    }
    let count = |status: Status| checked.iter().filter(|c| c.status == Some(status)).count();
    let mut said = format!(", criteria {}/{} met", count(Status::Met), checked.len());
    for status in [Status::Unmet, Status::Untested] {
        let n = count(status);
        if n > 0 {
            said.push_str(&format!(", {n} {}", status.word().to_lowercase()));
        }
    }
    said
}

/// What the reviewer of work about to stand is shown of an attempt.
pub(crate) struct Reviewing<'a> {
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
    /// The steps for an attempt that `ran` on a model (at the effort Gate
    /// chose, when it chose one), routed among `candidates`,
    /// working in `workspace`: the judge's verdict on unchecked work
    /// always, and when a candidate is a stronger model, so the work has
    /// somewhere to go, the others Gate's settings allow.
    pub(crate) fn new(
        live: &GateLive,
        candidates: &[Candidate],
        ran: (&str, Option<ThinkingEffort>),
        workspace: &Path,
    ) -> Self {
        let judge = gate_record::verdict_judge_for(candidates);
        let stronger =
            aura_agent_gate::has_stronger_model(&live.stats(), RouteLevel::Turn, ran, candidates);
        if !stronger {
            return Self {
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
        }
        let settings = live.settings();
        let check = settings
            .harness_check
            .then(|| check_command(workspace))
            .flatten();
        let reviewer = gate_record::reviewer_for(live, candidates, ran.0);
        Self {
            allowed: Allowed {
                nudge: settings.nudge,
                harness_check: check.is_some(),
                judge: judge.is_some(),
                review: if reviewer.is_some() {
                    settings.review
                } else {
                    ReviewMode::Off
                },
                criteria: if reviewer.is_some() {
                    settings.criteria
                } else {
                    ReviewMode::Off
                },
            },
            taken: Taken::default(),
            check,
            judge,
            reviewer,
            changed_before: 0,
            before: None,
            tiers: live.tiers(),
            changed: None,
            workspace: workspace.to_path_buf(),
            uncovered: None,
            criteria: None,
        }
    }

    /// The same steps, reading what the attempt changed against
    /// `before`.
    pub(crate) fn since(mut self, before: Option<Before>) -> Self {
        self.before = before;
        self
    }

    /// The same steps for work held to `criteria`, written down before
    /// it started.
    pub(crate) fn held_to(mut self, criteria: Option<Arc<Criteria>>) -> Self {
        self.criteria = criteria;
        self
    }

    /// The same steps for an attempt that takes over from the one
    /// `earlier` followed: a turn's work is sent back to its model once,
    /// so if that attempt was, this one is not.
    pub(crate) fn after(mut self, earlier: &Self) -> Self {
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
        self.changed
            .as_ref()
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
        self.changed
            .as_ref()
            .filter(|(at, _)| *at == result.messages.len())
            .and_then(|(_, changed)| changed.as_ref())
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
    pub(crate) fn evidence(
        &self,
        result: &AgentLoopResult,
        turn_start: usize,
    ) -> aura_agent_gate::evidence::Evidence {
        gate_log::evidence_of(result, turn_start, self.changed_paths(result))
    }

    /// The next step for an attempt that ended with `result` as `at`
    /// says; the step counts as taken.
    pub(crate) async fn next(&mut self, result: &AgentLoopResult, at: At) -> Step {
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
        let uncovered = if at.signal == Some(OutcomeSignal::ChecksPassed) && verifiable(at.category)
        {
            self.uncovered_as_of(result).await
        } else {
            Vec::new()
        };
        let reply = final_reply(result);
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
            criteria: self.criteria.is_some(),
        };
        let step = followup::next(&ended, self.taken, self.allowed);
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
        step
    }

    /// Whether work a review found unfinished, which ended in `result`,
    /// goes back to its model: once per attempt, and not when the
    /// attempt ran out of room. Counts as sent back when it does.
    pub(crate) fn send_back(&mut self, result: &AgentLoopResult) -> bool {
        if self.taken.repaired || hit_limit(result) {
            return false;
        }
        self.taken.repaired = true;
        self.changed_before += result.file_changes.len();
        true
    }

    /// Have the judge go through the work of `ended`, shown as `shown`,
    /// against the request before it stands, through `session`'s kernel,
    /// and log what it made of it to `live`'s attempt log. `None` when no
    /// model reviews or it gave no verdict.
    pub(crate) async fn review(
        &self,
        session: &AgentSession,
        live: &GateLive,
        ended: &OutcomePayload,
        shown: &Reviewing<'_>,
        auth_token: Option<String>,
    ) -> Option<Reviewed> {
        let judge = self.reviewer.as_deref()?;
        let messages = shown
            .result
            .messages
            .get(shown.turn_start..)
            .unwrap_or_default();
        let whole = self
            .changed
            .as_ref()
            .filter(|(at, _)| *at == shown.result.messages.len())
            .and_then(|(_, changed)| changed.as_ref());
        let reply = final_reply(shown.result);
        let commands = commands_run(messages, REVIEWED_COMMANDS);
        let checks = checks_shown(messages);
        let why: Vec<String> = shown
            .why
            .iter()
            .map(|why| why.describe().to_string())
            .collect();
        // Only a review the criteria called for is held to them, so one
        // acted on for other reasons asks what it always asked.
        let criteria = self
            .criteria
            .as_deref()
            .filter(|_| shown.why.contains(&Why::Criteria));
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
        let request = turn_judge::review_request(judge, &standing, LIVE_REVIEW, auth_token);
        let (answer, findings, cost_usd) =
            gate_record::ask_review(&session.kernel, judge, request).await?;
        info!(%judge, model = %ended.model, ?answer, cost_usd, act = shown.act, "The judge reviewed work about to stand");
        let signs: Vec<Flag> = flags(
            &self.evidence(shown.result, shown.turn_start),
            shown.category,
        );
        let checked = criteria.map(|c| checked(c, &findings)).unwrap_or_default();
        let variant = match criteria {
            Some(_) => format!("{}+criteria", LIVE_REVIEW.name()),
            None => LIVE_REVIEW.name().to_string(),
        };
        live.log_review(Review {
            at_ms: now_ms(),
            agent: session.agent_id().to_hex(),
            prompt_tx: ended.prompt_tx.to_hex(),
            why: shown.why.iter().map(|why| why.name().to_string()).collect(),
            signs,
            verdict: answer,
            findings: findings.clone(),
            judge: judge.to_string(),
            cost_usd,
            variant,
            criteria: checked.clone(),
            acted: shown.act,
            replayed: false,
        });
        Some(Reviewed {
            model: ended.model.clone(),
            judge: judge.to_string(),
            answer,
            findings,
            cost_usd,
            act: shown.act,
            checked,
        })
    }

    /// The test command the harness runs, when it may run one.
    pub(crate) fn check(&self) -> Option<&str> {
        self.check.as_deref()
    }

    /// The model that gives the verdict on unchecked work.
    pub(crate) fn judge(&self) -> Option<&str> {
        self.judge.as_deref()
    }
}

/// Whether `result` stopped on a limit rather than by finishing.
fn hit_limit(result: &AgentLoopResult) -> bool {
    result.budget_exhausted || result.stalled || result.output_truncated
}

/// The text of the last assistant message of `result`: its final reply,
/// without what the model narrated along the way.
pub(crate) fn final_reply(result: &AgentLoopResult) -> String {
    final_reply_in(&result.messages)
}

/// The text of the last assistant message in `messages` that has any.
pub(crate) fn final_reply_in(messages: &[Message]) -> String {
    messages
        .iter()
        .rev()
        .find(|m| m.role == Role::Assistant && !m.text_content().trim().is_empty())
        .map(Message::text_content)
        .unwrap_or_default()
}

/// The message that asks the model to check its change, marked as the
/// harness's.
pub(crate) fn nudge_prompt() -> String {
    format!("{}{NUDGE_PROMPT}", SteerSource::Harness.prefix())
}

/// The message of a [`Step::Nudge`] for `uncovered`, what the change
/// added that no test names: asked to test that when there is any, else
/// to check the change at all.
pub(crate) fn nudge_message(uncovered: &[String]) -> String {
    if uncovered.is_empty() {
        return nudge_prompt();
    }
    format!(
        "{}{}",
        SteerSource::Harness.prefix(),
        followup::uncovered_prompt(uncovered)
    )
}

/// What the user is told when `model`'s change is nudged for
/// `uncovered`.
pub(crate) fn nudge_notice(model: &str, uncovered: &[String]) -> String {
    let model = aura_model::model_catalog::route_label(model);
    match uncovered {
        [] => format!(
            "gate: {model} changed files without running a test or build; asking it to check its change"
        ),
        [name, ..] => format!(
            "gate: {model}'s tests passed but none names `{name}`{}, which it added; asking it to test the new code",
            if uncovered.len() > 1 {
                format!(" or {} other", uncovered.len() - 1)
            } else {
                String::new()
            }
        ),
    }
}

/// Files of a workspace [`untested`] reads at most; past them, what the
/// attempt added counts as named.
const MAX_SEARCHED: usize = 20_000;

/// Test files bigger than this are not read.
const MAX_TEST_BYTES: u64 = 1024 * 1024;

/// How long looking through a workspace's tests may take.
const SEARCH_TIMEOUT: Duration = Duration::from_secs(10);

/// `names` when no test file under `workspace` names any of them, else
/// none. Hidden folders (Aura's task records among them, which quote the
/// prompt) and build output are not searched; a search that fails or
/// takes too long finds them named.
async fn untested(workspace: PathBuf, names: Vec<String>) -> Vec<String> {
    let found = tokio::task::spawn_blocking(move || {
        let named = named_by_a_test(&workspace, &names);
        (!named).then_some(names)
    });
    match tokio::time::timeout(SEARCH_TIMEOUT, found).await {
        Ok(Ok(Some(names))) => names,
        Ok(_) => Vec::new(),
        Err(_) => {
            warn!("Looking for tests of what the attempt added took too long");
            Vec::new()
        }
    }
}

/// Whether a test file under `workspace` names any of `names`; also when
/// there are more files than are searched.
fn named_by_a_test(workspace: &Path, names: &[String]) -> bool {
    let searched = walkdir::WalkDir::new(workspace)
        .into_iter()
        .filter_entry(|entry| {
            let skipped = entry.depth() > 0
                && entry.file_type().is_dir()
                && entry.file_name().to_str().is_none_or(|name| {
                    name.starts_with('.') || changes::BUILD_OUTPUT.contains(&name)
                });
            !skipped
        });
    for (seen, entry) in searched.filter_map(Result::ok).enumerate() {
        if seen >= MAX_SEARCHED {
            return true;
        }
        if !entry.file_type().is_file() {
            continue;
        }
        let relative = entry.path().strip_prefix(workspace).unwrap_or(entry.path());
        if !is_test_path(&relative.to_string_lossy())
            || entry.metadata().map_or(true, |m| m.len() > MAX_TEST_BYTES)
        {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        if names.iter().any(|name| coverage::mentions(&text, name)) {
            return true;
        }
    }
    false
}

/// The message that sends work a review found unfinished back to its
/// model with the `findings`, marked as the harness's.
pub(crate) fn repair_message(findings: &str) -> String {
    format!(
        "{}{}",
        SteerSource::Harness.prefix(),
        followup::repair_prompt(findings)
    )
}

/// The message that sends work back to its model for the `signs` it
/// shows, marked as the harness's.
pub(crate) fn sign_repair_message(signs: &[Flag]) -> String {
    format!(
        "{}{}",
        SteerSource::Harness.prefix(),
        followup::sign_repair_prompt(signs)
    )
}

/// What the user is told when work goes back to `model` for `signs`.
pub(crate) fn sign_repair_notice(model: &str, signs: &[Flag]) -> String {
    let names: Vec<&str> = signs.iter().map(|sign| sign.name()).collect();
    format!(
        "gate: sending {}'s work back to it ({})",
        aura_model::model_catalog::route_label(model),
        names.join(", ")
    )
}

/// What the user is told when work a review found unfinished goes back
/// to `model`.
pub(crate) fn repair_notice(model: &str) -> String {
    format!(
        "gate: sending the review's findings back to {}",
        aura_model::model_catalog::route_label(model)
    )
}

/// The test command the harness runs in `workspace`: the operator's
/// (`AURA_DOD_TEST_COMMAND`), else the one its manifest implies.
fn check_command(workspace: &Path) -> Option<String> {
    aura_config::agent()
        .verify
        .test_command_override
        .map(|command| command.trim().to_string())
        .filter(|command| !command.is_empty())
        .or_else(|| aura_agent::infer_default_test_command(workspace))
}

/// Run `command` in `session`'s workspace as the harness's check.
pub(crate) async fn harness_check(session: &AgentSession, command: &str) -> CheckRun {
    run_script_within(session, command, CHECK_SOURCE, CHECK_TIMEOUT_SECS).await
}

/// What `check` says of the change: passed or failed, `None` when it
/// could not run, ran out of time, or failed for want of its tools.
pub(crate) fn check_signal(check: &CheckRun) -> Option<OutcomeSignal> {
    if could_not_run(check) {
        return None;
    }
    check.signal()
}

fn could_not_run(check: &CheckRun) -> bool {
    match (check.passed, check.exit_code) {
        (None, _) | (Some(false), None) => true,
        (Some(true), _) => false,
        (Some(false), Some(code)) => {
            NOT_RUN_EXIT_CODES.contains(&code)
                || (check.command.contains("pytest") && PYTEST_NOT_RUN_EXIT_CODES.contains(&code))
                || NOT_RUN_OUTPUT.iter().any(|text| check.tail.contains(text))
        }
    }
}

/// The last [`REVIEWED_CHECKS`] tests or builds in `messages` as the
/// reviewer sees them; empty when it ran none.
pub(crate) fn checks_shown(messages: &[Message]) -> String {
    let seen = checks_seen(messages);
    seen[seen.len().saturating_sub(REVIEWED_CHECKS)..]
        .iter()
        .map(check_shown)
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// The commands the `run_command` calls in `messages` ran, one per line,
/// the last `shown` of them.
pub(crate) fn commands_run(messages: &[Message], shown: usize) -> String {
    let commands: Vec<String> = messages
        .iter()
        .flat_map(|m| &m.content)
        .filter_map(|block| match block {
            ContentBlock::ToolUse { name, input, .. } if name == "run_command" => {
                Some(command_line(input))
            }
            _ => None,
        })
        .collect();
    commands[commands.len().saturating_sub(shown)..].join("\n")
}

/// The attempt's last check as the judge of unchecked work sees it: its
/// command, how it exited, and what it printed. A command whose exit
/// status may not be the check's is pointed out, since only the output
/// then says how the check went.
fn check_shown(check: &CheckSeen) -> String {
    let exited = match (check.passed, check.by_output) {
        (true, false) => "exited with success",
        (false, false) => "failed",
        (true, true) => {
            "may have exited as a command after the check did, so what the check printed \
             decided how it went: it shows tests passing"
        }
        (false, true) => {
            "may have exited as a command after the check did, so what the check printed \
             decided how it went: it shows failures, or that the tests never ran"
        }
    };
    format!(
        "`{}` {exited}. It printed:\n{}",
        check.command, check.output
    )
}

/// What the judge of unchecked work is shown of an attempt.
pub(crate) struct Unchecked<'a> {
    /// What the user asked
    pub task: &'a str,
    /// The attempt's file changes as a unified diff
    pub diff: &'a str,
    /// How it ended
    pub result: &'a AgentLoopResult,
    /// Where its own messages start in `result`
    pub turn_start: usize,
    pub suspicions: &'a [Suspicion],
}

/// Ask `judge` through `session`'s kernel whether the work of `ended`,
/// shown as `unchecked`, is done; returns the verdict as the attempt's
/// signal (see [`gate_record::judge_unverified`]).
pub(crate) async fn judge_unchecked(
    session: &AgentSession,
    judge: &str,
    ended: &OutcomePayload,
    unchecked: &Unchecked<'_>,
    auth_token: Option<String>,
) -> Option<(OutcomeSignal, Judged)> {
    let reply = final_reply(unchecked.result);
    let messages = unchecked
        .result
        .messages
        .get(unchecked.turn_start..)
        .unwrap_or_default();
    let (commands, last_check) = shown_to_judge(messages);
    let shown = Unverified {
        task: unchecked.task,
        diff: unchecked.diff,
        reply: &reply,
        commands: &commands,
        last_check: &last_check,
        suspicions: unchecked.suspicions,
    };
    gate_record::judge_unverified(&session.kernel, judge, ended, &shown, auth_token).await
}

/// What the judge of unchecked work is shown of the commands in
/// `messages`: the last of those they ran, one per line, and their last
/// test or build (empty when they ran none).
pub(crate) fn shown_to_judge(messages: &[Message]) -> (String, String) {
    let commands = commands_run(messages, SHOWN_COMMANDS);
    let last_check = last_check_seen(messages)
        .map(|check| check_shown(&check))
        .unwrap_or_default();
    (commands, last_check)
}

/// What the stronger model is told of a harness check that failed.
pub(crate) fn check_review(check: &CheckRun) -> String {
    format!("The harness ran the project's tests. {}", check.describe())
}

/// `first`, then `second`, on lines of their own; whichever is present.
pub(crate) fn joined(first: Option<String>, second: Option<String>) -> Option<String> {
    match (first, second) {
        (Some(first), Some(second)) => Some(format!("{first}\n{second}")),
        (first, second) => first.or(second),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aura_agent_gate::criteria::{Criterion, Kind};
    use serde_json::json;

    #[test]
    fn a_review_held_to_criteria_says_how_many_it_found_met() {
        let checked = |status| Checked {
            criterion: Criterion {
                text: "the body is byte-identical to MIMEMultipart's".to_string(),
                kind: Kind::ReferenceMatch,
                reference: None,
            },
            status,
        };
        let reviewed = Reviewed {
            model: "claude/claude-haiku-4-5".to_string(),
            judge: "claude/claude-opus-5-5".to_string(),
            answer: Answer::Model,
            findings: "CRITERION 2: UNTESTED".to_string(),
            cost_usd: 0.1,
            act: false,
            checked: vec![
                checked(Some(Status::Met)),
                checked(Some(Status::Untested)),
                checked(None),
            ],
        };
        let shown = reviewed.describe();
        assert!(
            shown.contains(", criteria 1/3 met, 1 untested): "),
            "{shown}"
        );
        assert!(
            shown.contains("AURA_GATE_CRITERIA=on acts on it"),
            "{shown}"
        );
        let plain = Reviewed {
            checked: Vec::new(),
            ..reviewed
        };
        let shown = plain.describe();
        assert!(!shown.contains("criteria"), "{shown}");
        assert!(shown.contains("AURA_GATE_REVIEW=on acts on it"), "{shown}");
    }

    #[tokio::test]
    async fn what_no_test_names_is_untested() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let write = |path: &str, text: &str| {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        write("lib/urls.py", "def prepare_multipart(fields):\n    pass\n");
        write("test/units/test_urls.py", "from urls import open_url\n");
        write(".aura/tasks/1.json", "implement prepare_multipart");
        write("node_modules/x/test.js", "prepare_multipart()");
        let names = vec!["prepare_multipart".to_string()];

        let found = untested(root.to_path_buf(), names.clone()).await;
        assert_eq!(
            found, names,
            "the prompt's records and dependencies are not tests"
        );

        write(
            "test/units/test_multipart.py",
            "from urls import prepare_multipart\n",
        );
        assert!(untested(root.to_path_buf(), names).await.is_empty());
    }

    #[test]
    fn work_with_no_stronger_model_to_go_to_gets_only_the_judge() {
        use aura_agent_gate::Candidate;
        use aura_model::model_catalog::{ModelBackend, ModelTier};
        const SONNET: &str = "claude/claude-sonnet-4-6";
        const OPUS: &str = "claude/claude-opus-4-7";
        let candidate = |id: &str, tier, cost_usd| Candidate {
            id: id.to_string(),
            label: id.to_string(),
            backend: ModelBackend::Claude,
            tier,
            cost_usd: Some(cost_usd),
        };
        let candidates = [
            candidate(SONNET, ModelTier::Standard, 3.0),
            candidate(OPUS, ModelTier::Frontier, 15.0),
        ];
        let live = GateLive::new(aura_config::GateConfig {
            explore_rate: 0.0,
            ..aura_config::GateConfig::defaults()
        });
        let workspace = tempfile::tempdir().unwrap();
        let at_high = |model| {
            FollowUp::new(
                &live,
                &candidates,
                (model, Some(ThinkingEffort::High)),
                workspace.path(),
            )
        };
        assert!(at_high(SONNET).allowed.nudge);
        let top = at_high(OPUS);
        assert_eq!(
            top.allowed,
            Allowed {
                judge: top.judge.is_some(),
                review: ReviewMode::Off,
                criteria: ReviewMode::Off,
                ..Allowed::default()
            },
            "max on the same model is not somewhere to go"
        );
    }

    #[test]
    fn a_nudge_says_what_no_test_names() {
        let plain = nudge_message(&[]);
        assert!(plain.ends_with(NUDGE_PROMPT), "{plain}");
        let names = ["prepare_multipart".to_string(), "Part".to_string()];
        let told = nudge_message(&names);
        assert!(told.contains("`prepare_multipart`, `Part`"), "{told}");
        assert!(followup::is_nudge(&told));
        let notice = nudge_notice("claude/claude-haiku-5-5", &names);
        assert!(
            notice.contains("none names `prepare_multipart` or 1 other"),
            "{notice}"
        );
    }

    fn check(passed: Option<bool>, exit_code: Option<i32>, command: &str, tail: &str) -> CheckRun {
        CheckRun {
            command: command.to_string(),
            passed,
            exit_code,
            tail: tail.to_string(),
        }
    }

    #[test]
    fn a_check_that_ran_says_how_the_change_went() {
        let passed = check(Some(true), Some(0), "cargo test", "ok");
        assert_eq!(check_signal(&passed), Some(OutcomeSignal::ChecksPassed));
        let failed = check(
            Some(false),
            Some(101),
            "cargo test",
            "test parse ... FAILED",
        );
        assert_eq!(check_signal(&failed), Some(OutcomeSignal::ChecksFailed));
    }

    #[test]
    fn a_check_that_could_not_run_says_nothing() {
        for unrun in [
            check(None, None, "cargo test", "denied"),
            check(Some(false), None, "cargo test", "Command timed out"),
            check(Some(false), Some(127), "go test ./...", "sh: go: not found"),
            check(
                Some(false),
                Some(1),
                "python -m pytest -q",
                "No module named pytest",
            ),
            check(Some(false), Some(5), "python -m pytest -q", "no tests ran"),
            check(
                Some(false),
                Some(1),
                "npm test --silent",
                "npm error Missing script: \"test\"",
            ),
        ] {
            assert_eq!(check_signal(&unrun), None, "{unrun:?}");
        }
    }

    #[test]
    fn the_judge_is_told_when_a_checks_exit_status_is_not_its_own() {
        let plain = CheckSeen {
            command: "pytest -q".to_string(),
            passed: true,
            by_output: false,
            output: "12 passed".to_string(),
        };
        assert_eq!(
            check_shown(&plain),
            "`pytest -q` exited with success. It printed:\n12 passed"
        );

        let piped = CheckSeen {
            command: "pytest -q 2>&1 | tail -3".to_string(),
            by_output: true,
            ..plain.clone()
        };
        assert_eq!(
            check_shown(&piped),
            "`pytest -q 2>&1 | tail -3` may have exited as a command after the check did, so \
             what the check printed decided how it went: it shows tests passing. It printed:\n\
             12 passed"
        );

        let never_ran = CheckSeen {
            passed: false,
            output: "no tests ran in 0.00s\nERROR: file not found: test/units/urls".to_string(),
            ..piped
        };
        assert!(
            check_shown(&never_ran).contains("it shows failures, or that the tests never ran."),
            "{}",
            check_shown(&never_ran)
        );
        let failed = CheckSeen {
            by_output: false,
            ..never_ran
        };
        assert!(check_shown(&failed).starts_with("`pytest -q 2>&1 | tail -3` failed. It printed:"));
    }

    #[test]
    fn the_judge_sees_the_last_commands_run() {
        let call = |n: usize| {
            Message::new(
                Role::Assistant,
                vec![ContentBlock::ToolUse {
                    id: n.to_string(),
                    name: "run_command".to_string(),
                    input: json!({"command": format!("cmd {n}")}),
                }],
            )
        };
        let messages: Vec<Message> = (0..20).map(call).collect();
        let shown = commands_run(&messages, SHOWN_COMMANDS);
        assert_eq!(shown.lines().count(), SHOWN_COMMANDS);
        assert!(
            shown.starts_with("cmd 5\n") && shown.ends_with("cmd 19"),
            "{shown}"
        );
        assert_eq!(commands_run(&[], SHOWN_COMMANDS), "");
    }

    #[test]
    fn the_final_reply_is_the_last_assistant_text() {
        let result = AgentLoopResult {
            messages: vec![
                Message::new(
                    Role::Assistant,
                    vec![ContentBlock::Text {
                        text: "I couldn't find it at first.".to_string(),
                    }],
                ),
                Message::user("ok"),
                Message::new(
                    Role::Assistant,
                    vec![ContentBlock::Text {
                        text: "Done: fixed the parser.".to_string(),
                    }],
                ),
            ],
            ..AgentLoopResult::default()
        };
        assert_eq!(final_reply(&result), "Done: fixed the parser.");
    }

    #[test]
    fn reviews_join_on_lines() {
        let some = |s: &str| Some(s.to_string());
        assert_eq!(joined(some("a"), some("b")), some("a\nb"));
        assert_eq!(joined(None, some("b")), some("b"));
        assert_eq!(joined(None, None), None);
    }
}
