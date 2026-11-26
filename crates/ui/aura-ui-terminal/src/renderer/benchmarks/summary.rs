//! The detail's summary: each arm's numbers side by side, and notes on
//! what they leave out.

use super::detail::{live_of, Detail};
use super::format::{cost_text, duration, fit, money, ratio, saving, short_model};
use super::table::{table, Col, Row};
use crate::events::{cost_per_fix, unpriced_note, BenchArm, BenchBaseline, BenchCost, BenchStatus};
use crate::events::{BenchTokens, BenchView};
use ratatui::text::{Line, Span};

/// `3/4, 1 working`: an arm's runs finished of those planned.
fn finished(view: &BenchView, arm: usize) -> String {
    let done = &view.arms[arm];
    if view.status != BenchStatus::Running {
        return format!("{}/{}", done.finished, done.runs);
    }
    let per_arm = usize::try_from(view.repeats.max(1))
        .unwrap_or(usize::MAX)
        .saturating_mul(view.tasks.len().max(1));
    let planned = per_arm.max(done.runs + live_of(view, arm).count());
    let working = live_of(view, arm).filter(|r| r.wall_ms.is_some()).count();
    let mut text = format!("{}/{planned}", done.finished);
    if working > 0 {
        text.push_str(&format!(", {working} working"));
    }
    text
}

/// What an arm's ended runs and runs under way have cost.
fn spent(view: &BenchView, arm: usize) -> BenchCost {
    let live = live_of(view, arm).map(crate::events::BenchLiveRun::cost);
    live.fold(view.arms[arm].total_cost(), |sum, cost| {
        BenchCost::new(sum.usd + cost.usd, sum.unpriced_calls + cost.unpriced_calls)
    })
}

/// How long an arm's runs have worked together.
fn worked(view: &BenchView, arm: usize) -> String {
    let ended: u64 = view
        .runs
        .iter()
        .filter(|r| r.arm.min(1) == arm)
        .map(|r| r.wall_ms)
        .sum();
    let live: u64 = live_of(view, arm).filter_map(|r| r.wall_ms).sum();
    Some(ended + live)
        .filter(|&ms| ms > 0)
        .map_or_else(|| "-".to_string(), duration)
}

fn none() -> [String; 2] {
    [String::new(), String::new()]
}

