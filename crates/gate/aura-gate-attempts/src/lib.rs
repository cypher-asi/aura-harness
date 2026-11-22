//! Layer: gate
//!
//! Gate's attempt log: a local file of what each attempt at a prompt
//! left behind and of what later proved it right or wrong, kept so Gate's
//! verdicts, and any new way of reaching one, can be scored against what
//! happened.
//!
//! The file is `gate/attempts.jsonl` under the data directory, one JSON
//! object per line, appended to and never rewritten. It stays on this
//! machine: the data directory is not under version control. A line is
//! one of three kinds, told apart by its `kind`:
//!
//! - `attempt` ([`Attempt`]): one model's work on a prompt, written when
//!   its `Outcome` is recorded: what Gate made of it, and its
//!   [`Evidence`]
//! - `truth` ([`Truth`]): something outside the attempt that says whether
//!   its work was right: a benchmark task's own check, or the user
//!   rewinding it, sending it again to a stronger model, or picking
//!   between runs
//! - `review` ([`Review`]): what Gate's judge made of an attempt that was
//!   about to stand (see `aura-gate-followup`), asked as the attempt ended
//!   or later by `aura gate replay`
//!
//! A truth or a review belongs to the attempt with the same `agent` and
//! `prompt_tx`. A turn Gate moves to a stronger model is a new attempt
//! under the prompt recorded for the move, so the pair names one attempt.
//!
//! [`evaluate`] joins them and reports how often Gate let failed work
//! stand, for each [`Flag`] how many of those failures it would have
//! caught against how many good attempts it would have questioned, and
//! how the reviews held up. [`tiers`] reads from the same lines what each
//! flag has earned: nothing, a review of the attempts it is raised on, or
//! sending them straight back to the model.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, BufReader, Write as _};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use aura_core_types::{Difficulty, OutcomePayload, OutcomeSignal, PromptCategory};
use aura_gate_evidence::{flags, Evidence, Flag};
use aura_gate_judge::criteria::Checked;
use aura_gate_judge::turn::Answer;
use aura_model::model_catalog::{infer_tier, ModelTier};
use serde::{Deserialize, Serialize};

/// The log's path under the data directory.
pub const FILE: &str = "gate/attempts.jsonl";

/// One model's work on a prompt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Attempt {
    /// When the line was written, in Unix milliseconds
    pub at_ms: u64,
    /// The agent that ran it, in hex
    pub agent: String,
    /// Hash of the prompt it answers, in hex: the user's, or the one
    /// recorded when Gate moved the turn to this model
    pub prompt_tx: String,
    /// What the prompt asked for
    pub category: PromptCategory,
    /// How hard the classifier judged it, when it said
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub difficulty: Option<Difficulty>,
    /// Whether the agent led it, handing its work to subtasks
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub led: Option<bool>,
    /// Whether Gate chose the model
    pub routed: bool,
    /// The model that did the work
    pub model: String,
    /// What Gate recorded of how it went, after any follow-up
    #[serde(deserialize_with = "stored_signal")]
    pub signal: OutcomeSignal,
    /// Whether that counts as success; absent when nothing said
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub success: Option<bool>,
    /// List-price cost of its model calls in USD
    pub cost_usd: f64,
    /// List-price cost of its subtasks' model calls in USD
    #[serde(default)]
    pub subtask_cost_usd: f64,
    /// Its wall time
    pub latency_ms: u64,
    /// What it left behind
    pub evidence: Evidence,
    /// Whether the line was rebuilt from the record and a saved session
    /// rather than written as the attempt ended
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub backfilled: bool,
}

/// A stored attempt's signal. Older logs wrote `none` for work that
/// ended with nothing to say how it went, which reads as
/// [`OutcomeSignal::Unavailable`]: it counts neither way.
fn stored_signal<'de, D>(deserializer: D) -> Result<OutcomeSignal, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let name = String::deserialize(deserializer)?;
    if name == "none" {
        return Ok(OutcomeSignal::Unavailable);
    }
    OutcomeSignal::deserialize(serde::de::value::StrDeserializer::new(&name))
}

/// What an attempt ran under, the part its `Outcome` does not hold.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Terms {
    /// How hard the classifier judged the prompt
    pub difficulty: Option<Difficulty>,
    /// Whether the agent led it
    pub led: Option<bool>,
    /// Whether Gate chose the model
    pub routed: bool,
}

impl Attempt {
    /// The attempt of `agent` (in hex) whose outcome is `outcome`.
    #[must_use]
    pub fn new(
        at_ms: u64,
        agent: String,
        outcome: &OutcomePayload,
        terms: Terms,
        evidence: Evidence,
    ) -> Self {
        Self {
            at_ms,
            agent,
            prompt_tx: outcome.prompt_tx.to_hex(),
            category: outcome.category,
            difficulty: terms.difficulty,
            led: terms.led,
            routed: terms.routed,
            model: outcome.model.clone(),
            signal: outcome.signal,
            success: outcome.success,
            cost_usd: outcome.cost_usd,
            subtask_cost_usd: outcome.subtask_cost_usd,
            latency_ms: outcome.latency_ms,
            evidence,
            backfilled: false,
        }
    }
}

/// What said whether an attempt's work was right.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TruthSource {
    /// A benchmark task's own check, run on the work after the run
    TaskCheck,
    /// The user rewound the turn
    Rewind,
    /// The user sent the prompt again to a stronger model
    RetryStronger,
    /// The user picked between two runs of `/compare`, in older logs
    Picked,
}

impl TruthSource {
    /// The source an `Outcome` with `signal` is, when the signal is the
    /// user's.
    #[must_use]
    pub const fn of_user(signal: OutcomeSignal) -> Option<Self> {
        match signal {
            OutcomeSignal::Rewind => Some(Self::Rewind),
            OutcomeSignal::RetryStronger => Some(Self::RetryStronger),
            _ => None,
        }
    }

    /// The task's own check outranks what the user did, which says less
    /// about the work itself.
    const fn rank(self) -> u8 {
        match self {
            Self::TaskCheck => 2,
            Self::Rewind | Self::RetryStronger | Self::Picked => 1,
        }
    }
}

/// Something outside an attempt that says whether its work was right.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Truth {
    /// When the line was written, in Unix milliseconds
    pub at_ms: u64,
    /// The attempt's agent, in hex
    pub agent: String,
    /// The attempt's prompt, in hex
    pub prompt_tx: String,
    /// What said so
    pub source: TruthSource,
    /// Whether the work was right
    pub passed: bool,
    /// The comparison the attempt's run belongs to, e.g. `Compare #22`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compare: Option<String>,
    /// The run, e.g. `a-4`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    /// The benchmark task the run did
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    /// Where the run did its work, when it worked in a copy
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub copy: Option<CopyAt>,
}

/// The copy of the workspace a run of `/compare` worked in, kept so what
/// the run changed can be read again while the copy is there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopyAt {
    /// Root of the copy
    pub root: PathBuf,
    /// Git directory kept beside a plain copy; absent for a worktree
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_dir: Option<PathBuf>,
    /// Git tree of the copy before the run
    pub baseline: String,
}

