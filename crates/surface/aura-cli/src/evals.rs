//! Eval suites: coding tasks with a verification command, run headless
//! and recorded as `eval` runs so harness arms can be compared.
//!
//! A suite is a directory of tasks. Each task directory holds a
//! `task.toml`, an optional `workspace/` fixture that is copied into a
//! fresh directory before the run, and an optional `checks/` directory
//! copied in after the run, before `verify`, so the agent never sees the
//! hidden checks:
//!
//! ```toml
//! title = "Fix the off-by-one"
//! category = "bugfix"
//! tags = ["rust"]
//! prompt = "The tests in src/lib.rs fail. Fix the bug."
//! verify = "cargo test --offline"
//! max_iterations = 30
//! ```
//!
//! On a model, each run is the plain agent loop with a kernel of its own
//! whose record is kept in the run's bundle, and `verify` runs with
//! `sh -c` after it. Under `gate` ([`EvalGate`]) each run is an agent in
//! the data directory, its prompts are `aura -p` turns, and `verify` is a
//! `run_command` call through that agent's kernel whose verdict feeds
//! Gate's table.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{bail, Context};
use aura_agent::AgentLoopConfig;
use aura_agent_trace::bundle::{list_runs, now_ms, RunBundle};
use aura_agent_trace::{scorecard, RunMeta, RunRecorder, RunScorecard, RunSource, RunTrace};
use aura_config::trace::TraceLevel;
use aura_core_types::trace::{Reward, RewardKind, RewardTarget};
use aura_exec_tools::ToolConfig;
use aura_model::model_catalog::GATE_MODEL_ID;
use aura_model::ModelProvider;
use serde::{Deserialize, Serialize};
use serde_json::json;

use aura_app::headless::EvalTurn;
pub use aura_app::headless::{copy_dir, GateUse};
use aura_app::recorded_runs::{self, HeadlessRun};

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

/// One eval task.
#[derive(Debug, Clone, Deserialize)]
pub struct EvalTask {
    /// Directory name; filled in by [`load_suite`].
    #[serde(skip)]
    pub id: String,
    /// Task directory; filled in by [`load_suite`].
    #[serde(skip)]
    pub dir: PathBuf,
    /// Short title.
    #[serde(default)]
    pub title: String,
    /// Category reports group by (default [`DEFAULT_CATEGORY`]).
    #[serde(default)]
    pub category: String,
    /// Free-form labels.
    #[serde(default)]
    pub tags: Vec<String>,
    /// First user prompt.
    pub prompt: String,
    /// Follow-up prompts, one per extra turn.
    #[serde(default)]
    pub follow_ups: Vec<String>,
    /// Shell command run in the workspace; exit status 0 passes.
    pub verify: String,
    /// Iteration cap for each turn.
    #[serde(default)]
    pub max_iterations: Option<usize>,
}

/// Load every task under `suite`, sorted by id.
///
/// # Errors
///
/// Fails when the directory can't be read or a `task.toml` is invalid.
pub fn load_suite(suite: &Path) -> anyhow::Result<Vec<EvalTask>> {
    let mut tasks = Vec::new();
    let entries =
        std::fs::read_dir(suite).with_context(|| format!("reading suite {}", suite.display()))?;
    for entry in entries {
        let dir = entry?.path();
        let file = dir.join(TASK_FILE);
        if !file.is_file() {
            continue;
        }
        let text = std::fs::read_to_string(&file)?;
        let mut task: EvalTask =
            toml::from_str(&text).with_context(|| format!("parsing {}", file.display()))?;
        task.id = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if task.title.is_empty() {
            task.title.clone_from(&task.id);
        }
        if task.category.is_empty() {
            task.category = DEFAULT_CATEGORY.to_string();
        }
        task.dir = dir;
        tasks.push(task);
    }
    if tasks.is_empty() {
        bail!("no tasks (`*/{TASK_FILE}`) under {}", suite.display());
    }
    tasks.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(tasks)
}