impl Detail<'_> {
    fn pair(&self, f: &dyn Fn(&BenchArm) -> String) -> [String; 2] {
        let [a, b] = &self.view.arms;
        [f(a), f(b)]
    }

    fn summary_row(&self, label: &str, [x, y]: [String; 2], [dx, dy]: [String; 2]) -> Row {
        let good = self.good();
        Row::Cells(vec![
            (label.to_string(), self.muted()),
            self.cell(x),
            (dx, good),
            self.cell(y),
            (dy, good),
        ])
    }

    /// The summary table and the notes under it.
    pub(super) fn summary_lines(&self) -> Vec<Line<'static>> {
        let [a, b] = &self.view.arms;
        let mut summary = self.time_and_cost_rows();
        summary.extend(self.count_rows());
        let a_head = format!("A {}", fit(short_model(&a.label), 24));
        let b_head = format!("B {}", fit(short_model(&b.label), 24));
        let mut lines = table(
            &[Col::L, Col::L.sep(), Col::L, Col::L.sep(), Col::L],
            Some(&["", &a_head, "", &b_head, ""]),
            &summary,
            self.width,
            self.theme,
        );
        lines.extend(self.summary_notes());
        lines
    }

    fn time_and_cost_rows(&self) -> Vec<Row> {
        let view = self.view;
        let [a, b] = &view.arms;
        let mut rows = vec![
            self.summary_row("finished", [finished(view, 0), finished(view, 1)], none()),
            self.summary_row(
                "time",
                self.pair(&|arm| arm.median_wall_ms.map_or_else(|| "-".to_string(), duration)),
                none(),
            ),
            self.summary_row("total time", [worked(view, 0), worked(view, 1)], none()),
            self.summary_row(
                "cost",
                [spent(view, 0).read(money), spent(view, 1).read(money)],
                saving(Some(spent(view, 0)), Some(spent(view, 1))),
            ),
            self.summary_row(
                "cost/run",
                self.pair(&|arm| cost_text(arm.median_cost())),
                saving(a.median_cost(), b.median_cost()),
            ),
        ];
        // A ratio against a session some of whose calls had no known price
        // would read as if the session were cheaper than it was.
        let baseline_usd = (view.baseline.as_ref())
            .and_then(BenchBaseline::cost)
            .and_then(BenchCost::known);
        if let Some(baseline_usd) = baseline_usd {
            rows.push(self.summary_row(
                "vs session",
                self.pair(&|arm| ratio(arm, baseline_usd)),
                none(),
            ));
        }
        rows
    }

    fn count_rows(&self) -> Vec<Row> {
        let view = self.view;
        let [a, b] = &view.arms;
        let mut rows = vec![self.summary_row(
            "checks",
            self.pair(&|arm| {
                if arm.checks_run == 0 {
                    "-".to_string()
                } else {
                    format!("{}/{}", arm.checks_passed, arm.checks_run)
                }
            }),
            none(),
        )];
        if a.checks_run + b.checks_run > 0 {
            let per_fix = |arm: &BenchArm| cost_per_fix(arm.total_cost(), arm.checks_passed);
            rows.push(self.summary_row(
                "$/fix",
                self.pair(&|arm| {
                    per_fix(arm).map_or_else(|| "no fix".to_string(), |cost| cost.read(money))
                }),
                saving(per_fix(a), per_fix(b)),
            ));
        }
        rows.extend(self.rare_count_rows());
        rows
    }

    /// Rows only some benchmarks have something for.
    fn rare_count_rows(&self) -> Vec<Row> {
        let view = self.view;
        let [a, b] = &view.arms;
        let mut rows = Vec::new();
        if a.escalated + b.escalated > 0 {
            let moved = self.pair(&|arm| format!("{}/{}", arm.escalated, arm.runs));
            rows.push(self.summary_row("moved up", moved, none()));
        }
        if a.nudged + b.nudged > 0 {
            let nudged = self.pair(&|arm| format!("{}/{}", arm.nudged, arm.runs));
            rows.push(self.summary_row("nudged", nudged, none()));
        }
        if view.judge.is_some() {
            let wlt = self
                .pair(&|arm| format!("{}-{}-{}", arm.judge_wins, arm.judge_losses, arm.judge_ties));
            rows.push(self.summary_row("judge W-L-T", wlt, none()));
        }
        let rated = |arm: &BenchArm| arm.ref_better + arm.ref_as_good + arm.ref_worse;
        if rated(a) + rated(b) > 0 {
            let vs = self
                .pair(&|arm| format!("{}/{}/{}", arm.ref_better, arm.ref_as_good, arm.ref_worse));
            rows.push(self.summary_row("vs yours +/=/-", vs, none()));
        }
        rows
    }

    /// Calls with no known price, the session replayed, and what
    /// `vs yours` means.
    fn summary_notes(&self) -> Vec<Line<'static>> {
        let (view, width, muted) = (self.view, self.width, self.muted());
        let mut lines = Vec::new();
        let unpriced: Vec<BenchTokens> = (0..2)
            .flat_map(|arm| {
                super::format::tokens_so_far(&view.arms[arm].tokens, live_of(view, arm))
            })
            .filter(|t| t.unpriced_calls > 0)
            .collect();
        if let Some(note) = unpriced_note(unpriced.iter().map(|t| short_model(&t.model))) {
            lines.push(Line::from(Span::styled(fit(&note, width), muted)));
        }
        if let Some(baseline) = &view.baseline {
            let models: Vec<&str> = baseline.models.iter().map(|m| short_model(m)).collect();
            let about = format!(
                "session: {} call{}, {}{}",
                baseline.calls,
                if baseline.calls == 1 { "" } else { "s" },
                baseline
                    .cost()
                    .map_or_else(|| "no price".to_string(), |cost| cost.read(money)),
                if models.is_empty() {
                    String::new()
                } else {
                    format!(" on {}", models.join(", "))
                },
            );
            lines.push(Line::from(Span::styled(fit(&about, width), muted)));
        }
        if view.has_reference && view.judge.is_some() {
            lines.push(Line::from(Span::styled(
                fit(
                    "vs yours: the judge rated each run against the diff and reply the session ended with",
                    width,
                ),
                muted,
            )));
        }
        lines
    }
}
