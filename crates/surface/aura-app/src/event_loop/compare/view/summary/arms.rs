//! The arms side by side: what their runs came to, whether the
//! difference is too close to call, and what the replayed session itself
//! cost.

use super::super::format::{duration, money, plural};
use super::Summary;
use aura_gate::compare::{too_close, ArmSummary};
use aura_ui_terminal::events::{cost_per_fix, cost_shares, per_fix_cells};
use std::fmt::Write;

/// Characters a row's label takes, arm A's column after it.
const LABEL_WIDTH: usize = 14;

impl Summary<'_> {
    /// The arms' headings after a blank line, then a row per figure for
    /// each arm.
    pub(super) fn render_arms(&self, out: &mut String) {
        out.push('\n');
        self.cells(out, "", &self.a_label, &self.b_label);
        self.row(out, "finished", |arm| {
            format!("{}/{}", arm.finished, arm.runs)
        });
        self.row(out, "median time", |arm| {
            arm.median_wall_ms.map_or_else(|| "-".to_string(), duration)
        });
        self.cost_row(out, "total cost", |arm| Some(arm.total_cost_usd));
        self.cost_row(out, "median cost", |arm| arm.median_cost_usd);
        self.row(out, "check", |arm| match arm.checks_run {
            0 => "-".to_string(),
            run => format!("{}/{run} passed", arm.checks_passed),
        });
        if self.a.checks_run + self.b.checks_run > 0 {
            let per_fix = |arm: &ArmSummary| cost_per_fix(arm.total_cost_usd, arm.checks_passed);
            let [a, b] = per_fix_cells(per_fix(&self.a), per_fix(&self.b), money);
            self.cells(out, "cost per fix", &a, &b);
        }
        if !self.payload.judgments.is_empty() {
            self.row(out, "judge", |arm| {
                let (won, lost, tied) = (arm.judge_wins, arm.judge_losses, arm.judge_ties);
                format!("{won} won, {lost} lost, {tied} tied")
            });
        }
        let rated = |arm: &ArmSummary| arm.ref_better + arm.ref_as_good + arm.ref_worse;
        if rated(&self.a) + rated(&self.b) > 0 {
            self.row(out, "vs yours", |arm| {
                let (better, as_good, worse) = (arm.ref_better, arm.ref_as_good, arm.ref_worse);
                format!("{better} better, {as_good} as good, {worse} worse")
            });
        }
        self.render_verdicts(out);
    }

    /// Whether the arms are too close to call on checks or on the judge's
    /// wins, and what the replayed session itself cost.
    fn render_verdicts(&self, out: &mut String) {
        let (a, b, payload) = (&self.a, &self.b, self.payload);
        let runs = payload.repeats;
        if a.checks_run > 0 && b.checks_run > 0 && too_close(a.checks_passed, b.checks_passed) {
            let _ = write!(
                out,
                "\n  Too close to call with {runs} runs: check passes are within one of each other."
            );
        }
        if !payload.judgments.is_empty() && too_close(a.judge_wins, b.judge_wins) {
            let _ = write!(
                out,
                "\n  Too close to call with {runs} runs: judge wins are within one of each other."
            );
        }
        let Some(baseline) = payload.source.baseline() else {
            return;
        };
        let calls = baseline.calls;
        let _ = write!(
            out,
            "\n  The session itself: {calls} call{}, ",
            plural(calls)
        );
        if baseline.priced {
            out.push_str(&money(baseline.cost_usd));
        } else {
            out.push_str("no price");
        }
        if !baseline.models.is_empty() {
            let _ = write!(out, " on {}", baseline.models.join(", "));
        }
    }

    /// The row `label` of what `cell` says of each arm.
    fn row(&self, out: &mut String, label: &str, cell: impl Fn(&ArmSummary) -> String) {
        self.cells(out, label, &cell(&self.a), &cell(&self.b));
    }

    /// The row `label` of what each arm's runs `cost`, with each arm's
    /// share of the other's.
    fn cost_row(&self, out: &mut String, label: &str, cost: impl Fn(&ArmSummary) -> Option<f64>) {
        let (a, b) = (cost(&self.a), cost(&self.b));
        let [a_share, b_share] = cost_shares(a, b);
        let cell =
            |usd: Option<f64>, share: &str| usd.map_or_else(|| "-".to_string(), money) + share;
        self.cells(out, label, &cell(a, &a_share), &cell(b, &b_share));
    }

    /// The row `label` reading `a` and `b`, `a` filling arm A's column.
    fn cells(&self, out: &mut String, label: &str, a: &str, b: &str) {
        let width = self.width;
        let _ = write!(out, "\n  {label:<LABEL_WIDTH$}{a:<width$}  {b}");
    }
}
