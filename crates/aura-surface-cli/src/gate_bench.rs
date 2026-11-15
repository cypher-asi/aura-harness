//! Paired cost and quality report of recorded eval arms: does an arm
//! (usually the gate) cost less than a reference arm (usually the
//! frontier model alone) without passing fewer tasks?
//!
//! Arms are paired by task. The pass-rate difference gets a 95%
//! interval from resampling tasks, and an arm counts as non-inferior
//! when the interval's lower end stays above `-margin`. Given a cheap
//! single-model arm, the report also prices a cost-matched mixture: the
//! pass rate of sending each task to the reference or the cheap model at
//! random, in the proportion that costs what the candidate costs. An
//! arm that does not beat it is no better than using the cheap model
//! more often.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use anyhow::bail;
use aura_agent_trace::RunScorecard;

/// Resamples behind each interval.
const RESAMPLES: usize = 2000;

/// Recorded runs by arm: `(task id, scorecard)`.
pub type ArmRuns = BTreeMap<String, Vec<(String, RunScorecard)>>;

/// Per-task means of one arm.
#[derive(Debug, Clone, Copy, Default)]
struct Cell {
    passed: f64,
    runs: usize,
    cost: f64,
    workers: f64,
    wall_ms: f64,
}

impl Cell {
    #[allow(clippy::cast_precision_loss)]
    fn mean(&self, total: f64) -> f64 {
        if self.runs == 0 {
            0.0
        } else {
            total / self.runs as f64
        }
    }
    fn pass(&self) -> f64 {
        self.mean(self.passed)
    }
    fn usd(&self) -> f64 {
        self.mean(self.cost)
    }
}

fn cells(rows: &[(String, RunScorecard)]) -> BTreeMap<String, Cell> {
    let mut out: BTreeMap<String, Cell> = BTreeMap::new();
    for (task, card) in rows {
        let cell = out.entry(task.clone()).or_default();
        cell.runs += 1;
        if card.outcome.unwrap_or_default() > 0.0 {
            cell.passed += 1.0;
        }
        cell.cost += card.total_cost_usd();
        cell.workers += card.child_cost_usd;
        #[allow(clippy::cast_precision_loss)]
        {
            cell.wall_ms += card.wall_ms as f64;
        }
    }
    out
}

/// Task-averaged means of `cells` over `tasks`.
#[derive(Debug, Clone, Copy, Default)]
struct Means {
    pass: f64,
    usd: f64,
    workers: f64,
    secs: f64,
}

#[allow(clippy::cast_precision_loss)]
fn means(cells: &BTreeMap<String, Cell>, tasks: &BTreeSet<String>) -> Means {
    let present: Vec<&Cell> = tasks.iter().filter_map(|t| cells.get(t)).collect();
    if present.is_empty() {
        return Means::default();
    }
    let n = present.len() as f64;
    Means {
        pass: present.iter().map(|c| c.pass()).sum::<f64>() / n,
        usd: present.iter().map(|c| c.usd()).sum::<f64>() / n,
        workers: present.iter().map(|c| c.mean(c.workers)).sum::<f64>() / n,
        secs: present.iter().map(|c| c.mean(c.wall_ms)).sum::<f64>() / n / 1000.0,
    }
}

/// Mean and 95% interval of `diffs`, resampling with a fixed seed.
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
fn interval(diffs: &[f64]) -> (f64, f64, f64) {
    if diffs.is_empty() {
        return (0.0, 0.0, 0.0);
    }
    let n = diffs.len();
    let mean = diffs.iter().sum::<f64>() / n as f64;
    let mut state: u64 = 0x2545_F491_4F6C_DD1D;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state % n as u64) as usize
    };
    let mut samples: Vec<f64> = (0..RESAMPLES)
        .map(|_| (0..n).map(|_| diffs[next()]).sum::<f64>() / n as f64)
        .collect();
    samples.sort_by(f64::total_cmp);
    let at = |q: f64| samples[((RESAMPLES - 1) as f64 * q).round() as usize];
    (mean, at(0.025), at(0.975))
}

/// What the report concluded about one candidate arm.
#[derive(Debug, Clone, PartialEq)]
pub struct Verdict {
    pub arm: String,
    /// Tasks run by both this arm and the reference.
    pub tasks: usize,
    /// Mean pass-rate difference to the reference.
    pub delta: f64,
    /// Lower end of the 95% interval of `delta`.
    pub delta_low: f64,
    /// Mean cost per run over the reference's.
    pub cost_ratio: f64,
    /// `delta_low >= -margin`.
    pub non_inferior: bool,
    /// Whether it passes at least as many tasks as the cost-matched
    /// mixture, when a cheap arm was given.
    pub beats_mixture: Option<bool>,
    /// Categories where the arm is not shown non-inferior.
    pub weak_categories: Vec<String>,
}

