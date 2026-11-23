//! What `/compare` and the BENCH tab record: one [`ComparePayload`] per
//! state change of a comparison (started, finished, picked, applied,
//! dismissed), written as a [`SystemKind::Compare`] system transaction on
//! the comparison's own agent. The latest one is the comparison as it
//! stands.
//!
//! A comparison runs the same work on two arms, A and B, each either
//! Gate or a pinned model. The work is one prompt, a plan to build, or
//! the prompts of an earlier session replayed in order.
//!
//! The runs themselves are ordinary agents with ordinary prompt and
//! `Outcome` records; this payload only says which runs belong to which
//! arm, what they measured, how the judge saw them, and where their
//! workspace copies are.

use std::borrow::Cow;
use std::path::PathBuf;

use super::gate::{Difficulty, OutcomeSignal, PromptCategory};
use super::plan_usage::PlanShare;
use super::transaction::SystemKind;
use crate::ids::{AgentId, Hash};
use serde::{Deserialize, Serialize};

/// Which side of a comparison a run is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum CompareArm {
    /// The first arm; Gate in comparisons recorded before arms were
    /// chosen freely.
    #[serde(rename = "a", alias = "gate")]
    A,
    /// The second arm; the model Gate was compared with in older records.
    #[serde(rename = "b", alias = "other")]
    B,
}

impl CompareArm {
    /// Both arms, A first.
    pub const BOTH: [Self; 2] = [Self::A, Self::B];

    /// The arm's name in run names and commands: `a` or `b`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::A => "a",
            Self::B => "b",
        }
    }

    /// The other arm.
    #[must_use]
    pub const fn opposite(self) -> Self {
        match self {
            Self::A => Self::B,
            Self::B => Self::A,
        }
    }

    /// 0 for A, 1 for B.
    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Self::A => 0,
            Self::B => 1,
        }
    }
}

impl std::fmt::Display for CompareArm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What one arm runs on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ArmSpec {
    /// Gate routes every turn and subtask, with exploration off;
    /// `delegate` says whether its lead must delegate.
    Gate { delegate: bool },
    /// Every turn and subtask runs on `model`.
    Model { model: String },
    /// Every turn and subtask is classified and runs on the model given
    /// for its difficulty.
    ByDifficulty {
        /// The model easy work runs on.
        easy: String,
        /// The model medium work, and work of unknown difficulty, runs on.
        medium: String,
        /// The model hard work runs on.
        hard: String,
    },
}

impl ArmSpec {
    /// How [`ArmSpec::label`] names Gate whose lead need not delegate.
    pub const NO_DELEGATE: &'static str = "gate:nodelegate";

    /// What [`ArmSpec::label`] starts a by-difficulty arm with, as in
    /// `difficulty:easy=<id>,medium=<id>,hard=<id>`.
    pub const BY_DIFFICULTY_PREFIX: &'static str = "difficulty:";

    /// `gate`, [`Self::NO_DELEGATE`], the model, or
    /// `difficulty:easy=<id>,medium=<id>,hard=<id>`.
    #[must_use]
    pub fn label(&self) -> Cow<'_, str> {
        match self {
            Self::Gate { delegate: true } => Cow::Borrowed("gate"),
            Self::Gate { delegate: false } => Cow::Borrowed(Self::NO_DELEGATE),
            Self::Model { model } => Cow::Borrowed(model),
            Self::ByDifficulty { easy, medium, hard } => Cow::Owned(format!(
                "{}easy={easy},medium={medium},hard={hard}",
                Self::BY_DIFFICULTY_PREFIX
            )),
        }
    }

    /// The arm a label from [`Self::label`] names. A label that is
    /// neither Gate nor a well-formed by-difficulty label names a model.
    #[must_use]
    pub fn from_label(label: &str) -> Self {
        match label {
            "gate" => Self::Gate { delegate: true },
            Self::NO_DELEGATE => Self::Gate { delegate: false },
            model => match Self::parse_by_difficulty(model) {
                Some(Ok([easy, medium, hard])) => Self::ByDifficulty { easy, medium, hard },
                _ => Self::Model {
                    model: model.to_string(),
                },
            },
        }
    }

    /// The easy, medium and hard models of a by-difficulty label:
    /// `None` when `label` does not start with
    /// [`Self::BY_DIFFICULTY_PREFIX`] (in any case). The keys come in any
    /// order, as `key=value` separated by commas.
    ///
    /// # Errors
    /// Says which key is missing, given twice or unknown, or which entry
    /// is not `key=value`.
    pub fn parse_by_difficulty(label: &str) -> Option<Result<[String; 3], String>> {
        let prefix = Self::BY_DIFFICULTY_PREFIX;
        let head = label.trim_start();
        if !head
            .get(..prefix.len())
            .is_some_and(|start| start.eq_ignore_ascii_case(prefix))
        {
            return None;
        }
        let body = head.get(prefix.len()..).unwrap_or_default();
        Some(parse_difficulty_models(body))
    }

    /// The model work of `difficulty` runs on: that of its difficulty
    /// (medium when unknown) for a by-difficulty arm, the model of a
    /// model arm, and `None` for Gate.
    #[must_use]
    pub fn model_for(&self, difficulty: Option<Difficulty>) -> Option<&str> {
        match self {
            Self::Gate { .. } => None,
            Self::Model { model } => Some(model),
            Self::ByDifficulty { easy, medium, hard } => Some(match difficulty {
                Some(Difficulty::Easy) => easy,
                Some(Difficulty::Hard) => hard,
                Some(Difficulty::Medium) | None => medium,
            }),
        }
    }

    /// Whether Gate routes the arm.
    #[must_use]
    pub const fn is_gate(&self) -> bool {
        matches!(self, Self::Gate { .. })
    }

    /// Whether the arm picks its model by the difficulty of the work.
    #[must_use]
    pub const fn is_by_difficulty(&self) -> bool {
        matches!(self, Self::ByDifficulty { .. })
    }
}

