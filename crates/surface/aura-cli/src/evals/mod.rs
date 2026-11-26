//! Eval suites: coding tasks with a verification command, run headless
//! and recorded as `eval` runs so harness arms can be compared.
//!
//! A suite is a directory of tasks. Each task directory holds a
//! `task.toml`, an optional `workspace/` fixture that is copied into a
//! fresh directory before the run, and an optional `checks/` directory
//! copied in after the run, before `verify`, so the agent never sees the
//! hidden checks. A task set in an existing codebase names a `[source]`,
//! a GitHub repository at a commit, whose files go into the directory
//! first (downloaded once into `AURA_HOME/evals/sources`):
//!
//! ```toml
//! title = "Fix the off-by-one"
//! category = "bugfix"
//! tags = ["rust"]
//! prompt = "The tests in src/lib.rs fail. Fix the bug."
//! verify = "cargo test --offline"
//! max_iterations = 30
//!
//! [source]
//! repo = "https://github.com/<owner>/<name>"
//! commit = "<the full hash of the commit before the fix>"
//! ```
//!
//! On a model, each run is the plain agent loop with a kernel of its own
//! whose record is kept in the run's bundle, and `verify` runs with
//! `sh -c` after it. Under `gate` ([`EvalGate`]) each run is an agent in
//! the data directory, its prompts are `aura -p` turns, and `verify` is a
//! `run_command` call through that agent's kernel whose verdict feeds
//! Gate's table.

use std::path::PathBuf;
use std::sync::Arc;

use aura_agent::AgentLoopConfig;
use aura_agent_trace::RunScorecard;
use aura_config::trace::TraceLevel;
use aura_exec_tools::ToolConfig;
use aura_model::ModelProvider;

pub use aura_app::headless::{copy_dir, GateUse};

mod comparison;
mod report;
mod run;
mod suite;
#[cfg(test)]
mod tests;

pub use comparison::{compare, runs_by_arm, Baseline};
pub use report::results_table;
pub use run::{run_suite, verify};
pub use suite::{categories, load_suite, suite_name, EvalTask};

/// Task definition file inside a task directory.
pub const TASK_FILE: &str = "task.toml";
/// Fixture directory copied into the run workspace.
pub const FIXTURE_DIR: &str = "workspace";
/// Hidden checks copied into the run workspace after the run.
pub const CHECKS_DIR: &str = "checks";
/// Category of tasks that name none.
pub const DEFAULT_CATEGORY: &str = "general";
/// Arm name used when a run has none.
pub const DEFAULT_ARM: &str = "default";

/// Builds the model provider for one run of a task.
pub type ProviderFactory =
    Box<dyn Fn(&EvalTask) -> Arc<dyn ModelProvider + Send + Sync> + Send + Sync>;

/// What runs the tasks of a suite.
pub enum EvalRunner {
    /// The agent loop on one model, with its own kernel per run.
    Model {
        /// Loop configuration (model, limits).
        config: Box<AgentLoopConfig>,
        /// Tool configuration.
        tool_config: ToolConfig,
        /// Provider for one run of a task.
        provider: ProviderFactory,
    },
    /// Gate, as `aura -p` runs it.
    Gate(EvalGate),
}

/// Runs tasks under `gate` as `aura -p` turns: classified and routed with
/// Gate's table, the agent leading where the category calls for it and
/// its subtasks routed, the prompts, `Outcome`s and verify verdicts
/// recorded in the data directory's agent records, so they count in that
/// table.
pub struct EvalGate {
    turns: tokio::sync::Mutex<aura_app::headless::EvalTurns>,
}

impl EvalGate {
    /// Gate on the agents and records of the terminal started in the
    /// current directory, with `provider` (`mock` answers without a model).
    ///
    /// # Errors
    /// When the sign-in state or data directory can't be read, or a
    /// terminal has the data directory open.
    pub(crate) async fn start(provider: &str) -> anyhow::Result<Self> {
        let headless = aura_app::headless::eval_harness(provider).await?;
        let turns = aura_app::headless::EvalTurns::new(headless);
        // On stderr like the runs' progress, so stdout stays the table.
        for line in turns.setup_lines() {
            eprintln!("{line}");
        }
        Ok(Self::on(turns))
    }

    pub(crate) fn on(turns: aura_app::headless::EvalTurns) -> Self {
        Self {
            turns: tokio::sync::Mutex::new(turns),
        }
    }
}

/// Settings shared by every task in a suite run.
pub struct EvalOptions {
    /// `AURA_HOME`.
    pub home: PathBuf,
    /// Suite name for `task_id`.
    pub suite: String,
    /// Harness arm label.
    pub arm: Option<String>,
    /// Runs per task.
    pub repeat: usize,
    /// Trace level.
    pub level: TraceLevel,
    /// What runs the tasks.
    pub runner: EvalRunner,
    /// Where task workspaces are created.
    pub work_root: PathBuf,
    /// Stop starting runs once the suite's runs cost this much.
    pub budget_usd: Option<f64>,
}

/// Result of one task run.
#[derive(Debug, Clone)]
pub struct EvalResult {
    /// Task id.
    pub task: String,
    /// Recorded run id.
    pub run_id: String,
    /// Whether verification passed.
    pub passed: bool,
    /// Loop error, if the run ended early.
    pub error: Option<String>,
    /// Scorecard including the verification outcome.
    pub scorecard: RunScorecard,
    /// What Gate did, under `gate`.
    pub gate: Option<GateUse>,
}

impl EvalResult {
    /// What the run cost at list prices: under Gate its subtasks and
    /// classifier calls included, else the scorecard's.
    #[must_use]
    pub fn cost_usd(&self) -> f64 {
        self.gate.as_ref().map_or(self.scorecard.cost_usd, |gate| {
            gate.tokens.iter().map(|m| m.cost_usd).sum()
        })
    }

    /// Model calls under Gate with no known list price, which
    /// [`Self::cost_usd`] leaves out.
    #[must_use]
    pub fn unpriced_calls(&self) -> u32 {
        self.gate
            .as_ref()
            .map_or(0, |gate| gate.tokens.iter().map(|m| m.unpriced_calls).sum())
    }
}