/// What Gate's judge made of an attempt that was about to stand.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Review {
    /// When the line was written, in Unix milliseconds
    pub at_ms: u64,
    /// The attempt's agent, in hex
    pub agent: String,
    /// The attempt's prompt, in hex
    pub prompt_tx: String,
    /// Why it was reviewed: `hard`, or the names of the signs and
    /// suspicions that called for it
    pub why: Vec<String>,
    /// Every sign the attempt's evidence raised when it was reviewed
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub signs: Vec<Flag>,
    /// What the judge answered
    pub verdict: Answer,
    /// What it found
    pub findings: String,
    /// The model that reviewed
    pub judge: String,
    /// List-price cost of the review in USD
    pub cost_usd: f64,
    /// How the judge was asked, e.g. `plain`, `thinking`, or
    /// `plain+criteria` when it was given the attempt's acceptance
    /// criteria
    pub variant: String,
    /// The acceptance criteria it was given, each with what it found
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub criteria: Vec<Checked>,
    /// Whether Gate acted on the verdict: sent work found unfinished
    /// back to the model or on to a stronger one
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub acted: bool,
    /// Whether `aura gate replay` asked, after the attempt had ended
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub replayed: bool,
}

/// What kind a line is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    /// See [`Attempt`]
    Attempt,
    /// See [`Truth`]
    Truth,
    /// See [`Review`]
    Review,
}

/// A line of the log.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Line {
    /// See [`Attempt`]
    Attempt(Attempt),
    /// See [`Truth`]
    Truth(Truth),
    /// See [`Review`]
    Review(Review),
}

impl Line {
    /// The agent and prompt the line is about, and its kind.
    #[must_use]
    pub fn key(&self) -> (&str, &str, Kind) {
        match self {
            Self::Attempt(a) => (&a.agent, &a.prompt_tx, Kind::Attempt),
            Self::Truth(t) => (&t.agent, &t.prompt_tx, Kind::Truth),
            Self::Review(r) => (&r.agent, &r.prompt_tx, Kind::Review),
        }
    }
}

/// The log file, appended to one line at a time.
#[derive(Debug)]
pub struct AttemptLog {
    path: PathBuf,
    /// Held while a line is written, so lines of concurrent turns do not
    /// interleave
    writing: Mutex<()>,
}

impl AttemptLog {
    /// The log at `path`.
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            writing: Mutex::new(()),
        }
    }

    /// The log of the data directory `data_dir`.
    #[must_use]
    pub fn in_data_dir(data_dir: &Path) -> Self {
        Self::new(data_dir.join(FILE))
    }

    /// Where the log is.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Append `line`.
    ///
    /// # Errors
    ///
    /// When the file or its directory cannot be written.
    pub fn append(&self, line: &Line) -> io::Result<()> {
        let mut text = serde_json::to_string(line)?;
        text.push('\n');
        let _held = self.writing.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir)?;
        }
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?
            .write_all(text.as_bytes())
    }

    /// The log's lines, oldest first; none when there is no file yet.
    ///
    /// # Errors
    ///
    /// When the file cannot be read.
    pub fn read(&self) -> io::Result<Read> {
        read(&self.path)
    }
}

/// The lines of a log, and how many of its lines could not be read as
/// one.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Read {
    /// The lines, oldest first
    pub lines: Vec<Line>,
    /// Lines that are not a line of the log
    pub unreadable: usize,
}

/// The lines of the log at `path`, oldest first; none when there is no
/// file.
///
/// # Errors
///
/// When the file cannot be read.
pub fn read(path: &Path) -> io::Result<Read> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Read::default()),
        Err(e) => return Err(e),
    };
    let mut out = Read::default();
    for text in BufReader::new(file).lines() {
        let text = text?;
        if text.trim().is_empty() {
            continue;
        }
        match serde_json::from_str(&text) {
            Ok(line) => out.lines.push(line),
            Err(_) => out.unreadable += 1,
        }
    }
    Ok(out)
}

/// How Gate's verdicts on a set of attempts held up.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Verdicts {
    /// Attempts whose work was later shown right or wrong
    pub known: u32,
    /// Of those, the ones whose work was right
    pub right: u32,
    /// Gate counted the attempt as passed, and its work was wrong
    pub false_passes: u32,
    /// Gate had nothing to judge the attempt by, and its work was wrong
    pub unjudged_failures: u32,
    /// Gate counted the attempt as failed, and its work was right
    pub false_failures: u32,
}

impl Verdicts {
    /// Failed work Gate let stand.
    #[must_use]
    pub const fn missed(self) -> u32 {
        self.false_passes + self.unjudged_failures
    }

    fn take(&mut self, said: Option<bool>, right: bool) {
        self.known += 1;
        self.right += u32::from(right);
        match (said, right) {
            (Some(true), false) => self.false_passes += 1,
            (None, false) => self.unjudged_failures += 1,
            (Some(false), true) => self.false_failures += 1,
            _ => {}
        }
    }
}

/// How a [`Flag`] did on the attempts Gate let stand whose work was
/// later shown right or wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct FlagScore {
    /// The flag
    pub flag: Flag,
    /// Attempts it was raised on
    pub raised: u32,
    /// Of those, the ones whose work was wrong: failures it would have
    /// caught
    pub caught: u32,
    /// Of those, the ones whose work was right: good attempts it would
    /// have questioned
    pub false_alarms: u32,
}

impl FlagScore {
    /// Share of the attempts it was raised on whose work was wrong.
    #[must_use]
    pub fn precision(self) -> Option<f64> {
        (self.raised > 0).then(|| f64::from(self.caught) / f64::from(self.raised))
    }
}

/// An attempt Gate let stand whose work was later shown right or wrong.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Scored {
    /// The comparison and run it belongs to, when it has one, e.g.
    /// `Compare #22 a-4`
    pub run: String,
    /// The benchmark task it did, when it did one
    pub task: String,
    /// The model that did the work
    pub model: String,
    /// Whether Gate chose the model
    pub routed: bool,
    /// What Gate recorded of how it went
    pub signal: OutcomeSignal,
    /// Whether the work was right
    pub right: bool,
    /// What said so
    pub source: TruthSource,
    /// The flags its evidence raises
    pub flags: Vec<Flag>,
}

/// What a [`Flag`] has earned on the attempts it was raised on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    /// It is logged and scored, nothing more.
    #[default]
    Log,
    /// An attempt it is raised on is reviewed before it stands.
    Review,
    /// An attempt it is raised on goes straight back to the model.
    Act,
}

impl Tier {
    /// The tier's name, e.g. `review`.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Log => "log only",
            Self::Review => "review",
            Self::Act => "acts",
        }
    }
}

/// Raises a flag needs before it can call for a review.
pub const REVIEW_MIN_RAISED: u32 = 5;

/// How many times as often as attempts of the same difficulty the
/// attempts a flag is raised on must be wrong for it to call for a
/// review.
pub const REVIEW_LIFT: f64 = 2.0;

/// Raises a flag needs before it can act on its own.
pub const ACT_MIN_RAISED: u32 = 10;

/// Share of the attempts a flag is raised on that must be wrong for it
/// to act on its own.
pub const ACT_PRECISION: f64 = 0.8;