/// Suite name recorded in each run's `task_id` (`<suite>/<task>`).
#[must_use]
pub fn suite_name(suite: &Path) -> String {
    suite
        .file_name()
        .map_or_else(|| "suite".into(), |n| n.to_string_lossy().into_owned())
}

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
/// Gate's table, the agent leading and its subtasks routed, the prompts,
/// `Outcome`s and verify verdicts recorded in the data directory's agent
/// records, so they count in that table.
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
        Ok(Self::on(aura_app::headless::EvalTurns::new(headless)))
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
}

/// Run every task `opts.repeat` times.
///
/// # Errors
///
/// Fails when a workspace can't be prepared or a run can't be recorded.
pub async fn run_suite(tasks: &[EvalTask], opts: &EvalOptions) -> anyhow::Result<Vec<EvalResult>> {
    let mut results = Vec::new();
    let mut spent = 0.0;
    for task in tasks {
        for _ in 0..opts.repeat.max(1) {
            if opts.budget_usd.is_some_and(|budget| spent >= budget) {
                tracing::warn!(spent, "Eval budget reached; skipping the remaining runs");
                return Ok(results);
            }
            let result = run_task(task, opts).await?;
            spent += result.cost_usd();
            results.push(result);
        }
    }
    Ok(results)
}

/// Category of each task id (`<suite>/<task>`) in `tasks`.
#[must_use]
pub fn categories(suite: &str, tasks: &[EvalTask]) -> BTreeMap<String, String> {
    tasks
        .iter()
        .map(|t| (format!("{suite}/{}", t.id), t.category.clone()))
        .collect()
}

async fn run_task(task: &EvalTask, opts: &EvalOptions) -> anyhow::Result<EvalResult> {
    let model = match &opts.runner {
        EvalRunner::Model { config, .. } => config.model.clone(),
        EvalRunner::Gate(_) => GATE_MODEL_ID.to_string(),
    };
    let task_id = format!("{}/{}", opts.suite, task.id);
    let mut meta = RunMeta::new(model, RunSource::Eval);
    meta.task_id = Some(task_id.clone());
    meta.title.clone_from(&task.title);
    meta.arm.clone_from(&opts.arm);

    let workspace = opts.work_root.join(format!("{}-{}", task.id, meta.run_id));
    std::fs::create_dir_all(&workspace)?;
    let fixture = task.dir.join(FIXTURE_DIR);
    if fixture.is_dir() {
        copy_dir(&fixture, &workspace)?;
    }
    let mut prompts = vec![task.prompt.clone()];
    prompts.extend(task.follow_ups.iter().cloned());
    let checks = task.dir.join(CHECKS_DIR);

    let (config, tool_config, provider) = match &opts.runner {
        EvalRunner::Model {
            config,
            tool_config,
            provider,
        } => (config, tool_config, provider),
        EvalRunner::Gate(gate) => {
            let workspace = workspace.canonicalize()?;
            meta.workspace = workspace.display().to_string();
            let level = match opts.level {
                TraceLevel::Off => TraceLevel::Decisions,
                level => level,
            };
            let trace = RunRecorder::start(&opts.home, meta, level)?.context("tracing is off")?;
            let ran = gate
                .turns
                .lock()
                .await
                .run(EvalTurn {
                    name: &format!("Eval {task_id}"),
                    workspace: &workspace,
                    prompts: &prompts,
                    max_iterations: task.max_iterations,
                    checks: checks.is_dir().then_some(checks.as_path()),
                    verify: &task.verify,
                    trace: Arc::clone(&trace),
                })
                .await?;
            let passed = ran.check.passed == Some(true);
            trace.add_rewards(&[Reward {
                target: RewardTarget::Run,
                kind: RewardKind::Outcome,
                name: "verify".into(),
                value: if passed { 1.0 } else { 0.0 },
                source: "eval".into(),
                at_ms: now_ms(),
                evidence: json!({
                    "cmd": task.verify,
                    "exit_code": ran.check.exit_code,
                    "ms": ran.check_ms,
                    "output_tail": ran.check.tail,
                    "via": "run_command",
                }),
            }]);
            let card = trace.finish().context("reading the run's trace")?;
            return Ok(EvalResult {
                task: task.id.clone(),
                run_id: trace.run_id(),
                passed,
                error: ran.error,
                scorecard: card,
                gate: Some(ran.gate),
            });
        }
    };

    let mut config = AgentLoopConfig::clone(config);
    if let Some(max) = task.max_iterations {
        config.max_iterations = max;
    }
    let outcome = recorded_runs::run(HeadlessRun {
        home: opts.home.clone(),
        workspace: workspace.clone(),
        provider: provider(task),
        config,
        prompts,
        meta,
        level: opts.level,
        tool_config: tool_config.clone(),
    })
    .await?;

    if checks.is_dir() {
        copy_dir(&checks, &workspace)?;
    }
    let (passed, reward) = verify(&task.verify, &workspace, "eval")?;
    let bundle = RunBundle::at(&outcome.bundle_dir);
    bundle.append_rewards(&[reward])?;
    let card = RunTrace::load(&bundle)?.scorecard();
    bundle.write_summary(&card)?;
    Ok(EvalResult {
        task: task.id.clone(),
        run_id: outcome.run_id,
        passed,
        error: outcome.error,
        scorecard: card,
        gate: None,
    })
}

