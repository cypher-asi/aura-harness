//! Comparing recorded eval runs across arms and against a [`Baseline`].

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use anyhow::{bail, Context};
use aura_agent_trace::bundle::{list_runs, now_ms};
use aura_agent_trace::{scorecard, RunScorecard, RunSource};
use serde::{Deserialize, Serialize};

use super::DEFAULT_ARM;

/// Recorded eval runs: `arm → [(task, scorecard)]`.
type RunsByArm = BTreeMap<String, Vec<(String, RunScorecard)>>;
/// Mean headline metrics ([`scorecard::mean`]).
type Means = BTreeMap<String, f64>;

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
    let arms = chosen_arms(runs, arms);
    if arms.is_empty() {
        bail!("no recorded eval runs");
    }
    let means = arm_means(runs, &arms)?;
    let mut out = String::new();
    write_metrics(&mut out, runs, &arms, &means);
    out.push('\n');
    write_task_pass_rates(&mut out, runs, &arms);
    let regressions = write_regressions(&mut out, &arms, &means, tolerance);
    Ok((out, regressions))
}

/// `arms`, or every arm with runs when none are named.
fn chosen_arms(runs: &RunsByArm, arms: &[String]) -> Vec<String> {
    if arms.is_empty() {
        runs.keys().cloned().collect()
    } else {
        arms.to_vec()
    }
}

/// Mean headline metrics of each of `arms`.
fn arm_means(runs: &RunsByArm, arms: &[String]) -> anyhow::Result<Vec<Means>> {
    let mut means = Vec::new();
    for arm in arms {
        let Some(cards) = runs.get(arm) else {
            bail!("no eval runs for arm `{arm}`");
        };
        let cards: Vec<RunScorecard> = cards.iter().map(|(_, c)| c.clone()).collect();
        means.push(scorecard::mean(&cards));
    }
    Ok(means)
}

/// Header, run counts and one row per mean metric, one column per arm.
fn write_metrics(out: &mut String, runs: &RunsByArm, arms: &[String], means: &[Means]) {
    let _ = write!(out, "{:<22}", "metric");
    for arm in arms {
        let _ = write!(out, " {arm:>14}");
    }
    out.push('\n');
    let _ = write!(out, "{:<22}", "runs");
    for arm in arms {
        let _ = write!(out, " {:>14}", runs[arm].len());
    }
    out.push('\n');
    for metric in means[0].keys() {
        let _ = write!(out, "{metric:<22}");
        for mean in means {
            let _ = write!(
                out,
                " {:>14.2}",
                mean.get(metric).copied().unwrap_or_default()
            );
        }
        out.push('\n');
    }
}

/// `passed/total` of each task, one column per arm.
fn write_task_pass_rates(out: &mut String, runs: &RunsByArm, arms: &[String]) {
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
    for (task, row) in &tasks {
        let _ = write!(out, "{task:<22}");
        for (passed, total) in row {
            let _ = write!(out, " {:>14}", format!("{passed}/{total}"));
        }
        out.push('\n');
    }
}

/// One line per metric of an arm that regressed against the first arm;
/// returns how many there were.
fn write_regressions(out: &mut String, arms: &[String], means: &[Means], tolerance: f64) -> usize {
    let mut regressions = 0;
    for (arm, mean) in arms.iter().zip(means).skip(1) {
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
    regressions
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
        let arms = chosen_arms(runs, arms);
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