/// The easy, medium and hard models of `body`, the part of a
/// by-difficulty label after its prefix.
fn parse_difficulty_models(body: &str) -> Result<[String; 3], String> {
    let (mut easy, mut medium, mut hard) = (None, None, None);
    for entry in body.split(',').map(str::trim).filter(|e| !e.is_empty()) {
        let Some((key, value)) = entry.split_once('=') else {
            return Err(format!("`{entry}` is not difficulty=model"));
        };
        let key = key.trim().to_ascii_lowercase();
        let difficulty = Difficulty::from_name(&key)
            .ok_or_else(|| format!("Unknown difficulty `{key}`; give easy, medium and hard"))?;
        let value = value.trim();
        if value.is_empty() {
            return Err(format!("No model given for {difficulty}"));
        }
        let slot = match difficulty {
            Difficulty::Easy => &mut easy,
            Difficulty::Medium => &mut medium,
            Difficulty::Hard => &mut hard,
        };
        if slot.is_some() {
            return Err(format!("{difficulty} is given twice"));
        }
        *slot = Some(value.to_string());
    }
    let given = |model: Option<String>, difficulty: Difficulty| {
        model.ok_or_else(|| format!("No model given for {difficulty}"))
    };
    Ok([
        given(easy, Difficulty::Easy)?,
        given(medium, Difficulty::Medium)?,
        given(hard, Difficulty::Hard)?,
    ])
}

/// Tokens and list-price cost one model used in a run.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ModelTokens {
    /// Model as the provider named it.
    pub model: String,
    /// Model calls.
    pub calls: u32,
    /// Prompt tokens, cached ones included.
    pub input_tokens: u64,
    /// Prompt tokens read from the cache.
    pub cache_read_tokens: u64,
    /// Prompt tokens written to the cache.
    pub cache_write_tokens: u64,
    /// Completion tokens.
    pub output_tokens: u64,
    /// List-price cost in USD of the priced calls: those of `calls` not
    /// in `unpriced_calls`. Local models are priced, at 0.
    pub cost_usd: f64,
    /// Calls with no known list price, which `cost_usd` leaves out; 0 in
    /// records written before they were counted.
    #[serde(default, skip_serializing_if = "is_zero_calls")]
    pub unpriced_calls: u32,
}

impl ModelTokens {
    /// Whether some of its calls have no known price, so `cost_usd` is
    /// only what the others cost.
    #[must_use]
    pub const fn is_incomplete(&self) -> bool {
        self.unpriced_calls > 0
    }
}

#[allow(clippy::trivially_copy_pass_by_ref)]
const fn is_zero_calls(calls: &u32) -> bool {
    *calls == 0
}

/// Where a run's workspace copy is and what it started from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompareCopy {
    /// Root of the copy: a detached git worktree, or a plain copy.
    pub root: PathBuf,
    /// The folder the run's agent works in (the user's folder inside
    /// the copy).
    pub workdir: PathBuf,
    /// Git directory kept beside a plain copy to diff it; `None` for a
    /// worktree, which has its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_dir: Option<PathBuf>,
    /// Git tree of the copy before the run, which its diff is taken
    /// against.
    pub baseline: String,
}

/// One prompt of a run: what it cost and how it ended.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunTurn {
    /// Model that ran the turn.
    pub model: String,
    /// List-price cost of the turn's model calls, subtasks included.
    pub cost_usd: f64,
    /// Wall time of the turn.
    pub wall_ms: u64,
    /// How the turn ended, as its `Outcome` records it.
    pub signal: OutcomeSignal,
    /// What its prompt asked for; `None` in older records.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<PromptCategory>,
    /// How hard the classifier judged it, when it said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub difficulty: Option<Difficulty>,
    /// Whether the agent led it, handing its work to subtasks; `None` in
    /// older records.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub led: Option<bool>,
    /// Times a failed attempt moved it to a stronger model.
    #[serde(default)]
    pub escalations: u32,
    /// The model it was first sent to, when a failed attempt moved it on
    /// from there to `model`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_model: Option<String>,
    /// Reasoning effort Gate ran `model` at (`high`, `max`, ...); `None`
    /// when it chose none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// The effort it was first run at, when a failed attempt raised it
    /// from there to `effort`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_effort: Option<String>,
    /// Times Gate asked the model to check a change it had not checked.
    #[serde(default)]
    pub nudges: u32,
    /// Times Gate sent the work back to the model that did it, with what
    /// a review found unfinished or the signs of failure it showed.
    #[serde(default)]
    pub sent_back: u32,
    /// The part of `cost_usd` its subtasks spent.
    #[serde(default)]
    pub subtask_cost_usd: f64,
    /// Its model calls, subtasks included, with no known list price,
    /// which `cost_usd` leaves out; 0 in older records.
    #[serde(default, skip_serializing_if = "is_zero_calls")]
    pub unpriced_calls: u32,
}

