//! Blocks: the units of work a coordinator hands to the gate, and what
//! came back.

use serde::{Deserialize, Serialize};

use crate::tier::Tier;

/// What kind of work a block is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockKind {
    /// A plain shell command; no model runs.
    Command,
    /// Read-only investigation that reports findings.
    Explore,
    /// A focused code change.
    Edit,
    /// Writing or fixing tests.
    Test,
    /// A change spread over many places.
    Refactor,
    /// Documentation or comments.
    Docs,
}

impl BlockKind {
    /// Stable snake-case name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Command => "command",
            Self::Explore => "explore",
            Self::Edit => "edit",
            Self::Test => "test",
            Self::Refactor => "refactor",
            Self::Docs => "docs",
        }
    }

    /// Whether workers of this kind change files.
    #[must_use]
    pub const fn writes(self) -> bool {
        !matches!(self, Self::Command | Self::Explore)
    }
}

/// How costly a wrong result would be.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Risk {
    /// Mistakes are obvious and cheap to fix.
    Low,
    /// The default.
    #[default]
    Medium,
    /// Subtle, security-sensitive or hard-to-verify work.
    High,
}

impl Risk {
    /// Stable snake-case name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }
}

/// How strongly a block's result was checked, weakest first.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum VerifyLevel {
    /// L0: the worker finished.
    #[default]
    Ran,
    /// L1: the build command passed.
    Built,
    /// L2: the checks the worker could see passed.
    Tested,
    /// L3: acceptance checks hidden from the worker passed.
    Accepted,
    /// L4: an independent reviewer approved the change.
    Reviewed,
}

/// Statistics bucket of drafts.
pub const DRAFT_BUCKET: &str = "draft";

/// One unit of work for the gate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockSpec {
    /// Short name, unique within a `run_blocks` call.
    pub title: String,
    /// Kind of work.
    pub kind: BlockKind,
    /// What to do, self-contained: the worker sees nothing else of the
    /// conversation.
    #[serde(default)]
    pub instructions: String,
    /// Shell command for [`BlockKind::Command`] blocks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Files the worker should start from.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
    /// Decisions, excerpts and constraints the worker needs.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub context: String,
    /// Build command that must pass (L1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build: Option<String>,
    /// Test commands the worker is told about and that must pass (L2).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checks: Vec<String>,
    /// Commands that must pass but are not shown to the worker (L3).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub acceptance: Vec<String>,
    /// Acceptance criteria for the worker and the reviewer.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub criteria: Vec<String>,
    /// Ask an independent reviewer to approve the change (L4).
    #[serde(default)]
    pub review: bool,
    /// Cost of a wrong result.
    #[serde(default)]
    pub risk: Risk,
    /// Cheapest tier allowed to do the block.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_tier: Option<Tier>,
    /// The gate's draft of a whole request: one attempt on the fast
    /// tier, which the coordinator reviews instead of the gate escalating.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub draft: bool,
}

impl BlockSpec {
    /// The draft of `request`: the user's words unchanged, done by a fast
    /// worker on its own before the coordinator looks at it.
    #[must_use]
    pub fn draft_of(request: &str) -> Self {
        Self {
            title: "Draft".into(),
            kind: BlockKind::Edit,
            instructions: format!(
                "{}\n\n---\nThis is the user's request, unchanged. Handle all of it on your own: \
                 find the relevant files, make the change, and run whatever check shows it \
                 works. If it asks a question, answer it in your final reply. A stronger model \
                 reviews your work, so say plainly what you were unsure of or left undone.",
                request.trim()
            ),
            command: None,
            files: Vec::new(),
            context: String::new(),
            build: None,
            checks: Vec::new(),
            acceptance: Vec::new(),
            criteria: Vec::new(),
            review: false,
            risk: Risk::Medium,
            min_tier: None,
            draft: true,
        }
    }

    /// The level a passing result must reach.
    #[must_use]
    pub fn required_level(&self) -> VerifyLevel {
        if self.review {
            VerifyLevel::Reviewed
        } else if !self.acceptance.is_empty() {
            VerifyLevel::Accepted
        } else if !self.checks.is_empty() {
            VerifyLevel::Tested
        } else if self.build.is_some() {
            VerifyLevel::Built
        } else {
            VerifyLevel::Ran
        }
    }