/// A [`Flag`]'s record on the attempts [`tiers`] counts.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct SignRecord {
    /// The flag
    pub flag: Flag,
    /// Attempts it was raised on
    pub raised: u32,
    /// Of those, the ones whose work was wrong
    pub wrong: u32,
    /// How many of them would be wrong if the flag said nothing: the sum
    /// over them of the share of attempts of the same difficulty that
    /// were wrong
    pub expected: f64,
    /// What that earns it
    pub tier: Tier,
}

impl SignRecord {
    fn tier_of(raised: u32, wrong: u32, expected: f64) -> Tier {
        // `expected` is a sum of shares; the slack keeps a record that
        // meets a bar exactly from missing it by rounding.
        let (raised_f, wrong_f) = (f64::from(raised), f64::from(wrong) + 1e-9);
        if raised >= ACT_MIN_RAISED && wrong_f >= ACT_PRECISION * raised_f {
            Tier::Act
        } else if raised >= REVIEW_MIN_RAISED && wrong >= 2 && wrong_f >= REVIEW_LIFT * expected {
            Tier::Review
        } else {
            Tier::Log
        }
    }

    /// What the flag lacks for the next tier, in words.
    #[must_use]
    pub fn lacks(&self) -> String {
        match self.tier {
            Tier::Act => String::new(),
            Tier::Review => format!("acts at {ACT_MIN_RAISED} raises with 4 in 5 wrong"),
            Tier::Log if self.raised < REVIEW_MIN_RAISED => {
                format!("{} more raises", REVIEW_MIN_RAISED - self.raised)
            }
            Tier::Log => format!(
                "wrong on {:.0} of its raises",
                (REVIEW_LIFT * self.expected - 1e-9).ceil().max(2.0)
            ),
        }
    }
}

/// What each [`Flag`] has earned; see [`tiers`].
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Tiers {
    /// Attempts counted
    pub known: u32,
    /// Of those, the ones whose work was wrong
    pub wrong: u32,
    /// Each flag's record, in [`Flag::ALL`]'s order
    pub signs: Vec<SignRecord>,
}

impl Tiers {
    /// The tier of `flag`.
    #[must_use]
    pub fn of(&self, flag: Flag) -> Tier {
        self.signs
            .iter()
            .find(|sign| sign.flag == flag)
            .map_or(Tier::Log, |sign| sign.tier)
    }

    /// Those of `flags` in `tier`.
    #[must_use]
    pub fn in_tier(&self, flags: &[Flag], tier: Tier) -> Vec<Flag> {
        flags
            .iter()
            .copied()
            .filter(|flag| self.of(*flag) == tier)
            .collect()
    }
}

/// How the reviews one model was asked one way held up against what
/// happened.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReviewScore {
    /// How the judge was asked
    pub variant: String,
    /// The model that reviewed
    pub judge: String,
    /// Whether `aura gate replay` asked
    pub replayed: bool,
    /// Attempts reviewed
    pub reviewed: u32,
    /// Of those, the ones whose work was later shown right or wrong
    pub known: u32,
    /// Of the known, the ones whose work was wrong
    pub wrong: u32,
    /// Known attempts the judge found unfinished
    pub unfinished: u32,
    /// Of those, the ones whose work was wrong: failures it caught
    pub caught: u32,
    /// Of those, the ones whose work was right: good work it would send
    /// back
    pub false_alarms: u32,
    /// List-price cost of the reviews in USD
    pub cost_usd: f64,
}

/// What the log says of Gate's verdicts and of each [`Flag`].
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Report {
    /// Attempts in the log, counted once each
    pub attempts: u32,
    /// Attempts Gate chose the model of
    pub routed: u32,
    /// How Gate's verdicts held up, over every attempt
    pub verdicts: Verdicts,
    /// The same per model, over the attempts Gate let stand
    pub models: BTreeMap<String, Verdicts>,
    /// Attempts Gate let stand whose work was later shown right or wrong:
    /// what the flags are scored on
    pub stood: u32,
    /// Of those, the ones whose work was wrong
    pub stood_wrong: u32,
    /// Each flag's score, in [`Flag::ALL`]'s order
    pub flags: Vec<FlagScore>,
    /// The attempts the flags are scored on, oldest first
    pub scored: Vec<Scored>,
    /// What each flag has earned, over the whole log whatever the filter
    pub tiers: Tiers,
    /// How the reviews held up, per way of asking
    pub reviews: Vec<ReviewScore>,
}

impl Report {
    /// The failed attempts Gate let stand, oldest first.
    pub fn missed(&self) -> impl Iterator<Item = &Scored> {
        self.scored.iter().filter(|scored| !scored.right)
    }
}

/// Which attempts a report is about.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filter {
    /// Only attempts Gate chose the model of
    pub routed: bool,
    /// Only attempts Gate counted as passed
    pub passed: bool,
    /// Only attempts at a benchmark task whose id contains this
    pub task: Option<String>,
}

impl Filter {
    fn takes(&self, attempt: &Attempt, truth: Option<&Truth>) -> bool {
        let task = self.task.as_deref().is_none_or(|wanted| {
            truth
                .and_then(|truth| truth.task.as_deref())
                .is_some_and(|task| task.contains(wanted))
        });
        task && (attempt.routed || !self.routed) && (attempt.success == Some(true) || !self.passed)
    }
}

/// Score Gate's verdicts and each flag on every attempt in `lines`; see
/// [`evaluate_where`].
#[must_use]
pub fn evaluate(lines: &[Line]) -> Report {
    evaluate_where(lines, &Filter::default())
}

/// The lines of a log joined: each attempt once, the truth that counts
/// for each, and the reviews.
struct Joined<'a> {
    attempts: Vec<&'a Attempt>,
    truths: HashMap<(&'a str, &'a str), &'a Truth>,
    reviews: Vec<&'a Review>,
}

impl<'a> Joined<'a> {
    /// An attempt logged more than once counts once: as it was last
    /// logged while the app ran (a follow-up that changes what Gate makes
    /// of an attempt logs it again), else as it was last rebuilt. Of
    /// several truths about one attempt, a task's own check outranks what
    /// the user did, and the later of equals counts.
    fn of(lines: &'a [Line]) -> Self {
        let mut attempts: Vec<&Attempt> = Vec::new();
        let mut at: HashMap<(&str, &str), usize> = HashMap::new();
        let mut truths: HashMap<(&str, &str), &Truth> = HashMap::new();
        let mut reviews = Vec::new();
        for line in lines {
            match line {
                Line::Attempt(attempt) => {
                    let key = (attempt.agent.as_str(), attempt.prompt_tx.as_str());
                    match at.get(&key) {
                        Some(&i) if !attempt.backfilled || attempts[i].backfilled => {
                            attempts[i] = attempt;
                        }
                        Some(_) => {}
                        None => {
                            at.insert(key, attempts.len());
                            attempts.push(attempt);
                        }
                    }
                }
                Line::Truth(truth) => {
                    let key = (truth.agent.as_str(), truth.prompt_tx.as_str());
                    let outranked = truths
                        .get(&key)
                        .is_some_and(|held| held.source.rank() > truth.source.rank());
                    if !outranked {
                        truths.insert(key, truth);
                    }
                }
                Line::Review(review) => reviews.push(review),
            }
        }
        Self {
            attempts,
            truths,
            reviews,
        }
    }

