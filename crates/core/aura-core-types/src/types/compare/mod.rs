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

mod arm;
mod judgment;
mod run;
mod source;
#[cfg(test)]
mod tests;

use std::path::PathBuf;

use super::transaction::SystemKind;
use crate::ids::AgentId;
use serde::{Deserialize, Serialize};

pub use arm::{ArmSpec, CompareArm};
pub use judgment::{CompareJudgment, RefVerdict};
pub use run::{CompareCopy, CompareRun, ModelTokens, RunTurn};
pub use source::{BenchSource, ExampleTaskInfo, Reference, SessionBaseline};

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
    /// The classifier's calls, per model: each prompt is classified once
    /// for both arms, on the comparison's own agent, so they are in
    /// neither arm's totals. Empty while it runs, and in records from
    /// before then, whose runs each paid for their own.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub classify_tokens: Vec<ModelTokens>,
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
