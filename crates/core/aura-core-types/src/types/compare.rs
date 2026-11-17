//! What `/compare` records: one [`ComparePayload`] per state change of a
//! comparison (started, finished, picked, applied, dismissed), written as
//! a [`SystemKind::Compare`] system transaction on the comparison's own
//! agent. The latest one is the comparison as it stands.
//!
//! The runs themselves are ordinary agents with ordinary prompt and
//! `Outcome` records; this payload only says which runs belong to which
//! arm, what they measured, how the judge saw them, and where their
//! workspace copies are.

use std::path::PathBuf;

use super::plan_usage::PlanShare;
use super::transaction::SystemKind;
use crate::ids::{AgentId, Hash};
use serde::{Deserialize, Serialize};

/// Which side of a comparison a run is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompareArm {
    /// Gate routes the run, with exploration off.
    Gate,
    /// The run is pinned to the model Gate is compared with.
    Other,
}

impl CompareArm {
    /// Both arms, Gate first.
    pub const BOTH: [Self; 2] = [Self::Gate, Self::Other];

    /// The arm's name in run names and commands: `gate` or `other`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Gate => "gate",
            Self::Other => "other",
        }
    }

    /// The other arm.
    #[must_use]
    pub const fn opposite(self) -> Self {
        match self {
            Self::Gate => Self::Other,
            Self::Other => Self::Gate,
        }
    }
}

impl std::fmt::Display for CompareArm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
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

/// One run of a comparison.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompareRun {
    /// `gate-1`, `other-2`, ...
    pub name: String,
    /// The arm it belongs to.
    pub arm: CompareArm,
    /// The agent that ran it.
    pub agent_id: AgentId,
    /// Its prompt transaction, once recorded.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::serde_helpers::option_hex_hash"
    )]
    pub prompt_tx: Option<Hash>,
    /// Model that ran the turn.
    pub model: String,
    /// Its workspace copy.
    pub copy: CompareCopy,
    /// Wall time from the prompt to the end of the turn, subtasks
    /// included.
    pub wall_ms: u64,
    /// List-price cost of every model call: the turn, its classifier,
    /// and the subtasks it dispatched.
    pub cost_usd: f64,
    /// Tokens and cost per model, most expensive first.
    pub tokens: Vec<ModelTokens>,
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
}

/// The blind judge's verdict on one Gate run paired with one run of the
/// other arm.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompareJudgment {
    /// Name of the Gate run.
    pub gate_run: String,
    /// Name of the other arm's run.
    pub other_run: String,
    /// Whether the judge saw the Gate run as attempt A.
    pub gate_first: bool,
    /// The better arm; `None` for a tie. Meaningless when not `parsed`.
    pub winner: Option<CompareArm>,
    /// Whether the judge gave a verdict that could be read.
    pub parsed: bool,
    /// Model that judged.
    pub model: String,
    /// List-price cost of the judge call.
    pub cost_usd: f64,
    /// The judge's reasoning, cut short.
    pub reasoning: String,
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
    /// The prompt every run got.
    pub task: String,
    /// Plan file the runs built instead of a prompt, relative to the
    /// workspace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    /// Model the other arm is pinned to.
    pub against: String,
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

    /// The payload as stored on the transaction.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        // Strings, numbers, paths and enums always serialize; an empty
        // payload would only come from a serde bug.
        serde_json::to_vec(self).unwrap_or_default()
    }

    /// Read a stored payload; `None` for any other system payload.
    #[must_use]
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        serde_json::from_slice::<Self>(bytes)
            .ok()
            .filter(|payload| payload.system_kind == SystemKind::Compare)
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
            against: "claude/claude-opus-4-7".to_string(),
            repeats: 3,
            check: Some("cargo test".to_string()),
            budget_usd: Some(2.5),
            requested_by: AgentId::generate(),
            apply_root: PathBuf::from("/work/repo"),
            started_ms: 1_000,
            runs: vec![CompareRun {
                name: "gate-1".to_string(),
                arm: CompareArm::Gate,
                agent_id: AgentId::generate(),
                prompt_tx: Some(Hash::from_content(b"prompt")),
                model: "claude/claude-sonnet-4-6".to_string(),
                copy: CompareCopy {
                    root: PathBuf::from("/home/.aura/compare/c1/gate-1"),
                    workdir: PathBuf::from("/home/.aura/compare/c1/gate-1/app"),
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
            }],
            judgments: vec![CompareJudgment {
                gate_run: "gate-1".to_string(),
                other_run: "other-1".to_string(),
                gate_first: false,
                winner: None,
                parsed: true,
                model: "claude/claude-opus-4-7".to_string(),
                cost_usd: 0.03,
                reasoning: "Both are fine.".to_string(),
            }],
            notes: Vec::new(),
            status: CompareStatus::Picked(CompareArm::Gate),
        }
    }

    #[test]
    fn compare_payload_round_trips_as_a_compare_system_payload() {
        let payload = payload();
        let bytes = payload.to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["system_kind"], "compare");
        assert_eq!(json["status"]["state"], "picked");
        assert_eq!(json["status"]["detail"], "gate");
        assert_eq!(json["runs"][0]["arm"], "gate");
        assert!(json["runs"][0].get("error").is_none());
        assert_eq!(json["runs"][0]["plan_usage"][0]["plan"], "claude");
        assert_eq!(ComparePayload::from_bytes(&bytes), Some(payload.clone()));

        let mut without = serde_json::to_value(&payload).unwrap();
        without["runs"][0]
            .as_object_mut()
            .unwrap()
            .remove("plan_usage");
        let older = ComparePayload::from_bytes(&serde_json::to_vec(&without).unwrap()).unwrap();
        assert!(
            older.runs[0].plan_usage.is_empty(),
            "older records still load"
        );
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
        assert!(CompareStatus::Picked(CompareArm::Other).has_copies());
        assert!(!CompareStatus::Applied("gate-1".to_string()).has_copies());
        assert!(!CompareStatus::Dismissed.has_copies());
        assert_eq!(CompareArm::Gate.opposite(), CompareArm::Other);
    }

    #[test]
    fn runs_are_found_by_name_and_arm() {
        let payload = payload();
        assert_eq!(payload.run("gate-1").map(|r| r.wall_ms), Some(42_000));
        assert!(payload.run("other-1").is_none());
        assert_eq!(payload.arm_runs(CompareArm::Gate).count(), 1);
        assert_eq!(payload.arm_runs(CompareArm::Other).count(), 0);
    }
}