    fn truth(&self, attempt: &Attempt) -> Option<&'a Truth> {
        self.truths
            .get(&(attempt.agent.as_str(), attempt.prompt_tx.as_str()))
            .copied()
    }
}

/// Whether `model` is below the strongest tier, where Gate's follow-up
/// applies: work there has a stronger model to go on to.
#[must_use]
pub fn below_strongest(model: &str) -> bool {
    infer_tier(model) < ModelTier::Frontier
}

/// An attempt Gate let stand whose work was later shown right or wrong.
#[derive(Debug, Clone, Copy)]
pub struct Stood<'a> {
    /// The attempt, as [`evaluate_where`] counts it
    pub attempt: &'a Attempt,
    /// What showed its work right or wrong
    pub truth: &'a Truth,
    /// Attempts the log holds for the attempt's agent
    pub attempts_by_agent: usize,
}

/// The attempts in `lines` that Gate let stand and whose work was later
/// shown right or wrong, oldest first, each counted once as
/// [`evaluate_where`] counts it.
#[must_use]
pub fn stood(lines: &[Line]) -> Vec<Stood<'_>> {
    let joined = Joined::of(lines);
    let mut by_agent: HashMap<&str, usize> = HashMap::new();
    for attempt in &joined.attempts {
        *by_agent.entry(attempt.agent.as_str()).or_default() += 1;
    }
    joined
        .attempts
        .iter()
        .filter(|attempt| attempt.success != Some(false))
        .filter_map(|attempt| {
            Some(Stood {
                attempt,
                truth: joined.truth(attempt)?,
                attempts_by_agent: by_agent[attempt.agent.as_str()],
            })
        })
        .collect()
}

/// What each [`Flag`] has earned on the attempts in `lines` that Gate
/// routed to a model below the strongest tier and whose work is known
/// right or wrong: those it let stand that something later proved, and
/// those a review it acted on found unfinished, which count as wrong
/// with the signs they showed then.
///
/// A flag calls for a review ([`Tier::Review`]) once it was raised
/// [`REVIEW_MIN_RAISED`] times and the attempts it was raised on were
/// wrong [`REVIEW_LIFT`] times as often as attempts of the same
/// difficulty (hard, or not) are. It acts on its own ([`Tier::Act`]) once
/// it was raised [`ACT_MIN_RAISED`] times and [`ACT_PRECISION`] of those
/// attempts were wrong.
#[must_use]
pub fn tiers(lines: &[Line]) -> Tiers {
    let joined = Joined::of(lines);
    // The first review Gate acted on that found each attempt unfinished.
    let mut sent_back: HashMap<(&str, &str), &Review> = HashMap::new();
    for review in &joined.reviews {
        if review.acted && !review.replayed && review.verdict == Answer::Model {
            sent_back
                .entry((review.agent.as_str(), review.prompt_tx.as_str()))
                .or_insert(review);
        }
    }
    // Each counted attempt: whether it is hard, whether it was wrong, and
    // the flags it raised.
    let mut counted: Vec<(bool, bool, Vec<Flag>)> = Vec::new();
    for attempt in &joined.attempts {
        if !attempt.routed || !below_strongest(&attempt.model) {
            continue;
        }
        let hard = attempt.difficulty == Some(Difficulty::Hard);
        let key = (attempt.agent.as_str(), attempt.prompt_tx.as_str());
        if let Some(review) = sent_back.get(&key) {
            counted.push((hard, true, review.signs.clone()));
        } else if let Some(truth) = joined.truth(attempt) {
            if attempt.success != Some(false) {
                let raised = flags(&attempt.evidence, attempt.category);
                counted.push((hard, !truth.passed, raised));
            }
        }
    }
    let rate = |of_hard: bool| {
        let (known, wrong) = counted
            .iter()
            .filter(|(hard, ..)| *hard == of_hard)
            .fold((0u32, 0u32), |(known, wrong), (_, was_wrong, _)| {
                (known + 1, wrong + u32::from(*was_wrong))
            });
        if known == 0 {
            0.0
        } else {
            f64::from(wrong) / f64::from(known)
        }
    };
    let rates = [rate(false), rate(true)];
    let signs = Flag::ALL
        .into_iter()
        .map(|flag| {
            let (mut raised, mut wrong, mut expected) = (0, 0, 0.0);
            for (hard, was_wrong, flags) in &counted {
                if flags.contains(&flag) {
                    raised += 1;
                    wrong += u32::from(*was_wrong);
                    expected += rates[usize::from(*hard)];
                }
            }
            SignRecord {
                flag,
                raised,
                wrong,
                expected,
                tier: SignRecord::tier_of(raised, wrong, expected),
            }
        })
        .collect();
    Tiers {
        known: u32::try_from(counted.len()).unwrap_or(u32::MAX),
        wrong: u32::try_from(counted.iter().filter(|(_, wrong, _)| *wrong).count())
            .unwrap_or(u32::MAX),
        signs,
    }
}

/// Score Gate's verdicts, each flag and the reviews on the attempts in
/// `lines` that `filter` takes. An attempt logged more than once counts
/// once: as it was last logged while the app ran (a follow-up that
/// changes what Gate makes of an attempt logs it again), else as it was
/// last rebuilt. Of several truths about one attempt, a task's own check
/// outranks what the user did, and the later of equals counts. Of
/// several reviews of one attempt asked the same way, the last counts.
#[must_use]
pub fn evaluate_where(lines: &[Line], filter: &Filter) -> Report {
    let mut joined = Joined::of(lines);
    let truths = std::mem::take(&mut joined.truths);
    let truth_of = |attempt: &Attempt| {
        truths
            .get(&(attempt.agent.as_str(), attempt.prompt_tx.as_str()))
            .copied()
    };
    joined
        .attempts
        .retain(|attempt| filter.takes(attempt, truth_of(attempt)));
    let mut report = Report {
        attempts: u32::try_from(joined.attempts.len()).unwrap_or(u32::MAX),
        flags: Flag::ALL
            .into_iter()
            .map(|flag| FlagScore {
                flag,
                raised: 0,
                caught: 0,
                false_alarms: 0,
            })
            .collect(),
        tiers: tiers(lines),
        ..Report::default()
    };
    let taken: HashMap<(&str, &str), Option<bool>> = joined
        .attempts
        .iter()
        .map(|attempt| {
            (
                (attempt.agent.as_str(), attempt.prompt_tx.as_str()),
                truth_of(attempt).map(|truth| truth.passed),
            )
        })
        .collect();
    for attempt in &joined.attempts {
        report.routed += u32::from(attempt.routed);
        let Some(truth) = truth_of(attempt) else {
            continue;
        };
        report.verdicts.take(attempt.success, truth.passed);
        if attempt.success == Some(false) {
            continue;
        }
        report
            .models
            .entry(attempt.model.clone())
            .or_default()
            .take(attempt.success, truth.passed);
        report.stood += 1;
        report.stood_wrong += u32::from(!truth.passed);
        let raised = flags(&attempt.evidence, attempt.category);
        for score in &mut report.flags {
            if raised.contains(&score.flag) {
                score.raised += 1;
                score.caught += u32::from(!truth.passed);
                score.false_alarms += u32::from(truth.passed);
            }
        }
        let run = [truth.compare.as_deref(), truth.run.as_deref()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" ");
        report.scored.push(Scored {
            run,
            task: truth.task.clone().unwrap_or_default(),
            model: attempt.model.clone(),
            routed: attempt.routed,
            signal: attempt.signal,
            right: truth.passed,
            source: truth.source,
            flags: raised,
        });
    }

    // The last review of each attempt per way of asking and reviewer.
    let mut last: BTreeMap<(&str, &str, bool, &str, &str), &Review> = BTreeMap::new();
    for review in &joined.reviews {
        last.insert(
            (
                review.variant.as_str(),
                review.judge.as_str(),
                review.replayed,
                review.agent.as_str(),
                review.prompt_tx.as_str(),
            ),
            review,
        );
    }
    for ((variant, judge, replayed, agent, prompt), review) in last {
        let Some(right) = taken.get(&(agent, prompt)) else {
            continue;
        };
        if report.reviews.last().is_none_or(|score| {
            score.variant != variant || score.judge != judge || score.replayed != replayed
        }) {
            report.reviews.push(ReviewScore {
                variant: variant.to_string(),
                judge: judge.to_string(),
                replayed,
                reviewed: 0,
                known: 0,
                wrong: 0,
                unfinished: 0,
                caught: 0,
                false_alarms: 0,
                cost_usd: 0.0,
            });
        }
        let Some(score) = report.reviews.last_mut() else {
            continue;
        };
        score.reviewed += 1;
        score.cost_usd += review.cost_usd;
        if let Some(right) = right {
            let unfinished = review.verdict == Answer::Model;
            score.known += 1;
            score.wrong += u32::from(!right);
            score.unfinished += u32::from(unfinished);
            score.caught += u32::from(unfinished && !right);
            score.false_alarms += u32::from(unfinished && *right);
        }
    }
    report
}