/// Run `cmd` with `sh -c` in `dir` and turn its exit status into an
/// outcome reward (1 passed, 0 failed) with the output tails as evidence.
/// Cargo builds into `dir/target`, so workspaces of crates with the same
/// name never share build results.
///
/// # Errors
///
/// Fails when the shell can't be started.
pub fn verify(cmd: &str, dir: &Path, source: &str) -> anyhow::Result<(bool, Reward)> {
    let started = now_ms();
    let output = std::process::Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .current_dir(dir)
        .env("CARGO_TARGET_DIR", dir.join("target"))
        .output()
        .with_context(|| format!("running `{cmd}` in {}", dir.display()))?;
    let passed = output.status.success();
    let tail = |b: &[u8]| {
        let s = String::from_utf8_lossy(b);
        let lines: Vec<&str> = s.lines().rev().take(20).collect();
        lines.into_iter().rev().collect::<Vec<_>>().join("\n")
    };
    let reward = Reward {
        target: RewardTarget::Run,
        kind: RewardKind::Outcome,
        name: "verify".into(),
        value: if passed { 1.0 } else { 0.0 },
        source: source.into(),
        at_ms: now_ms(),
        evidence: json!({
            "cmd": cmd,
            "exit_code": output.status.code(),
            "ms": now_ms().saturating_sub(started),
            "stdout_tail": tail(&output.stdout),
            "stderr_tail": tail(&output.stderr),
        }),
    };
    Ok((passed, reward))
}

/// Table of suite results.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn results_table(results: &[EvalResult]) -> String {
    let mut out = format!(
        "{:<24} {:<6} {:>9} {:>6} {:>9} {:>8}  {}\n",
        "task", "result", "tokens", "calls", "usd", "secs", "run"
    );
    for r in results {
        let _ = writeln!(
            out,
            "{:<24} {:<6} {:>9} {:>6} {:>9.4} {:>8.1}  {}{}",
            r.task,
            if r.passed { "pass" } else { "FAIL" },
            r.scorecard.total_tokens(),
            r.scorecard.model_calls,
            r.cost_usd(),
            r.scorecard.wall_ms as f64 / 1000.0,
            r.run_id,
            r.error
                .as_deref()
                .map(|e| format!("  ({e})"))
                .unwrap_or_default(),
        );
        if let Some(gate) = &r.gate {
            let calls: u32 = gate.tokens.iter().map(|m| m.calls).sum();
            let _ = writeln!(
                out,
                "  gate routed to {} ({calls} model calls with subtasks and classifier)",
                if gate.routed.is_empty() {
                    "nothing".to_string()
                } else {
                    gate.routed.join(", ")
                }
            );
        }
    }
    let passed = results.iter().filter(|r| r.passed).count();
    let cost: f64 = results.iter().map(EvalResult::cost_usd).sum();
    let _ = writeln!(out, "{passed}/{} passed, ${cost:.4}", results.len());
    out
}

