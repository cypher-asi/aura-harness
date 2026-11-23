//! What each arm used: tokens per model, the share of each plan window it
//! drew, and what the judge's blind calls cost.

use super::super::format::{money, plural, tokens};
use super::Summary;
use aura_core_types::{ModelTokens, Plan, PlanShare};
use aura_gate::compare::judge_cost_usd;
use aura_model::model_catalog::route_label;
use std::fmt::Write;

/// Why plan usage is not shown when no call reported it.
const PLAN_USAGE_NOTE: &str = "Plan usage: no call reported a plan's usage (only Claude and \
ChatGPT plans do), so tokens per model (what draws a plan down) are shown instead.";

/// How the plan shares are worked out, shown with them.
const PLAN_SHARE_NOTE: &str = "Plan readings cover the whole account: each rise between \
readings goes to the run whose call saw it, so runs at the same time don't count it twice, \
and use outside the comparison while it ran lands on the next run to read.";

impl Summary<'_> {
    /// Each arm's tokens per model, then its plan usage (or why none is
    /// shown), then the judge's calls.
    pub(super) fn render_usage(&self, out: &mut String) {
        let arms = [("A", &self.a), ("B", &self.b)];
        out.push_str("\n\n  Tokens per model");
        for (arm, summary) in arms {
            let _ = write!(out, "\n    {arm}: ");
            write_model_tokens(out, &summary.tokens);
        }
        if self.a.plan_usage.is_empty() && self.b.plan_usage.is_empty() {
            let _ = write!(out, "\n  {PLAN_USAGE_NOTE}");
        } else {
            out.push_str("\n\n  Plan usage, share of each window's limit");
            for (arm, summary) in arms {
                let _ = write!(out, "\n    {arm}: ");
                write_plan_shares(out, &summary.plan_usage);
            }
            let _ = write!(out, "\n  {PLAN_SHARE_NOTE}");
        }
        self.render_judge(out);
    }

    /// How many blind calls the judge made on which model, what they cost
    /// and how many gave no verdict.
    fn render_judge(&self, out: &mut String) {
        let judgments = &self.payload.judgments;
        let Some(judgment) = judgments.first() else {
            return;
        };
        let _ = write!(
            out,
            "\n  Judge: {} blind call{} on {}, {} (not in the arm totals)",
            judgments.len(),
            plural(judgments.len()),
            route_label(&judgment.model),
            money(judge_cost_usd(self.payload)),
        );
        let unparsed = judgments.iter().filter(|j| !j.parsed).count();
        if unparsed > 0 {
            let _ = write!(out, "; {unparsed} gave no verdict");
        }
    }
}

/// `claude-sonnet-4-6 9 calls, 126k in / 5k out, $0.18` for each of
/// `rows`, one after another.
fn write_model_tokens(out: &mut String, rows: &[ModelTokens]) {
    if rows.is_empty() {
        out.push_str("no model calls");
        return;
    }
    for (i, row) in rows.iter().enumerate() {
        if i > 0 {
            out.push_str("; ");
        }
        let _ = write!(
            out,
            "{} {} call{}, {} in / {} out, {}",
            row.model,
            row.calls,
            plural(row.calls),
            tokens(row.input_tokens),
            tokens(row.output_tokens),
            money(row.cost_usd),
        );
    }
}

/// `Claude plan 5h window +3.1%, 7d window +0.4%`, one plan after another
/// in the order they first appear in `rows`.
fn write_plan_shares(out: &mut String, rows: &[PlanShare]) {
    if rows.is_empty() {
        out.push_str("no plan usage reported");
        return;
    }
    let mut plans: Vec<Plan> = Vec::new();
    for row in rows {
        if !plans.contains(&row.plan) {
            plans.push(row.plan);
        }
    }
    for (i, plan) in plans.into_iter().enumerate() {
        if i > 0 {
            out.push_str("; ");
        }
        let _ = write!(out, "{} ", plan.label());
        for (j, row) in rows.iter().filter(|row| row.plan == plan).enumerate() {
            if j > 0 {
                out.push_str(", ");
            }
            let window = row.window.replace('_', " ");
            let _ = write!(out, "{window} window +{:.1}%", row.used * 100.0);
        }
    }
}