    /// Whether anything but the worker's own word decides the outcome.
    #[must_use]
    pub fn verifiable(&self) -> bool {
        self.required_level() > VerifyLevel::Ran
    }

    /// Bucket the gate keeps statistics for: kind, risk, whether the
    /// result is checked, and how many files the block starts from.
    #[must_use]
    pub fn bucket(&self) -> String {
        if self.draft {
            return DRAFT_BUCKET.to_string();
        }
        let checked = if self.verifiable() {
            "checked"
        } else {
            "unchecked"
        };
        let size = match self.files.len() {
            0..=2 => "small",
            _ => "wide",
        };
        format!(
            "{}/{}/{checked}/{size}",
            self.kind.as_str(),
            self.risk.as_str()
        )
    }

    /// Reject specs the runner can't act on.
    ///
    /// # Errors
    /// Describes the first problem found.
    pub fn validate(&self) -> Result<(), String> {
        if self.title.trim().is_empty() {
            return Err("every block needs a title".into());
        }
        match self.kind {
            BlockKind::Command => {
                if self.command.as_deref().is_none_or(|c| c.trim().is_empty()) {
                    return Err(format!("command block `{}` has no command", self.title));
                }
            }
            _ if self.instructions.trim().is_empty() => {
                return Err(format!("block `{}` has no instructions", self.title));
            }
            _ => {}
        }
        Ok(())
    }
}

/// Result of one verification command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckOutcome {
    /// `build`, `check` or `acceptance`.
    pub stage: String,
    /// Command run.
    pub command: String,
    /// Whether it exited 0.
    pub passed: bool,
    /// Exit code, when the command ran.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// Last part of its output.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub output: String,
}

/// One worker run on a block.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Attempt {
    /// 1-based attempt number within the block.
    pub n: u32,
    /// Tier that ran.
    pub tier: Tier,
    /// Model id that ran.
    pub model: String,
    /// Reasoning effort requested.
    pub effort: String,
    /// Why this tier: `rule`, `earned`, `cost`, `explore`, `fix`,
    /// `escalate` or `min_tier`.
    pub reason: String,
    /// Chosen by exploration rather than the policy's best guess.
    #[serde(default)]
    pub explored: bool,
    /// Probability the policy gave this choice.
    pub propensity: f64,
    /// Whether the block's checks passed after this attempt.
    pub passed: bool,
    /// Level reached.
    pub level: VerifyLevel,
    /// Why it failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<String>,
    /// Verification commands run after the attempt.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checks: Vec<CheckOutcome>,
    /// List price of the worker's (and reviewer's) model calls.
    pub cost_usd: f64,
    /// Prompt tokens.
    pub input_tokens: u64,
    /// Generated tokens.
    pub output_tokens: u64,
    /// Files the worker changed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files_changed: Vec<String>,
    /// The worker's closing summary.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub summary: String,
    /// Child agent that ran, hex.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// Wall time.
    pub wall_ms: u64,
}

/// How a block ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockStatus {
    /// Reached its required level.
    Passed,
    /// Every allowed attempt failed; its changes were undone when the
    /// workspace could be restored.
    Failed,
    /// Not run because an earlier block failed.
    Skipped,
}

/// What the gate reports back for one block.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BlockResult {
    /// Block title.
    pub title: String,
    /// Block kind.
    pub kind: BlockKind,
    /// Statistics bucket.
    pub bucket: String,
    /// Outcome.
    pub status: BlockStatus,
    /// Level the final attempt reached.
    pub level: VerifyLevel,
    /// Level it had to reach.
    pub required: VerifyLevel,
    /// Tier of the final attempt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier: Option<Tier>,
    /// Model of the final attempt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Final worker summary or command output.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub summary: String,
    /// Files the block left changed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files_changed: Vec<String>,
    /// Unified diff of what a passed block changed, cut to a few KB.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub diff: String,
    /// Why it failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<String>,
    /// Whether failed attempts' changes were rolled back.
    #[serde(default)]
    pub restored: bool,
    /// List price of every attempt.
    pub cost_usd: f64,
    /// Every attempt, in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attempts: Vec<Attempt>,
}