fn share(part: u32, of: u32) -> String {
    if of == 0 {
        "-".to_string()
    } else {
        format!("{:.0}%", 100.0 * f64::from(part) / f64::from(of))
    }
}

impl Report {
    /// The report as text, for a terminal.
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = String::new();
        let v = self.verdicts;
        let _ = writeln!(
            out,
            "{} attempts logged ({} routed by Gate), {} later shown right or wrong: {} right, {} wrong",
            self.attempts,
            self.routed,
            v.known,
            v.right,
            v.known - v.right
        );
        if v.known == 0 {
            out.push_str(
                "Nothing to score yet: run /compare on benchmark tasks, whose own checks say whether each run's work was right.\n",
            );
            return out;
        }
        let _ = writeln!(
            out,
            "Gate let stand {} of them: {} right, {} wrong ({} it counted as passed, {} it had nothing to judge by)",
            self.stood, self.stood - self.stood_wrong, self.stood_wrong, v.false_passes, v.unjudged_failures
        );
        let _ = writeln!(
            out,
            "Gate counted {} as failed whose work was right",
            v.false_failures
        );

        let _ = writeln!(out, "\nPer model, attempts Gate let stand:");
        let width = self
            .models
            .keys()
            .map(String::len)
            .max()
            .unwrap_or(5)
            .max(5);
        let _ = writeln!(out, "  {:width$}  stood  wrong  share wrong", "model");
        for (model, m) in &self.models {
            let wrong = m.known - m.right;
            let _ = writeln!(
                out,
                "  {model:width$}  {:>5}  {:>5}  {:>11}",
                m.known,
                wrong,
                share(wrong, m.known)
            );
        }

        let _ = writeln!(
            out,
            "\nSigns of failure, scored on the {} attempts Gate let stand ({} wrong, {} right):",
            self.stood,
            self.stood_wrong,
            self.stood - self.stood_wrong
        );
        let _ = writeln!(
            out,
            "  {:26}  raised  caught  of wrong  false alarms  of right  precision",
            "sign"
        );
        for score in &self.flags {
            let _ = writeln!(
                out,
                "  {:26}  {:>6}  {:>6}  {:>8}  {:>12}  {:>8}  {:>9}",
                score.flag.name(),
                score.raised,
                score.caught,
                share(score.caught, self.stood_wrong),
                score.false_alarms,
                share(score.false_alarms, self.stood - self.stood_wrong),
                share(score.caught, score.raised),
            );
        }

        self.render_tiers(&mut out);
        self.render_reviews(&mut out);