/// Scorecards of recorded eval runs grouped by arm, optionally limited
/// to one suite.
#[must_use]
pub fn runs_by_arm(
    home: &Path,
    suite: Option<&str>,
) -> BTreeMap<String, Vec<(String, RunScorecard)>> {
    let mut arms: BTreeMap<String, Vec<(String, RunScorecard)>> = BTreeMap::new();
    for (meta, card) in list_runs(home) {
        let Some(card) = card else { continue };
        if meta.source != RunSource::Eval {
            continue;
        }
        let task = meta.task_id.clone().unwrap_or_default();
        if suite.is_some_and(|s| !task.starts_with(&format!("{s}/"))) {
            continue;
        }
        let arm = meta.arm.clone().unwrap_or_else(|| DEFAULT_ARM.into());
        arms.entry(arm).or_default().push((task, card));
    }
    arms
}

/// Compare arms: mean headline metrics per arm, per-task pass rates, and
/// regressions of every arm against the first. Returns the report and
/// the number of regressions.
///
/// # Errors
///
/// Fails when a requested arm has no runs or fewer than one arm is found.
pub fn compare(
    runs: &BTreeMap<String, Vec<(String, RunScorecard)>>,
    arms: &[String],
    tolerance: f64,
) -> anyhow::Result<(String, usize)> {
    let arms: Vec<String> = if arms.is_empty() {
        runs.keys().cloned().collect()
    } else {
        arms.to_vec()
    };
    if arms.is_empty() {
        bail!("no recorded eval runs");
    }
    let mut means = Vec::new();
    for arm in &arms {
        let Some(cards) = runs.get(arm) else {
            bail!("no eval runs for arm `{arm}`");
        };
        let cards: Vec<RunScorecard> = cards.iter().map(|(_, c)| c.clone()).collect();
        means.push(scorecard::mean(&cards));
    }

    let mut out = format!("{:<22}", "metric");
    for arm in &arms {
        let _ = write!(out, " {arm:>14}");
    }
    out.push('\n');
    let _ = write!(out, "{:<22}", "runs");
    for arm in &arms {
        let _ = write!(out, " {:>14}", runs[arm].len());
    }
    out.push('\n');
    for metric in means[0].keys() {
        let _ = write!(out, "{metric:<22}");
        for mean in &means {
            let _ = write!(
                out,
                " {:>14.2}",
                mean.get(metric).copied().unwrap_or_default()
            );
        }
        out.push('\n');
    }

    let mut tasks: BTreeMap<&str, Vec<(usize, usize)>> = BTreeMap::new();
    for (i, arm) in arms.iter().enumerate() {
        for (task, card) in &runs[arm] {
            let row = tasks
                .entry(task.as_str())
                .or_insert_with(|| vec![(0, 0); arms.len()]);
            row[i].1 += 1;
            if card.outcome.unwrap_or_default() > 0.0 {
                row[i].0 += 1;
            }
        }
    }
    out.push('\n');
    for (task, row) in &tasks {
        let _ = write!(out, "{task:<22}");
        for (passed, total) in row {
            let _ = write!(out, " {:>14}", format!("{passed}/{total}"));
        }
        out.push('\n');
    }

    let mut regressions = 0;
    for (arm, mean) in arms.iter().zip(&means).skip(1) {
        for delta in scorecard::diff_means(&means[0], mean, tolerance) {
            if delta.regression {
                regressions += 1;
                let _ = writeln!(
                    out,
                    "regression: {arm} {} {:.2} → {:.2} (vs {})",
                    delta.metric, delta.baseline, delta.candidate, arms[0]
                );
            }
        }
    }
    Ok((out, regressions))
}