impl BlockResult {
    /// A block left unrun.
    #[must_use]
    pub fn skipped(spec: &BlockSpec) -> Self {
        Self {
            title: spec.title.clone(),
            kind: spec.kind,
            bucket: spec.bucket(),
            status: BlockStatus::Skipped,
            level: VerifyLevel::Ran,
            required: spec.required_level(),
            tier: None,
            model: None,
            summary: String::new(),
            files_changed: Vec::new(),
            diff: String::new(),
            failure: Some("not run: an earlier block failed".into()),
            restored: false,
            cost_usd: 0.0,
            attempts: Vec::new(),
        }
    }
}

/// Arguments of `run_blocks`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunBlocksInput {
    /// Blocks to run in order.
    pub blocks: Vec<BlockSpec>,
}

/// Result of `run_blocks`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunBlocksOutput {
    /// Blocks that passed.
    pub passed: usize,
    /// Blocks that failed.
    pub failed: usize,
    /// Blocks not run.
    pub skipped: usize,
    /// List price of every attempt of every block.
    pub cost_usd: f64,
    /// One result per block, in order.
    pub blocks: Vec<BlockResult>,
}

impl RunBlocksOutput {
    /// Totals over `blocks`.
    #[must_use]
    pub fn new(blocks: Vec<BlockResult>) -> Self {
        let count = |s: BlockStatus| blocks.iter().filter(|b| b.status == s).count();
        Self {
            passed: count(BlockStatus::Passed),
            failed: count(BlockStatus::Failed),
            skipped: count(BlockStatus::Skipped),
            cost_usd: blocks.iter().map(|b| b.cost_usd).sum(),
            blocks,
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn edit(title: &str) -> BlockSpec {
        BlockSpec {
            title: title.into(),
            kind: BlockKind::Edit,
            instructions: "change it".into(),
            command: None,
            files: vec!["src/lib.rs".into()],
            context: String::new(),
            build: None,
            checks: Vec::new(),
            acceptance: Vec::new(),
            criteria: Vec::new(),
            review: false,
            risk: Risk::Medium,
            min_tier: None,
            draft: false,
        }
    }

    #[test]
    fn required_level_is_the_strongest_check_given() {
        let mut spec = edit("a");
        assert_eq!(spec.required_level(), VerifyLevel::Ran);
        spec.build = Some("cargo check".into());
        assert_eq!(spec.required_level(), VerifyLevel::Built);
        spec.checks.push("cargo test".into());
        assert_eq!(spec.required_level(), VerifyLevel::Tested);
        spec.acceptance.push("sh hidden.sh".into());
        assert_eq!(spec.required_level(), VerifyLevel::Accepted);
        spec.review = true;
        assert_eq!(spec.required_level(), VerifyLevel::Reviewed);
    }

    #[test]
    fn bucket_names_kind_risk_checking_and_size() {
        let mut spec = edit("a");
        assert_eq!(spec.bucket(), "edit/medium/unchecked/small");
        spec.checks.push("t".into());
        spec.files = vec!["a".into(), "b".into(), "c".into()];
        spec.risk = Risk::High;
        assert_eq!(spec.bucket(), "edit/high/checked/wide");
    }

    #[test]
    fn validate_requires_instructions_or_command() {
        let mut spec = edit("a");
        assert!(spec.validate().is_ok());
        spec.instructions.clear();
        assert!(spec.validate().is_err());
        spec.kind = BlockKind::Command;
        assert!(spec.validate().is_err());
        spec.command = Some("cargo fmt".into());
        assert!(spec.validate().is_ok());
    }

    #[test]
    fn spec_parses_from_minimal_json() {
        let spec: BlockSpec = serde_json::from_value(serde_json::json!({
            "title": "fmt", "kind": "command", "command": "cargo fmt"
        }))
        .unwrap();
        assert_eq!(spec.risk, Risk::Medium);
        assert!(!spec.review);
    }
}