        if self.stood_wrong > 0 {
            let _ = writeln!(out, "\nFailed work Gate let stand:");
            for missed in self.missed() {
                let names: Vec<&str> = missed.flags.iter().map(|flag| flag.name()).collect();
                let flags = if names.is_empty() {
                    "no sign raised".to_string()
                } else {
                    names.join(", ")
                };
                let what = [missed.run.as_str(), missed.task.as_str()]
                    .into_iter()
                    .filter(|part| !part.is_empty())
                    .collect::<Vec<_>>()
                    .join(" · ");
                let _ = writeln!(
                    out,
                    "  {what} · {} · gate: {} · {flags}",
                    missed.model, missed.signal
                );
            }
        }
        out
    }

    fn render_tiers(&self, out: &mut String) {
        let tiers = &self.tiers;
        let _ = writeln!(
            out,
            "\nWhat each sign has earned, on the {} routed attempts below the strongest tier whose work is known ({} wrong):",
            tiers.known, tiers.wrong
        );
        let _ = writeln!(
            out,
            "  {:26}  raised  wrong  by chance  tier      next",
            "sign"
        );
        for sign in &tiers.signs {
            let _ = writeln!(
                out,
                "  {:26}  {:>6}  {:>5}  {:>9.1}  {:8}  {}",
                sign.flag.name(),
                sign.raised,
                sign.wrong,
                sign.expected,
                sign.tier.name(),
                sign.lacks()
            );
        }
        let _ = writeln!(
            out,
            "  review: raised {REVIEW_MIN_RAISED} times or more, and wrong {REVIEW_LIFT} times as often as attempts of the same difficulty; acts: raised {ACT_MIN_RAISED} times or more, and wrong 4 times in 5"
        );
    }

    fn render_reviews(&self, out: &mut String) {
        if self.reviews.is_empty() {
            return;
        }
        let _ = writeln!(out, "\nReviews of work that was about to stand:");
        let _ = writeln!(
            out,
            "  {:25}  {:20}  reviewed  known  wrong  said unfinished  caught  right but flagged  wrong but passed     cost",
            "asked", "reviewer"
        );
        for score in &self.reviews {
            let asked = if score.replayed {
                format!("{} (replayed)", score.variant)
            } else {
                score.variant.clone()
            };
            let reviewer = score.judge.rsplit('/').next().unwrap_or_default();
            let _ = writeln!(
                out,
                "  {asked:25}  {reviewer:20}  {:>8}  {:>5}  {:>5}  {:>15}  {:>6}  {:>17}  {:>16}  ${:>6.2}",
                score.reviewed,
                score.known,
                score.wrong,
                score.unfinished,
                score.caught,
                score.false_alarms,
                score.wrong - score.caught,
                score.cost_usd
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aura_gate_evidence::CheckEvidence;

    fn evidence(command: &str, output: &str, reply: &str) -> Evidence {
        Evidence {
            calls: 3,
            commands: 1,
            checks_run: 1,
            checks: vec![CheckEvidence {
                command: command.to_string(),
                passed: true,
                output: output.to_string(),
            }],
            files_changed: Some(1),
            reply: reply.to_string(),
            ..Evidence::default()
        }
    }

    fn attempt(prompt: &str, signal: OutcomeSignal, evidence: Evidence) -> Attempt {
        Attempt {
            at_ms: 1,
            agent: "aa".to_string(),
            prompt_tx: prompt.to_string(),
            category: PromptCategory::MultiFileChange,
            difficulty: Some(Difficulty::Hard),
            led: Some(false),
            routed: true,
            model: "claude/claude-sonnet-5-5".to_string(),
            signal,
            success: signal.success(),
            cost_usd: 0.1,
            subtask_cost_usd: 0.0,
            latency_ms: 48_000,
            evidence,
            backfilled: false,
        }
    }

    fn truth(prompt: &str, source: TruthSource, passed: bool) -> Truth {
        Truth {
            at_ms: 2,
            agent: "aa".to_string(),
            prompt_tx: prompt.to_string(),
            source,
            passed,
            compare: Some("Compare #22".to_string()),
            run: Some("a-4".to_string()),
            task: Some("ansible-d33bedc4".to_string()),
            copy: None,
        }
    }

    fn review(prompt: &str, verdict: Answer, variant: &str) -> Review {
        Review {
            at_ms: 3,
            agent: "aa".to_string(),
            prompt_tx: prompt.to_string(),
            why: vec!["hard".to_string()],
            signs: vec![Flag::ShellEdit],
            verdict,
            findings: "The `int` case accepts `'1.1'`.".to_string(),
            judge: "claude/claude-opus-5-5".to_string(),
            cost_usd: 0.05,
            variant: variant.to_string(),
            criteria: Vec::new(),
            acted: false,
            replayed: true,
        }
    }

    fn clean() -> Evidence {
        evidence("pytest -q", "9 passed", "Done; the tests pass.")
    }

    fn masked() -> Evidence {
        evidence(
            "pytest -q 2>&1 | grep -E 'passed|failed' | head -20",
            "TypeError: boom\n9 passed",
            "I rewrote it, but it isn't verified.",
        )
    }

    #[test]
    fn lines_round_trip_through_the_file() {
        let dir = std::env::temp_dir().join(format!("aura-attempt-log-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let log = AttemptLog::in_data_dir(&dir);
        assert_eq!(log.read().unwrap(), Read::default(), "no file yet");
        let kept = Truth {
            copy: Some(CopyAt {
                root: PathBuf::from("/home/me/.aura/compare/c1/a-4"),
                git_dir: Some(PathBuf::from("/home/me/.aura/compare/c1/a-4.git")),
                baseline: "4b825dc6".to_string(),
            }),
            ..truth("p1", TruthSource::TaskCheck, false)
        };
        let lines = [
            Line::Attempt(attempt("p1", OutcomeSignal::ChecksPassed, masked())),
            Line::Truth(kept),
            Line::Review(review("p1", Answer::Model, "plain")),
        ];
        for line in &lines {
            log.append(line).unwrap();
        }
        assert_eq!(lines[2].key(), ("aa", "p1", Kind::Review));
        fs::OpenOptions::new()
            .append(true)
            .open(log.path())
            .unwrap()
            .write_all(b"not json\n\n")
            .unwrap();
        let read = log.read().unwrap();
        assert_eq!(read.lines, lines);
        assert_eq!(read.unreadable, 1);
        let text = fs::read_to_string(log.path()).unwrap();
        assert!(
            text.starts_with(r#"{"kind":"attempt","at_ms":1,"agent":"aa","prompt_tx":"p1""#),
            "{text}"
        );
        assert!(text.contains(r#"{"kind":"truth","#), "{text}");
        assert!(
            text.contains(r#""copy":{"root":"/home/me/.aura/compare/c1/a-4","#),
            "{text}"
        );
        assert!(
            text.contains(r#"{"kind":"review","at_ms":3,"agent":"aa","prompt_tx":"p1","why":["hard"],"signs":["shell_edit"],"verdict":"model","#),
            "{text}"
        );
        assert!(log.path().ends_with("gate/attempts.jsonl"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_report_scores_gate_and_each_flag_against_what_happened() {
        let lines = [
            // Let stand, wrong, and flagged.
            Line::Attempt(attempt("p1", OutcomeSignal::ChecksPassed, masked())),
            Line::Truth(truth("p1", TruthSource::TaskCheck, false)),
            // Let stand and right: clean, then one that pipes its check.
            Line::Attempt(attempt("p2", OutcomeSignal::ChecksPassed, clean())),
            Line::Truth(truth("p2", TruthSource::TaskCheck, true)),
            Line::Attempt(attempt(
                "p3",
                OutcomeSignal::ChecksPassed,
                evidence("pytest -q | tail -5", "9 passed", "Done."),
            )),
            Line::Truth(truth("p3", TruthSource::TaskCheck, true)),
            // Nothing to judge by, wrong.
            Line::Attempt(attempt(
                "p4",
                OutcomeSignal::Unavailable,
                Evidence::default(),
            )),
            Line::Truth(truth("p4", TruthSource::Rewind, false)),
            // Counted as failed (and moved up): not what flags are for.
            Line::Attempt(attempt("p5", OutcomeSignal::ChecksFailed, clean())),
            Line::Truth(truth("p5", TruthSource::TaskCheck, true)),
            // No truth: not scored.
            Line::Attempt(attempt("p6", OutcomeSignal::ChecksPassed, masked())),
        ];
        let report = evaluate(&lines);
        assert_eq!((report.attempts, report.routed), (6, 6));
        assert_eq!(
            report.verdicts,
            Verdicts {
                known: 5,
                right: 3,
                false_passes: 1,
                unjudged_failures: 1,
                false_failures: 1,
            }
        );
        assert_eq!((report.stood, report.stood_wrong), (4, 2));
        let score = |flag: Flag| {
            let s = report.flags.iter().find(|s| s.flag == flag).unwrap();
            (s.raised, s.caught, s.false_alarms)
        };
        assert_eq!(score(Flag::MaskedCheck), (2, 1, 1));
        assert_eq!(score(Flag::MaskedCheckWithFailures), (1, 1, 0));
        assert_eq!(score(Flag::ReplyUnverified), (1, 1, 0));
        assert_eq!(score(Flag::NoCheck), (1, 1, 0));
        assert_eq!(score(Flag::HitLimit), (0, 0, 0));
        assert_eq!(report.models["claude/claude-sonnet-5-5"].known, 4);
        assert_eq!(report.scored.len(), 4);
        let missed: Vec<&Scored> = report.missed().collect();
        assert_eq!(missed.len(), 2);
        assert_eq!(missed[0].run, "Compare #22 a-4");
        assert_eq!(
            missed[0].flags,
            [
                Flag::MaskedCheck,
                Flag::CheckOutputFailures,
                Flag::MaskedCheckWithFailures,
                Flag::ReplyUnverified
            ]
        );

        let text = report.render();
        assert!(text.contains("6 attempts logged (6 routed by Gate), 5 later shown right or wrong: 3 right, 2 wrong"), "{text}");
        assert!(text.contains("Gate let stand 4 of them: 2 right, 2 wrong (1 it counted as passed, 1 it had nothing to judge by)"), "{text}");
        assert!(text.contains("masked_check_with_failures"), "{text}");
        assert!(text.contains("Compare #22 a-4 · ansible-d33bedc4 · claude/claude-sonnet-5-5 · gate: checks_passed · masked_check"), "{text}");
    }

    #[test]
    fn an_attempt_logged_twice_counts_once_and_a_task_check_outranks_the_user() {
        let mut rebuilt = attempt("p1", OutcomeSignal::ChecksPassed, clean());
        rebuilt.backfilled = true;
        let lines = [
            Line::Attempt(rebuilt.clone()),
            Line::Attempt(attempt("p1", OutcomeSignal::Unavailable, clean())),
            Line::Attempt(attempt("p1", OutcomeSignal::ChecksPassed, masked())),
            Line::Attempt(rebuilt),
            Line::Truth(truth("p1", TruthSource::TaskCheck, false)),
            Line::Truth(truth("p1", TruthSource::Picked, true)),
        ];
        let report = evaluate(&lines);
        assert_eq!(report.attempts, 1);
        assert_eq!(
            (
                report.verdicts.false_passes,
                report.verdicts.unjudged_failures
            ),
            (1, 0),
            "the task's check counts, against what Gate last made of the attempt"
        );
        assert_eq!(
            report.missed().next().unwrap().flags.first(),
            Some(&Flag::MaskedCheck),
            "the last line written while the app ran counts"
        );
    }

    #[test]
    fn a_report_can_be_about_some_attempts_only() {
        let mut pinned = attempt("p2", OutcomeSignal::ChecksPassed, clean());
        pinned.routed = false;
        let mut other = truth("p2", TruthSource::TaskCheck, true);
        other.task = Some("coding/rename-symbol".to_string());
        let lines = [
            Line::Attempt(attempt("p1", OutcomeSignal::ChecksPassed, masked())),
            Line::Truth(truth("p1", TruthSource::TaskCheck, false)),
            Line::Attempt(pinned),
            Line::Truth(other),
            Line::Attempt(attempt("p3", OutcomeSignal::ChecksPassed, clean())),
        ];
        let all = evaluate(&lines);
        assert_eq!((all.attempts, all.stood, all.stood_wrong), (3, 2, 1));
        let routed = evaluate_where(
            &lines,
            &Filter {
                routed: true,
                ..Filter::default()
            },
        );
        assert_eq!(
            (routed.attempts, routed.stood, routed.stood_wrong),
            (2, 1, 1)
        );
        let task = evaluate_where(
            &lines,
            &Filter {
                task: Some("coding/".to_string()),
                ..Filter::default()
            },
        );
        assert_eq!((task.attempts, task.stood, task.stood_wrong), (1, 1, 0));
        let mut unjudged = lines.to_vec();
        unjudged.push(Line::Attempt(attempt(
            "p4",
            OutcomeSignal::Unavailable,
            Evidence::default(),
        )));
        unjudged.push(Line::Truth(truth("p4", TruthSource::Rewind, false)));
        let passed = Filter {
            passed: true,
            ..Filter::default()
        };
        assert_eq!(evaluate(&unjudged).stood, 3);
        assert_eq!(evaluate_where(&unjudged, &passed).stood, 2);
    }

    fn stale() -> Evidence {
        Evidence {
            changed_after_check: true,
            ..clean()
        }
    }

    /// `count` attempts named `{name}{n}` with `evidence`, the first
    /// `wrong` of them failing the task's check.
    fn proven(name: &str, count: usize, wrong: usize, evidence: &Evidence) -> Vec<Line> {
        (0..count)
            .flat_map(|n| {
                let prompt = format!("{name}{n}");
                [
                    Line::Attempt(attempt(
                        &prompt,
                        OutcomeSignal::ChecksPassed,
                        evidence.clone(),
                    )),
                    Line::Truth(truth(&prompt, TruthSource::TaskCheck, n >= wrong)),
                ]
            })
            .collect()
    }

    #[test]
    fn a_sign_earns_a_review_by_being_wrong_more_often_than_its_attempts_are() {
        // Ten hard attempts, three wrong, all three among the five whose
        // check is stale: twice what chance gives those five.
        let mut lines = proven("s", 5, 3, &stale());
        lines.extend(proven("c", 5, 0, &clean()));
        let found = tiers(&lines);
        assert_eq!((found.known, found.wrong), (10, 3));
        let sign = found
            .signs
            .iter()
            .find(|sign| sign.flag == Flag::StaleCheck)
            .unwrap();
        assert_eq!((sign.raised, sign.wrong), (5, 3));
        assert!((sign.expected - 1.5).abs() < 1e-9, "{}", sign.expected);
        assert_eq!(sign.tier, Tier::Review);
        assert_eq!(found.of(Flag::StaleCheck), Tier::Review);
        assert_eq!(found.of(Flag::NoCheck), Tier::Log);
        assert_eq!(
            found.in_tier(&[Flag::NoCheck, Flag::StaleCheck], Tier::Review),
            [Flag::StaleCheck]
        );

        // One wrong attempt more among the clean ones and the sign is no
        // better than chance.
        let mut diluted = proven("s", 5, 3, &stale());
        diluted.extend(proven("c", 5, 2, &clean()));
        assert_eq!(tiers(&diluted).of(Flag::StaleCheck), Tier::Log);

        // Too few raises earn nothing, however wrong.
        let mut few = proven("s", 4, 4, &stale());
        few.extend(proven("c", 20, 0, &clean()));
        let found = tiers(&few);
        assert_eq!(found.of(Flag::StaleCheck), Tier::Log);
        assert_eq!(
            found.signs[Flag::ALL
                .iter()
                .position(|flag| *flag == Flag::StaleCheck)
                .unwrap()]
            .lacks(),
            "1 more raises"
        );
    }

    #[test]
    fn a_sign_acts_once_most_of_what_it_is_raised_on_is_wrong() {
        let mut lines = proven("m", 10, 8, &masked());
        lines.extend(proven("c", 10, 0, &clean()));
        let found = tiers(&lines);
        assert_eq!(found.of(Flag::MaskedCheckWithFailures), Tier::Act);
        assert_eq!(found.of(Flag::ReplyUnverified), Tier::Act);
        let mut fewer = proven("m", 10, 7, &masked());
        fewer.extend(proven("c", 10, 0, &clean()));
        assert_eq!(
            tiers(&fewer).of(Flag::MaskedCheckWithFailures),
            Tier::Review
        );
        let rendered = evaluate(&lines).render();
        assert!(
            rendered.contains(
                "What each sign has earned, on the 20 routed attempts below the strongest tier whose work is known (8 wrong):"
            ),
            "{rendered}"
        );
        assert!(
            rendered.contains("masked_check_with_failures      10      8        4.0  acts"),
            "{rendered}"
        );
    }

    #[test]
    fn tiers_count_routed_work_below_the_strongest_tier_and_what_reviews_sent_back() {
        let mut lines = proven("s", 5, 3, &stale());
        lines.extend(proven("c", 5, 0, &clean()));
        // Work on the strongest model, work Gate did not route and work
        // nothing proved say nothing of the signs.
        let opus = Attempt {
            model: "claude/claude-opus-5-5".to_string(),
            ..attempt("o1", OutcomeSignal::ChecksPassed, stale())
        };
        let pinned = Attempt {
            routed: false,
            ..attempt("u1", OutcomeSignal::ChecksPassed, stale())
        };
        lines.extend([
            Line::Attempt(opus),
            Line::Truth(truth("o1", TruthSource::TaskCheck, true)),
            Line::Attempt(pinned),
            Line::Truth(truth("u1", TruthSource::TaskCheck, true)),
            Line::Attempt(attempt("n1", OutcomeSignal::ChecksPassed, stale())),
        ]);
        let found = tiers(&lines);
        assert_eq!((found.known, found.wrong), (10, 3));

        // An attempt a review sent back counts as wrong with the signs it
        // showed then, whatever became of it; a replayed review or one
        // only logged changes nothing.
        let sent_back = Review {
            acted: true,
            replayed: false,
            signs: vec![Flag::StaleCheck],
            ..review("n1", Answer::Model, "plain")
        };
        lines.push(Line::Review(sent_back));
        lines.push(Line::Review(review("c0", Answer::Model, "plain")));
        let found = tiers(&lines);
        assert_eq!((found.known, found.wrong), (11, 4));
        let sign = found
            .signs
            .iter()
            .find(|sign| sign.flag == Flag::StaleCheck)
            .unwrap();
        assert_eq!((sign.raised, sign.wrong), (6, 4));
    }

    #[test]
    fn reviews_are_scored_per_way_of_asking_against_what_happened() {
        let lines = [
            Line::Attempt(attempt("p1", OutcomeSignal::ChecksPassed, clean())),
            Line::Truth(truth("p1", TruthSource::TaskCheck, false)),
            Line::Attempt(attempt("p2", OutcomeSignal::ChecksPassed, clean())),
            Line::Truth(truth("p2", TruthSource::TaskCheck, true)),
            Line::Attempt(attempt("p3", OutcomeSignal::ChecksPassed, clean())),
            Line::Review(review("p1", Answer::Model, "plain")),
            Line::Review(review("p2", Answer::Ok, "plain")),
            Line::Review(review("p3", Answer::Model, "plain")),
            // Asked twice the same way, the last answer counts.
            Line::Review(review("p1", Answer::Ok, "thinking")),
            Line::Review(review("p1", Answer::Model, "thinking")),
            Line::Review(review("p2", Answer::Model, "thinking")),
            // A review of an attempt the log does not hold is left out.
            Line::Review(review("gone", Answer::Model, "plain")),
        ];
        let report = evaluate(&lines);
        let score = |variant: &str| {
            report
                .reviews
                .iter()
                .find(|score| score.variant == variant)
                .unwrap()
        };
        assert_eq!(report.reviews.len(), 2);
        let plain = score("plain");
        assert!(plain.replayed);
        assert_eq!(
            (plain.reviewed, plain.known, plain.wrong),
            (3, 2, 1),
            "{plain:?}"
        );
        assert_eq!(
            (plain.unfinished, plain.caught, plain.false_alarms),
            (1, 1, 0)
        );
        assert!((plain.cost_usd - 0.15).abs() < 1e-9);
        let thinking = score("thinking");
        assert_eq!((thinking.reviewed, thinking.known), (2, 2));
        assert_eq!(
            (thinking.unfinished, thinking.caught, thinking.false_alarms),
            (2, 1, 1)
        );
        let rendered = report.render();
        assert!(
            rendered.contains("Reviews of work that was about to stand:"),
            "{rendered}"
        );
        assert!(rendered.contains("plain (replayed)"), "{rendered}");

        // Another reviewer asked the same way is scored on its own.
        let mut lines = lines.to_vec();
        lines.push(Line::Review(Review {
            judge: "claude/claude-sonnet-5-5".to_string(),
            ..review("p1", Answer::Ok, "plain")
        }));
        let both = evaluate(&lines);
        let by = |judge: &str| {
            both.reviews
                .iter()
                .find(|score| score.variant == "plain" && score.judge == judge)
                .unwrap()
        };
        assert_eq!(both.reviews.len(), 3);
        assert_eq!(by("claude/claude-opus-5-5"), plain);
        let sonnet = by("claude/claude-sonnet-5-5");
        assert_eq!((sonnet.reviewed, sonnet.wrong, sonnet.caught), (1, 1, 0));
        let rendered = both.render();
        assert!(rendered.contains("claude-sonnet-5-5"), "{rendered}");

        // A report about some attempts scores the reviews of those only.
        let failed = evaluate_where(
            &lines,
            &Filter {
                task: Some("nothing".to_string()),
                ..Filter::default()
            },
        );
        assert!(failed.reviews.is_empty());
    }

    #[test]
    fn the_attempts_that_stood_are_listed_once_with_what_proved_them() {
        let failed = Attempt {
            success: Some(false),
            ..attempt("p3", OutcomeSignal::ChecksFailed, clean())
        };
        let lines = [
            Line::Attempt(attempt("p1", OutcomeSignal::ChecksPassed, clean())),
            Line::Attempt(attempt("p1", OutcomeSignal::ChecksPassed, masked())),
            Line::Truth(truth("p1", TruthSource::TaskCheck, false)),
            Line::Attempt(attempt("p2", OutcomeSignal::ChecksPassed, clean())),
            Line::Attempt(failed),
            Line::Truth(truth("p3", TruthSource::TaskCheck, false)),
        ];
        let stood = stood(&lines);
        assert_eq!(stood.len(), 1, "p2 has no truth and p3 did not stand");
        assert_eq!(stood[0].attempt.prompt_tx, "p1");
        assert_eq!(stood[0].attempt.evidence, masked(), "as last logged");
        assert!(!stood[0].truth.passed);
        assert_eq!(stood[0].attempts_by_agent, 3);
        assert!(below_strongest("claude/claude-sonnet-5-5"));
        assert!(!below_strongest("claude/claude-opus-5-5"));
    }

    #[test]
    fn an_empty_log_says_what_fills_it() {
        let text = evaluate(&[]).render();
        assert!(text.starts_with("0 attempts logged"), "{text}");
        assert!(text.contains("Nothing to score yet"), "{text}");
    }

    #[test]
    fn the_users_signals_are_truths() {
        assert_eq!(
            TruthSource::of_user(OutcomeSignal::Rewind),
            Some(TruthSource::Rewind)
        );
        assert_eq!(
            TruthSource::of_user(OutcomeSignal::RetryStronger),
            Some(TruthSource::RetryStronger)
        );
        assert_eq!(TruthSource::of_user(OutcomeSignal::ChecksPassed), None);
        assert_eq!(TruthSource::of_user(OutcomeSignal::JudgedFail), None);
    }

    #[test]
    fn older_lines_still_read() {
        let mut line = serde_json::to_value(Line::Attempt(attempt(
            "p1",
            OutcomeSignal::ChecksPassed,
            clean(),
        )))
        .unwrap();
        line["signal"] = "none".into();
        let Line::Attempt(read) = serde_json::from_value(line).unwrap() else {
            panic!("an attempt");
        };
        assert_eq!(read.signal, OutcomeSignal::Unavailable);
        let mut picked =
            serde_json::to_value(Line::Truth(truth("p1", TruthSource::Rewind, true))).unwrap();
        picked["source"] = "picked".into();
        assert!(serde_json::from_value::<Line>(picked).is_ok());
    }
}