/// Checked-in reference numbers for one arm of a suite.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Baseline {
    /// Suite the runs came from.
    pub suite: Option<String>,
    /// Arm the numbers describe.
    pub arm: String,
    /// Harness version that produced them.
    pub harness_version: String,
    /// When the baseline was written.
    pub created_at_ms: u64,
    /// Runs averaged.
    pub runs: usize,
    /// Mean headline metrics ([`scorecard::mean`]).
    pub means: BTreeMap<String, f64>,
    /// `task → [passed, total]`.
    pub tasks: BTreeMap<String, [usize; 2]>,
}

impl Baseline {
    /// Baseline from `arm`'s recorded runs.
    ///
    /// # Errors
    ///
    /// Fails when the arm has no runs.
    pub fn from_runs(
        runs: &BTreeMap<String, Vec<(String, RunScorecard)>>,
        arm: &str,
        suite: Option<&str>,
    ) -> anyhow::Result<Self> {
        let Some(rows) = runs.get(arm).filter(|r| !r.is_empty()) else {
            bail!("no eval runs for arm `{arm}`");
        };
        let cards: Vec<RunScorecard> = rows.iter().map(|(_, c)| c.clone()).collect();
        let mut tasks: BTreeMap<String, [usize; 2]> = BTreeMap::new();
        for (task, card) in rows {
            let entry = tasks.entry(task.clone()).or_default();
            entry[1] += 1;
            if card.outcome.unwrap_or_default() > 0.0 {
                entry[0] += 1;
            }
        }
        Ok(Self {
            suite: suite.map(str::to_string),
            arm: arm.to_string(),
            harness_version: aura_agent_trace::bundle::harness_version(),
            created_at_ms: now_ms(),
            runs: rows.len(),
            means: scorecard::mean(&cards),
            tasks,
        })
    }

    /// Read a baseline file.
    ///
    /// # Errors
    ///
    /// Fails when the file can't be read or parsed.
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading baseline {}", path.display()))?;
        serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))
    }

    /// Write the baseline as pretty JSON.
    ///
    /// # Errors
    ///
    /// Fails on an I/O error.
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(self)? + "\n")?;
        Ok(())
    }

    /// Compare each of `arms` (default: all) against this baseline.
    /// Returns the report and the number of regressions.
    ///
    /// # Errors
    ///
    /// Fails when a requested arm has no runs.
    pub fn compare(
        &self,
        runs: &BTreeMap<String, Vec<(String, RunScorecard)>>,
        arms: &[String],
        tolerance: f64,
    ) -> anyhow::Result<(String, usize)> {
        let arms: Vec<String> = if arms.is_empty() {
            runs.keys().cloned().collect()
        } else {
            arms.to_vec()
        };
        let mut out = format!(
            "baseline: arm {} · {} runs · harness {}\n",
            self.arm, self.runs, self.harness_version
        );
        let mut regressions = 0;
        for arm in &arms {
            let current = Self::from_runs(runs, arm, self.suite.as_deref())?;
            let deltas = scorecard::diff_means(&self.means, &current.means, tolerance);
            let _ = writeln!(out, "\n{arm} ({} runs)", current.runs);
            out.push_str(&aura_agent_trace::report::diff_table(&deltas));
            for (task, [passed, total]) in &current.tasks {
                if let Some([base_passed, base_total]) = self.tasks.get(task) {
                    let now = rate(*passed, *total);
                    let before = rate(*base_passed, *base_total);
                    if now + tolerance < before {
                        regressions += 1;
                        let _ =
                            writeln!(out, "regression: {task} pass rate {before:.2} → {now:.2}");
                    }
                }
            }
            for delta in deltas.iter().filter(|d| d.regression) {
                regressions += 1;
                let _ = writeln!(
                    out,
                    "regression: {} {:.2} → {:.2}",
                    delta.metric, delta.baseline, delta.candidate
                );
            }
        }
        Ok((out, regressions))
    }
}

#[allow(clippy::cast_precision_loss)]
fn rate(passed: usize, total: usize) -> f64 {
    if total == 0 {
        0.0
    } else {
        passed as f64 / total as f64
    }
}