impl RunTurn {
    /// A turn on `model` that cost `cost_usd`, took `wall_ms` and ended
    /// with `signal`, with nothing known of how it was routed.
    #[must_use]
    pub fn new(
        model: impl Into<String>,
        cost_usd: f64,
        wall_ms: u64,
        signal: OutcomeSignal,
    ) -> Self {
        Self {
            model: model.into(),
            cost_usd,
            wall_ms,
            signal,
            category: None,
            difficulty: None,
            led: None,
            escalations: 0,
            first_model: None,
            effort: None,
            first_effort: None,
            nudges: 0,
            sent_back: 0,
            subtask_cost_usd: 0.0,
            unpriced_calls: 0,
        }
    }
}

/// One run of a comparison.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompareRun {
    /// `a-1`, `b-2`, ... (`gate-1`, `other-2` in older records)
    pub name: String,
    /// The arm it belongs to.
    pub arm: CompareArm,
    /// The agent that ran it.
    pub agent_id: AgentId,
    /// Its first prompt transaction, once recorded.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::serde_helpers::option_hex_hash"
    )]
    pub prompt_tx: Option<Hash>,
    /// Model that ran the first turn.
    pub model: String,
    /// Its workspace copy.
    pub copy: CompareCopy,
    /// Wall time from the first prompt to the end of the last turn,
    /// subtasks included.
    pub wall_ms: u64,
    /// List-price cost of every priced model call: the turns, their
    /// classifier, and the subtasks they dispatched. Calls with no known
    /// price are left out and counted in `tokens`.
    pub cost_usd: f64,
    /// Tokens and cost per model, most expensive first.
    pub tokens: Vec<ModelTokens>,
    /// Each prompt's turn, in order; empty in older records.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub turns: Vec<RunTurn>,
    /// Share of each plan window the run used, when its calls ran on a
    /// plan whose provider reports usage.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub plan_usage: Vec<PlanShare>,
    /// Whether the check command passed in the copy; `None` without a
    /// check, or when it could not run.
    pub check_passed: Option<bool>,
    /// Files the run changed.
    pub files_changed: u32,
    /// Why the run did not finish, if it did not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Id of the example task it worked on, in a benchmark of example
    /// tasks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
}

impl CompareRun {
    /// Its model calls with no known list price, which `cost_usd` leaves
    /// out.
    #[must_use]
    pub fn unpriced_calls(&self) -> u32 {
        self.tokens.iter().map(|row| row.unpriced_calls).sum()
    }
}

/// How the judge rated an attempt against the reference: what the user
/// kept from the session that was replayed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefVerdict {
    Better,
    AsGood,
    Worse,
}

impl RefVerdict {
    /// `better`, `as good`, `worse`.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Better => "better",
            Self::AsGood => "as good",
            Self::Worse => "worse",
        }
    }
}

/// The blind judge's verdict on one run of arm A paired with one run of
/// arm B.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompareJudgment {
    /// Name of the arm A run.
    #[serde(alias = "gate_run")]
    pub a_run: String,
    /// Name of the arm B run.
    #[serde(alias = "other_run")]
    pub b_run: String,
    /// Whether the judge saw the arm A run as attempt A.
    #[serde(alias = "gate_first")]
    pub a_first: bool,
    /// The better arm; `None` for a tie. Meaningless when not `parsed`.
    pub winner: Option<CompareArm>,
    /// Whether the judge gave a verdict that could be read.
    pub parsed: bool,
    /// How the arm A run compares with the reference, when there is one
    /// and the judge said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub a_vs_reference: Option<RefVerdict>,
    /// How the arm B run compares with the reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub b_vs_reference: Option<RefVerdict>,
    /// Model that judged.
    pub model: String,
    /// List-price cost of the judge call; 0 when its price is unknown,
    /// which `unpriced_calls` says.
    pub cost_usd: f64,
    /// Judge calls with no known list price, which `cost_usd` leaves
    /// out: 1 or 0; 0 in records written before they were counted.
    #[serde(default, skip_serializing_if = "is_zero_calls")]
    pub unpriced_calls: u32,
    /// The judge's reasoning, cut short.
    pub reasoning: String,
}

impl CompareJudgment {
    /// How `arm`'s run compares with the reference.
    #[must_use]
    pub const fn vs_reference(&self, arm: CompareArm) -> Option<RefVerdict> {
        match arm {
            CompareArm::A => self.a_vs_reference,
            CompareArm::B => self.b_vs_reference,
        }
    }
}

/// What a replayed session cost when it ran.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionBaseline {
    /// Model calls.
    pub calls: u32,
    /// List-price cost in USD.
    pub cost_usd: f64,
    /// Whether any call had a price.
    pub priced: bool,
    /// Models the calls went to, first use first.
    pub models: Vec<String>,
}

/// What the user kept from a replayed session, for the judge to measure
/// the runs against.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reference {
    /// The files the session changed, as a unified diff; empty when it
    /// changed none.
    pub diff: String,
    /// The session's last reply.
    pub reply: String,
}