impl Verdict {
    /// Cheaper than the reference and non-inferior, beating the mixture
    /// when one was priced.
    #[must_use]
    pub fn wins(&self) -> bool {
        self.non_inferior && self.cost_ratio < 1.0 && self.beats_mixture != Some(false)
    }
}

/// Report comparing `candidates` with `reference` over the tasks both
/// ran, overall and by category (`categories` maps task id to category;
/// unmapped tasks are `general`). `cheap` names the single cheap-model
/// arm the mixture is priced from.
///
/// # Errors
/// Fails when an arm has no runs.
pub fn report(
    runs: &ArmRuns,
    categories: &BTreeMap<String, String>,
    reference: &str,
    candidates: &[String],
    cheap: Option<&str>,
    margin: f64,
) -> anyhow::Result<(String, Vec<Verdict>)> {
    let arm_cells = |arm: &str| -> anyhow::Result<BTreeMap<String, Cell>> {
        match runs.get(arm) {
            Some(rows) if !rows.is_empty() => Ok(cells(rows)),
            _ => bail!("no eval runs for arm `{arm}`"),
        }
    };
    let reference_cells = arm_cells(reference)?;
    let cheap_cells = cheap.map(arm_cells).transpose()?;
    let category_of = |task: &str| {
        categories
            .get(task)
            .cloned()
            .unwrap_or_else(|| crate::evals::DEFAULT_CATEGORY.to_string())
    };

    let mut out = format!(
        "{:<16} {:>6} {:>6} {:>10} {:>10} {:>8} {:>8}\n",
        "arm", "tasks", "pass", "usd/run", "workers", "secs", "cost×"
    );
    let all_reference: BTreeSet<String> = reference_cells.keys().cloned().collect();
    let base = means(&reference_cells, &all_reference);
    let row = |out: &mut String, name: &str, m: Means, tasks: usize| {
        let ratio = if base.usd > 0.0 {
            m.usd / base.usd
        } else {
            0.0
        };
        let _ = writeln!(
            out,
            "{name:<16} {tasks:>6} {:>6.2} {:>10.4} {:>10.4} {:>8.1} {ratio:>8.2}",
            m.pass, m.usd, m.workers, m.secs
        );
    };
    row(&mut out, reference, base, all_reference.len());
    if let (Some(name), Some(cells)) = (cheap, &cheap_cells) {
        let tasks: BTreeSet<String> = cells.keys().cloned().collect();
        row(&mut out, name, means(cells, &tasks), tasks.len());
    }

    let mut verdicts = Vec::new();
    let mut paired = String::new();
    for arm in candidates {
        let candidate = arm_cells(arm)?;
        let shared: BTreeSet<String> = candidate
            .keys()
            .filter(|t| reference_cells.contains_key(*t))
            .cloned()
            .collect();
        let c = means(&candidate, &shared);
        let r = means(&reference_cells, &shared);
        row(&mut out, arm, c, shared.len());

        let mixture = cheap_cells.as_ref().and_then(|cheap| {
            let shared: BTreeSet<String> = shared
                .iter()
                .filter(|t| cheap.contains_key(*t))
                .cloned()
                .collect();
            let (r, h, c) = (
                means(&reference_cells, &shared),
                means(cheap, &shared),
                means(&candidate, &shared),
            );
            (r.usd > h.usd).then(|| {
                let lambda = ((c.usd - h.usd) / (r.usd - h.usd)).clamp(0.0, 1.0);
                (lambda, lambda * r.pass + (1.0 - lambda) * h.pass, c.pass)
            })
        });
        if let Some((lambda, pass, _)) = mixture {
            let _ = writeln!(
                out,
                "{:<16} {:>6} {pass:>6.2} {:>10.4}   ({:.0}% {reference}, rest {})",
                format!("mixture@{arm}"),
                shared.len(),
                c.usd,
                lambda * 100.0,
                cheap.unwrap_or_default()
            );
        }

        let diffs: Vec<f64> = shared
            .iter()
            .map(|t| candidate[t].pass() - reference_cells[t].pass())
            .collect();
        let (delta, low, high) = interval(&diffs);
        let mut weak = Vec::new();
        let mut by_category: BTreeMap<String, Vec<&String>> = BTreeMap::new();
        for task in &shared {
            by_category.entry(category_of(task)).or_default().push(task);
        }
        let _ = writeln!(
            paired,
            "\n{arm} vs {reference}: {} paired tasks, Δpass {delta:+.3} (95% {low:+.3} .. {high:+.3}), \
             margin {margin:.2}: {}",
            shared.len(),
            if low >= -margin { "non-inferior" } else { "not shown non-inferior" }
        );
        let _ = writeln!(
            paired,
            "  {:<20} {:>5} {:>8} {:>8} {:>8} {:>10} {:>10}",
            "category", "tasks", reference, arm, "Δ", "usd ref", "usd arm"
        );
        for (category, tasks) in &by_category {
            let set: BTreeSet<String> = tasks.iter().map(|t| (*t).clone()).collect();
            let (cr, cc) = (means(&reference_cells, &set), means(&candidate, &set));
            let diffs: Vec<f64> = set
                .iter()
                .map(|t| candidate[t].pass() - reference_cells[t].pass())
                .collect();
            let (d, low, _) = interval(&diffs);
            if low < -margin {
                weak.push(category.clone());
            }
            let _ = writeln!(
                paired,
                "  {category:<20} {:>5} {:>8.2} {:>8.2} {d:>+8.2} {:>10.4} {:>10.4}",
                set.len(),
                cr.pass,
                cc.pass,
                cr.usd,
                cc.usd
            );
        }
        verdicts.push(Verdict {
            arm: arm.clone(),
            tasks: shared.len(),
            delta,
            delta_low: low,
            cost_ratio: if r.usd > 0.0 { c.usd / r.usd } else { 0.0 },
            non_inferior: low >= -margin,
            beats_mixture: mixture.map(|(_, mix, pass)| pass >= mix),
            weak_categories: weak,
        });
    }
    out.push_str(&paired);
    Ok((out, verdicts))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(passed: bool, usd: f64) -> RunScorecard {
        RunScorecard {
            outcome: Some(if passed { 1.0 } else { 0.0 }),
            cost_usd: usd,
            ..RunScorecard::default()
        }
    }

    fn arm(rows: &[(&str, bool, f64)]) -> Vec<(String, RunScorecard)> {
        rows.iter()
            .map(|(t, p, c)| ((*t).to_string(), card(*p, *c)))
            .collect()
    }

    #[test]
    fn cheaper_arm_with_equal_passes_wins_and_beats_the_mixture() {
        let tasks = ["s/a", "s/b", "s/c", "s/d"];
        let mut runs = ArmRuns::new();
        runs.insert("opus".into(), arm(&tasks.map(|t| (t, true, 1.0))));
        runs.insert(
            "haiku".into(),
            arm(&[
                ("s/a", true, 0.05),
                ("s/b", false, 0.05),
                ("s/c", false, 0.05),
                ("s/d", true, 0.05),
            ]),
        );
        runs.insert("gate".into(), arm(&tasks.map(|t| (t, true, 0.3))));
        let categories: BTreeMap<String, String> = [
            ("s/a", "bugfix"),
            ("s/b", "bugfix"),
            ("s/c", "docs"),
            ("s/d", "docs"),
        ]
        .into_iter()
        .map(|(t, c)| (t.to_string(), c.to_string()))
        .collect();
        let (text, verdicts) = report(
            &runs,
            &categories,
            "opus",
            &["gate".into()],
            Some("haiku"),
            0.1,
        )
        .unwrap();
        let v = &verdicts[0];
        assert_eq!(v.tasks, 4);
        assert!((v.cost_ratio - 0.3).abs() < 1e-9, "{text}");
        assert!(
            v.non_inferior && v.beats_mixture == Some(true) && v.wins(),
            "{text}"
        );
        assert!(text.contains("mixture@gate"), "{text}");
        assert!(text.contains("bugfix") && text.contains("docs"), "{text}");
    }

    #[test]
    fn an_arm_that_drops_tasks_is_not_non_inferior() {
        let tasks = ["s/a", "s/b", "s/c", "s/d"];
        let mut runs = ArmRuns::new();
        runs.insert("opus".into(), arm(&tasks.map(|t| (t, true, 1.0))));
        runs.insert(
            "gate".into(),
            arm(&[
                ("s/a", true, 0.2),
                ("s/b", false, 0.2),
                ("s/c", false, 0.2),
                ("s/d", true, 0.2),
            ]),
        );
        let (_, verdicts) =
            report(&runs, &BTreeMap::new(), "opus", &["gate".into()], None, 0.1).unwrap();
        assert!(!verdicts[0].non_inferior);
        assert!(!verdicts[0].wins());
        assert_eq!(verdicts[0].weak_categories, ["general"]);
        assert!(report(&runs, &BTreeMap::new(), "missing", &[], None, 0.1).is_err());
    }
}
