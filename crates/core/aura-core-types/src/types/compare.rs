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
}

impl ArmSpec {
    /// How [`ArmSpec::label`] names Gate whose lead need not delegate.
    pub const NO_DELEGATE: &'static str = "gate:nodelegate";

    /// `gate`, [`Self::NO_DELEGATE`], or the model.
    #[must_use]
    pub fn label(&self) -> &str {
        match self {
            Self::Gate { delegate: true } => "gate",
            Self::Gate { delegate: false } => Self::NO_DELEGATE,
            Self::Model { model } => model,
        }
    }

    /// The arm a label from [`Self::label`] names.
    #[must_use]
    pub fn from_label(label: &str) -> Self {
        match label {
            "gate" => Self::Gate { delegate: true },
            Self::NO_DELEGATE => Self::Gate { delegate: false },
            model => Self::Model {
                model: model.to_string(),
            },
        }
    }

    /// Whether Gate routes the arm.
    #[must_use]
    pub const fn is_gate(&self) -> bool {
        matches!(self, Self::Gate { .. })
    }
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
    /// List-price cost in USD; 0 when the price is unknown.
    pub cost_usd: f64,
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
    /// The part of `cost_usd` its subtasks spent.
    #[serde(default)]
    pub subtask_cost_usd: f64,
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
            subtask_cost_usd: 0.0,
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
    /// List-price cost of every model call: the turns, their classifier,
    /// and the subtasks they dispatched.
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
    /// List-price cost of the judge call.
    pub cost_usd: f64,
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
#[serde(rename_all = "snake_case", tag = "state", content = "detail")]
pub enum CompareStatus {
    /// Runs are under way.
    Running,
    /// Every run ended (or was skipped for the budget); copies are kept.
    Done,
    /// The user stopped it; copies are kept.
    Cancelled,
    /// The user picked the better arm; copies are kept.
    Picked(CompareArm),
    /// The named run's changes went into the user's workspace and the
    /// copies were removed.
    Applied(String),
    /// The copies were removed without applying anything.
    Dismissed,
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
                reasoning: "Both are fine.".to_string(),
            }],
            notes: Vec::new(),
            status: CompareStatus::Picked(CompareArm::A),
        }
    }

    #[test]
    fn compare_payload_round_trips_as_a_compare_system_payload() {
        let payload = payload();
        let bytes = payload.to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["system_kind"], "compare");
        assert_eq!(json["status"]["state"], "picked");
        assert_eq!(json["status"]["detail"], "a");
        assert_eq!(json["runs"][0]["arm"], "a");
        assert_eq!(json["arm_a"]["kind"], "gate");
        assert_eq!(json["arm_b"]["model"], "claude/claude-opus-4-7");
        assert_eq!(json["runs"][0]["turns"][0]["signal"], "checks_passed");
        assert_eq!(json["runs"][0]["turns"][0]["category"], "debugging");
        assert_eq!(json["runs"][0]["turns"][0]["escalations"], 1);
        assert!(json["runs"][0].get("error").is_none());
        assert!(json["judgments"][0].get("a_vs_reference").is_none());
        assert_eq!(json["runs"][0]["plan_usage"][0]["plan"], "claude");
        assert_eq!(ComparePayload::from_bytes(&bytes), Some(payload.clone()));

        let mut without = serde_json::to_value(&payload).unwrap();
        let run = without["runs"][0].as_object_mut().unwrap();
        run.remove("plan_usage");
        let turn = run["turns"][0].as_object_mut().unwrap();
        for field in [
            "category",
            "difficulty",
            "led",
            "escalations",
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
        assert_eq!(payload.status, CompareStatus::Picked(CompareArm::A));
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
        assert!(CompareStatus::Picked(CompareArm::B).has_copies());
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
        ] {
            assert_eq!(ArmSpec::from_label(spec.label()), spec);
        }
        assert!(ArmSpec::Gate { delegate: false }.is_gate());
    }
}