/// Where a comparison's prompts came from.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum BenchSource {
    /// One prompt (or the build prompt of a plan).
    #[default]
    Prompt,
    /// The prompts of an earlier session of `agent_id`, replayed in order
    /// from the files as they were when it started.
    Session {
        agent_id: AgentId,
        started_ms: u64,
        /// Its first prompt, cut short.
        title: String,
        /// What it cost when it ran.
        baseline: SessionBaseline,
        /// What it left; `None` when nothing could be read.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reference: Option<Reference>,
    },
    /// Built-in example coding tasks, each in its own fixture and checked
    /// by its own command. Run `n` of either arm works on task
    /// `(n - 1) % tasks.len()`.
    Examples { tasks: Vec<ExampleTaskInfo> },
}

/// One example coding task of a benchmark.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExampleTaskInfo {
    /// `<suite>/<task>`, as `coding/write-file`.
    pub id: String,
    pub title: String,
    /// The prompts its runs get, in order.
    pub prompts: Vec<String>,
    /// Command run in its copy after the run; exit 0 passes.
    pub check: String,
}

impl BenchSource {
    /// The reference to judge runs against, if any.
    #[must_use]
    pub const fn reference(&self) -> Option<&Reference> {
        match self {
            Self::Session { reference, .. } => reference.as_ref(),
            Self::Prompt | Self::Examples { .. } => None,
        }
    }

    /// What the replayed session cost, if this is one.
    #[must_use]
    pub const fn baseline(&self) -> Option<&SessionBaseline> {
        match self {
            Self::Session { baseline, .. } => Some(baseline),
            Self::Prompt | Self::Examples { .. } => None,
        }
    }

    /// The example tasks, if this is a benchmark of them.
    #[must_use]
    pub fn examples(&self) -> &[ExampleTaskInfo] {
        match self {
            Self::Examples { tasks } => tasks,
            Self::Prompt | Self::Session { .. } => &[],
        }
    }

    /// The example task run `n` (from 1) works on, if any.
    #[must_use]
    pub fn example_for(&self, n: u32) -> Option<&ExampleTaskInfo> {
        let tasks = self.examples();
        let n = usize::try_from(n.max(1) - 1).ok()?;
        (!tasks.is_empty()).then(|| &tasks[n % tasks.len()])
    }
}

/// Where a comparison stands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    rename_all = "snake_case",
    tag = "state",
    content = "detail",
    from = "StoredStatus"
)]
pub enum CompareStatus {
    /// Runs are under way.
    Running,
    /// Every run ended (or was skipped for the budget); copies are kept.
    Done,
    /// The user stopped it; copies are kept.
    Cancelled,
    /// The named run's changes went into the user's workspace and the
    /// copies were removed.
    Applied(String),
    /// The copies were removed without applying anything.
    Dismissed,
}

/// A [`CompareStatus`] as stored, older records' included: those once
/// said which arm the user picked, which leaves a comparison done.
#[derive(Deserialize)]
#[serde(rename_all = "snake_case", tag = "state", content = "detail")]
enum StoredStatus {
    Running,
    Done,
    Cancelled,
    Picked(serde::de::IgnoredAny),
    Applied(String),
    Dismissed,
}

impl From<StoredStatus> for CompareStatus {
    fn from(stored: StoredStatus) -> Self {
        match stored {
            StoredStatus::Running => Self::Running,
            StoredStatus::Done | StoredStatus::Picked(_) => Self::Done,
            StoredStatus::Cancelled => Self::Cancelled,
            StoredStatus::Applied(run) => Self::Applied(run),
            StoredStatus::Dismissed => Self::Dismissed,
        }
    }
}

impl CompareStatus {
    /// Whether the workspace copies still exist.
    #[must_use]
    pub const fn has_copies(&self) -> bool {
        !matches!(self, Self::Applied(_) | Self::Dismissed)
    }
}

/// Payload of a [`SystemKind::Compare`] transaction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ComparePayload {
    /// Always [`SystemKind::Compare`].
    pub system_kind: SystemKind,
    /// What the comparison is about: its prompt, or the first prompt of
    /// a replayed session.
    pub task: String,
    /// Plan file the runs built instead of a prompt, relative to the
    /// workspace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    /// What arm A runs on; Gate in older records.
    pub arm_a: ArmSpec,
    /// What arm B runs on.
    pub arm_b: ArmSpec,
    /// Label of arm B, as older readers expect it.
    pub against: String,
    /// The prompts every run gets, in order; `[task]` in older records.
    #[serde(default)]
    pub prompts: Vec<String>,
    /// Where the prompts came from.
    #[serde(default)]
    pub source: BenchSource,
    /// Runs requested per arm.
    pub repeats: u32,
    /// Command run in each copy after its run; exit 0 passes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check: Option<String>,
    /// Spend after which no new run starts, in USD.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_usd: Option<f64>,
    /// The model that classified each run's prompt; empty in records from
    /// before benchmarks picked their classifier.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub classifier: String,
    /// The user's agent that asked for it.
    pub requested_by: AgentId,
    /// Where the user's changes go when a run is applied: the root of
    /// their git repository, or their folder.
    pub apply_root: PathBuf,
    /// When it started, in Unix milliseconds.
    pub started_ms: u64,
    /// Runs in start order.
    pub runs: Vec<CompareRun>,
    /// The judge's verdicts, one per pair of runs.
    pub judgments: Vec<CompareJudgment>,
    /// Things the user should know, such as changes that could not be
    /// carried into the copies or runs the budget skipped.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
    /// Where it stands.
    pub status: CompareStatus,
}

impl ComparePayload {
    /// The run named `name`.
    #[must_use]
    pub fn run(&self, name: &str) -> Option<&CompareRun> {
        self.runs.iter().find(|run| run.name == name)
    }

    /// Runs of `arm`, in start order.
    pub fn arm_runs(&self, arm: CompareArm) -> impl Iterator<Item = &CompareRun> {
        self.runs.iter().filter(move |run| run.arm == arm)
    }

    /// What `arm` runs on.
    #[must_use]
    pub const fn spec(&self, arm: CompareArm) -> &ArmSpec {
        match arm {
            CompareArm::A => &self.arm_a,
            CompareArm::B => &self.arm_b,
        }
    }

    /// The payload as stored on the transaction.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        // Strings, numbers, paths and enums always serialize; an empty
        // payload would only come from a serde bug.
        serde_json::to_vec(self).unwrap_or_default()
    }

    /// Read a stored payload; `None` for any other system payload.
    /// Records from before arms were chosen freely get arm A as Gate and
    /// arm B from `against`; those from before replays get `[task]` as
    /// their prompts.
    #[must_use]
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        let mut value = serde_json::from_slice::<serde_json::Value>(bytes).ok()?;
        let object = value.as_object_mut()?;
        if object.get("system_kind")?.as_str()? != "compare" {
            return None;
        }
        if !object.contains_key("arm_a") {
            let gate = serde_json::to_value(ArmSpec::Gate { delegate: true }).ok()?;
            object.insert("arm_a".to_string(), gate);
        }
        if !object.contains_key("arm_b") {
            let against = object.get("against")?.as_str()?;
            let other = serde_json::to_value(ArmSpec::from_label(against)).ok()?;
            object.insert("arm_b".to_string(), other);
        }
        let mut payload = serde_json::from_value::<Self>(value)
            .ok()
            .filter(|payload| payload.system_kind == SystemKind::Compare)?;
        if payload.prompts.is_empty() && payload.source.examples().is_empty() {
            payload.prompts.push(payload.task.clone());
        }
        Some(payload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload() -> ComparePayload {
        ComparePayload {
            system_kind: SystemKind::Compare,
            task: "fix the parser".to_string(),
            plan: None,
            arm_a: ArmSpec::Gate { delegate: true },
            arm_b: ArmSpec::Model {
                model: "claude/claude-opus-4-7".to_string(),
            },
            against: "claude/claude-opus-4-7".to_string(),
            prompts: vec!["fix the parser".to_string()],
            source: BenchSource::Prompt,
            repeats: 3,
            check: Some("cargo test".to_string()),
            budget_usd: Some(2.5),
            classifier: "claude/claude-haiku-4-5".to_string(),
            requested_by: AgentId::generate(),
            apply_root: PathBuf::from("/work/repo"),
            started_ms: 1_000,
            runs: vec![CompareRun {
                name: "a-1".to_string(),
                arm: CompareArm::A,
                agent_id: AgentId::generate(),
                prompt_tx: Some(Hash::from_content(b"prompt")),
                model: "claude/claude-sonnet-4-6".to_string(),
                copy: CompareCopy {
                    root: PathBuf::from("/home/.aura/compare/c1/a-1"),
                    workdir: PathBuf::from("/home/.aura/compare/c1/a-1/app"),
                    git_dir: None,
                    baseline: "abc123".to_string(),
                },
                wall_ms: 42_000,
                cost_usd: 0.12,
                tokens: vec![ModelTokens {
                    model: "claude-sonnet-4-6".to_string(),
                    calls: 3,
                    input_tokens: 30_000,
                    output_tokens: 2_000,
                    cost_usd: 0.12,
                    ..ModelTokens::default()
                }],
                turns: vec![RunTurn {
                    category: Some(PromptCategory::Debugging),
                    difficulty: Some(Difficulty::Hard),
                    led: Some(false),
                    escalations: 1,
                    first_model: Some("claude/claude-haiku-4-5".to_string()),
                    effort: Some("max".to_string()),
                    first_effort: Some("high".to_string()),
                    nudges: 1,
                    sent_back: 1,
                    ..RunTurn::new(
                        "claude/claude-sonnet-4-6",
                        0.12,
                        42_000,
                        OutcomeSignal::ChecksPassed,
                    )
                }],
                plan_usage: vec![PlanShare {
                    plan: crate::Plan::Claude,
                    window: "5h".to_string(),
                    first: 0.10,
                    last: 0.12,
                    used: 0.03,
                }],
                check_passed: Some(true),
                files_changed: 2,
                error: None,
                task: None,
            }],
            judgments: vec![CompareJudgment {
                a_run: "a-1".to_string(),
                b_run: "b-1".to_string(),
                a_first: false,
                winner: None,
                parsed: true,
                a_vs_reference: None,
                b_vs_reference: None,
                model: "claude/claude-opus-4-7".to_string(),
                cost_usd: 0.03,
                unpriced_calls: 0,
                reasoning: "Both are fine.".to_string(),
            }],
            notes: Vec::new(),
            status: CompareStatus::Applied("a-1".to_string()),
        }
    }

    #[test]
    fn an_unpriced_judgment_round_trips_and_older_ones_load_as_priced() {
        let mut unpriced = payload();
        unpriced.judgments[0].unpriced_calls = 1;
        let json: serde_json::Value = serde_json::from_slice(&unpriced.to_bytes()).unwrap();
        assert_eq!(json["judgments"][0]["unpriced_calls"], 1);
        assert_eq!(
            ComparePayload::from_bytes(&unpriced.to_bytes()),
            Some(unpriced)
        );

        let mut older = serde_json::to_value(&payload().judgments[0]).unwrap();
        assert!(older.get("unpriced_calls").is_none(), "0 is not written");
        older.as_object_mut().unwrap().remove("unpriced_calls");
        let older: CompareJudgment = serde_json::from_value(older).unwrap();
        assert_eq!(older.unpriced_calls, 0);
    }

    #[test]
    fn compare_payload_round_trips_as_a_compare_system_payload() {
        let payload = payload();
        let bytes = payload.to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["system_kind"], "compare");
        assert_eq!(json["status"]["state"], "applied");
        assert_eq!(json["status"]["detail"], "a-1");
        assert_eq!(json["runs"][0]["arm"], "a");
        assert_eq!(json["arm_a"]["kind"], "gate");
        assert_eq!(json["arm_b"]["model"], "claude/claude-opus-4-7");
        assert_eq!(json["runs"][0]["turns"][0]["signal"], "checks_passed");
        assert_eq!(json["runs"][0]["turns"][0]["category"], "debugging");
        assert_eq!(json["runs"][0]["turns"][0]["escalations"], 1);
        assert_eq!(
            json["runs"][0]["turns"][0]["first_model"],
            "claude/claude-haiku-4-5"
        );
        assert_eq!(json["runs"][0]["turns"][0]["effort"], "max");
        assert_eq!(json["runs"][0]["turns"][0]["first_effort"], "high");
        assert_eq!(json["runs"][0]["turns"][0]["nudges"], 1);
        assert_eq!(json["runs"][0]["turns"][0]["sent_back"], 1);
        assert!(json["runs"][0].get("error").is_none());
        assert!(json["judgments"][0].get("a_vs_reference").is_none());
        assert_eq!(json["runs"][0]["plan_usage"][0]["plan"], "claude");
        assert_eq!(json["classifier"], "claude/claude-haiku-4-5");
        assert_eq!(ComparePayload::from_bytes(&bytes), Some(payload.clone()));

        let mut unnamed = serde_json::to_value(&payload).unwrap();
        unnamed.as_object_mut().unwrap().remove("classifier");
        let unnamed = ComparePayload::from_bytes(&serde_json::to_vec(&unnamed).unwrap()).unwrap();
        assert_eq!(unnamed.classifier, "", "older records name no classifier");
        let no_classifier = ComparePayload {
            classifier: String::new(),
            ..payload.clone()
        };
        let json: serde_json::Value = serde_json::from_slice(&no_classifier.to_bytes()).unwrap();
        assert!(
            json.get("classifier").is_none(),
            "an empty one is not written"
        );

        let mut without = serde_json::to_value(&payload).unwrap();
        let run = without["runs"][0].as_object_mut().unwrap();
        run.remove("plan_usage");
        let turn = run["turns"][0].as_object_mut().unwrap();
        for field in [
            "category",
            "difficulty",
            "led",
            "escalations",
            "first_model",
            "effort",
            "first_effort",
            "nudges",
            "sent_back",
            "subtask_cost_usd",
        ] {
            turn.remove(field);
        }
        let older = ComparePayload::from_bytes(&serde_json::to_vec(&without).unwrap()).unwrap();
        assert!(
            older.runs[0].plan_usage.is_empty(),
            "older records still load"
        );
        assert_eq!(
            older.runs[0].turns,
            [RunTurn::new(
                "claude/claude-sonnet-4-6",
                0.12,
                42_000,
                OutcomeSignal::ChecksPassed
            )]
        );
        without["runs"][0].as_object_mut().unwrap().remove("turns");
        let oldest = ComparePayload::from_bytes(&serde_json::to_vec(&without).unwrap()).unwrap();
        assert!(oldest.runs[0].turns.is_empty());
    }

    #[test]
    fn unpriced_calls_round_trip_and_older_tokens_load_as_all_priced() {
        let row = ModelTokens {
            model: "glm-9".to_string(),
            calls: 4,
            input_tokens: 1_000,
            output_tokens: 100,
            cost_usd: 0.02,
            unpriced_calls: 3,
            ..ModelTokens::default()
        };
        let json = serde_json::to_value(&row).unwrap();
        assert_eq!(json["unpriced_calls"], 3);
        let back: ModelTokens = serde_json::from_value(json).unwrap();
        assert_eq!(back, row);
        assert!(back.is_incomplete());

        let priced = ModelTokens {
            unpriced_calls: 0,
            ..row.clone()
        };
        let json = serde_json::to_value(&priced).unwrap();
        assert!(json.get("unpriced_calls").is_none(), "0 is not written");

        let older = serde_json::json!({
            "model": "claude-sonnet-4-6",
            "calls": 2,
            "input_tokens": 10,
            "cache_read_tokens": 0,
            "cache_write_tokens": 0,
            "output_tokens": 5,
            "cost_usd": 0.01
        });
        let older: ModelTokens = serde_json::from_value(older).unwrap();
        assert_eq!(older.unpriced_calls, 0);
        assert!(!older.is_incomplete());
    }

    #[test]
    fn a_run_counts_the_unpriced_calls_of_its_models() {
        let mut payload = payload();
        let run = &mut payload.runs[0];
        assert_eq!(run.unpriced_calls(), 0);
        run.tokens[0].unpriced_calls = 1;
        run.tokens.push(ModelTokens {
            model: "glm-9".to_string(),
            calls: 2,
            unpriced_calls: 2,
            ..ModelTokens::default()
        });
        run.turns[0].unpriced_calls = 3;
        assert_eq!(run.unpriced_calls(), 3);
        let back = ComparePayload::from_bytes(&payload.to_bytes()).unwrap();
        assert_eq!(back, payload);
    }

    #[test]
    fn records_from_before_free_arms_read_as_gate_against_the_other() {
        let old = serde_json::json!({
            "system_kind": "compare",
            "task": "fix the parser",
            "against": "gate:nodelegate",
            "repeats": 1,
            "requested_by": AgentId::generate(),
            "apply_root": "/work",
            "started_ms": 1,
            "runs": [{
                "name": "gate-1",
                "arm": "gate",
                "agent_id": AgentId::generate(),
                "model": "m",
                "copy": {"root": "/c", "workdir": "/c", "baseline": "t"},
                "wall_ms": 1,
                "cost_usd": 0.1,
                "tokens": [],
                "check_passed": null,
                "files_changed": 0
            }],
            "judgments": [{
                "gate_run": "gate-1",
                "other_run": "other-1",
                "gate_first": true,
                "winner": "other",
                "parsed": true,
                "model": "judge",
                "cost_usd": 0.01,
                "reasoning": ""
            }],
            "status": {"state": "picked", "detail": "gate"}
        });
        let payload = ComparePayload::from_bytes(&serde_json::to_vec(&old).unwrap()).unwrap();
        assert_eq!(payload.arm_a, ArmSpec::Gate { delegate: true });
        assert_eq!(payload.arm_b, ArmSpec::Gate { delegate: false });
        assert_eq!(payload.prompts, ["fix the parser"]);
        assert_eq!(payload.source, BenchSource::Prompt);
        assert_eq!(payload.runs[0].arm, CompareArm::A);
        let judgment = &payload.judgments[0];
        assert_eq!(
            (judgment.a_run.as_str(), judgment.b_run.as_str()),
            ("gate-1", "other-1")
        );
        assert!(judgment.a_first);
        assert_eq!(judgment.winner, Some(CompareArm::B));
        assert_eq!(
            payload.status,
            CompareStatus::Done,
            "a comparison the user once picked an arm of is done"
        );
    }

    #[test]
    fn a_replayed_session_keeps_its_baseline_and_reference() {
        let mut payload = payload();
        payload.prompts = vec!["one".to_string(), "two".to_string()];
        payload.source = BenchSource::Session {
            agent_id: AgentId::generate(),
            started_ms: 7,
            title: "one".to_string(),
            baseline: SessionBaseline {
                calls: 4,
                cost_usd: 0.8,
                priced: true,
                models: vec!["opus".to_string()],
            },
            reference: Some(Reference {
                diff: "+x".to_string(),
                reply: "done".to_string(),
            }),
        };
        payload.judgments[0].a_vs_reference = Some(RefVerdict::AsGood);
        payload.judgments[0].b_vs_reference = Some(RefVerdict::Worse);
        let back = ComparePayload::from_bytes(&payload.to_bytes()).unwrap();
        assert_eq!(back, payload);
        assert_eq!(back.source.reference().unwrap().reply, "done");
        assert!((back.source.baseline().unwrap().cost_usd - 0.8).abs() < 1e-9);
        assert_eq!(
            back.judgments[0].vs_reference(CompareArm::B),
            Some(RefVerdict::Worse)
        );
    }

    #[test]
    fn example_tasks_take_turns_by_run_number_and_keep_no_prompts() {
        let mut payload = payload();
        payload.prompts = Vec::new();
        let task = |id: &str| ExampleTaskInfo {
            id: id.to_string(),
            title: id.to_string(),
            prompts: vec![format!("do {id}")],
            check: "true".to_string(),
        };
        payload.source = BenchSource::Examples {
            tasks: vec![task("coding/one"), task("coding/two")],
        };
        payload.runs[0].task = Some("coding/one".to_string());
        let back = ComparePayload::from_bytes(&payload.to_bytes()).unwrap();
        assert_eq!(back, payload);
        assert!(back.prompts.is_empty());
        let id = |n| back.source.example_for(n).map(|t| t.id.as_str());
        assert_eq!(
            (id(1), id(2), id(3)),
            (Some("coding/one"), Some("coding/two"), Some("coding/one"))
        );
        assert_eq!(BenchSource::Prompt.example_for(1), None);
    }

    #[test]
    fn other_system_payloads_are_not_comparisons() {
        let rewind = serde_json::json!({"system_kind": "rewind", "turn": 1});
        assert_eq!(
            ComparePayload::from_bytes(&serde_json::to_vec(&rewind).unwrap()),
            None
        );
        let mut wrong_kind = serde_json::to_value(payload()).unwrap();
        wrong_kind["system_kind"] = "rewind".into();
        assert_eq!(
            ComparePayload::from_bytes(&serde_json::to_vec(&wrong_kind).unwrap()),
            None
        );
    }

    #[test]
    fn copies_exist_until_applied_or_dismissed() {
        assert!(CompareStatus::Done.has_copies());
        assert!(!CompareStatus::Applied("a-1".to_string()).has_copies());
        assert!(!CompareStatus::Dismissed.has_copies());
        assert_eq!(CompareArm::A.opposite(), CompareArm::B);
    }

    #[test]
    fn runs_are_found_by_name_and_arm() {
        let payload = payload();
        assert_eq!(payload.run("a-1").map(|r| r.wall_ms), Some(42_000));
        assert!(payload.run("b-1").is_none());
        assert_eq!(payload.arm_runs(CompareArm::A).count(), 1);
        assert_eq!(payload.arm_runs(CompareArm::B).count(), 0);
        assert_eq!(
            payload.spec(CompareArm::B).label(),
            "claude/claude-opus-4-7"
        );
    }

    #[test]
    fn arm_labels_read_back() {
        for spec in [
            ArmSpec::Gate { delegate: true },
            ArmSpec::Gate { delegate: false },
            ArmSpec::Model {
                model: "aura-claude-opus-4-7".to_string(),
            },
            by_difficulty(),
        ] {
            assert_eq!(ArmSpec::from_label(&spec.label()), spec);
        }
        assert!(ArmSpec::Gate { delegate: false }.is_gate());
        assert!(!by_difficulty().is_gate());
        assert!(by_difficulty().is_by_difficulty());
        assert_eq!(
            by_difficulty().label(),
            "difficulty:easy=claude/claude-haiku-4-5,medium=openrouter/deepseek/deepseek-v3.2,\
             hard=chatgpt/gpt-5.4"
        );
    }

    fn by_difficulty() -> ArmSpec {
        ArmSpec::ByDifficulty {
            easy: "claude/claude-haiku-4-5".to_string(),
            medium: "openrouter/deepseek/deepseek-v3.2".to_string(),
            hard: "chatgpt/gpt-5.4".to_string(),
        }
    }

    #[test]
    fn a_by_difficulty_arm_round_trips_as_by_difficulty() {
        let mut payload = payload();
        payload.arm_b = by_difficulty();
        let bytes = payload.to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["arm_b"]["kind"], "by_difficulty");
        assert_eq!(json["arm_b"]["hard"], "chatgpt/gpt-5.4");
        assert_eq!(ComparePayload::from_bytes(&bytes), Some(payload));
    }

    #[test]
    fn by_difficulty_labels_take_their_keys_in_any_order() {
        let parsed = ArmSpec::parse_by_difficulty(" Difficulty: hard = c , easy=a,medium= b ");
        assert_eq!(
            parsed,
            Some(Ok(["a".to_string(), "b".to_string(), "c".to_string()]))
        );
        assert_eq!(ArmSpec::parse_by_difficulty("claude/claude-opus-4-7"), None);
        assert_eq!(ArmSpec::parse_by_difficulty("gate"), None);
    }

    #[test]
    fn malformed_by_difficulty_labels_say_what_is_wrong() {
        let error = |label: &str| match ArmSpec::parse_by_difficulty(label) {
            Some(Err(error)) => error,
            other => panic!("{label:?} gave {other:?}"),
        };
        assert!(error("difficulty:easy=a,medium=b").contains("hard"));
        assert!(error("difficulty:easy=a,easy=b,medium=c,hard=d").contains("easy is given twice"));
        assert!(error("difficulty:easy=a,medium=b,hard=c,extreme=d").contains("`extreme`"));
        assert!(error("difficulty:easy=a,medium,hard=c").contains("`medium`"));
        assert!(error("difficulty:easy=,medium=b,hard=c").contains("easy"));
        assert!(error("difficulty:").contains("easy"));
        let malformed = "difficulty:easy=a,medium=b";
        assert_eq!(
            ArmSpec::from_label(malformed),
            ArmSpec::Model {
                model: malformed.to_string()
            },
            "from_label never fails"
        );
    }

    #[test]
    fn each_arm_names_the_model_for_a_difficulty() {
        let spec = by_difficulty();
        assert_eq!(
            spec.model_for(Some(Difficulty::Easy)),
            Some("claude/claude-haiku-4-5")
        );
        assert_eq!(
            spec.model_for(Some(Difficulty::Medium)),
            Some("openrouter/deepseek/deepseek-v3.2")
        );
        assert_eq!(
            spec.model_for(Some(Difficulty::Hard)),
            Some("chatgpt/gpt-5.4")
        );
        assert_eq!(
            spec.model_for(None),
            Some("openrouter/deepseek/deepseek-v3.2"),
            "unknown difficulty runs on the medium model"
        );
        let model = ArmSpec::Model {
            model: "m".to_string(),
        };
        assert_eq!(model.model_for(Some(Difficulty::Hard)), Some("m"));
        assert_eq!(ArmSpec::Gate { delegate: true }.model_for(None), None);
    }
}
