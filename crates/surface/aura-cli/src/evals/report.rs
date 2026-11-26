//! What a suite run prints: progress lines, the results table and costs.

use std::fmt::Write as _;

use super::{EvalResult, GateUse};

/// `cost_usd` to four places, `+?` after it when `unpriced` calls had no
/// known price, or `unknown` when nothing priced cost anything.
pub(super) fn amount(cost_usd: f64, unpriced: u32) -> String {
    match unpriced {
        0 => format!("{cost_usd:.4}"),
        _ if cost_usd > 0.0 => format!("{cost_usd:.4}+?"),
        _ => "unknown".to_string(),
    }
}

/// [`amount`] in dollars.
pub(super) fn dollars(cost_usd: f64, unpriced: u32) -> String {
    match unpriced {
        0 => format!("${cost_usd:.4}"),
        _ if cost_usd > 0.0 => format!("${cost_usd:.4}+?"),
        _ => "unknown cost".to_string(),
    }
}

/// One finished run of a suite: its place, verdict, cost and time, what
/// the suite has cost so far and, under Gate, the models it was routed
/// to.
#[allow(clippy::cast_precision_loss)]
pub(super) fn progress_line(n: usize, total: usize, result: &EvalResult, spent: f64) -> String {
    let routed = result
        .gate
        .as_ref()
        .filter(|gate| !gate.routed.is_empty())
        .map(|gate| format!("  on {}", gate.routed.join(", ")))
        .unwrap_or_default();
    let judged = result
        .gate
        .as_ref()
        .filter(|gate| !gate.judged.is_empty())
        .map(|gate| format!("  {}", judged_note(gate)))
        .unwrap_or_default();
    format!(
        "[{n}/{total}] {} {} {} {:.1}s (${spent:.4} so far){routed}{judged}{}",
        result.task,
        if result.passed { "pass" } else { "FAIL" },
        dollars(result.cost_usd(), result.unpriced_calls()),
        result.scorecard.wall_ms as f64 / 1000.0,
        result
            .error
            .as_deref()
            .map(|e| format!("  ({e})"))
            .unwrap_or_default(),
    )
}

/// What Gate's judge found on a run's failed turns and what asking it
/// cost, which is part of the run's cost.
fn judged_note(gate: &GateUse) -> String {
    let label = |id: &str| aura_model::model_catalog::route_label(id);
    let found: Vec<String> = gate
        .judged
        .iter()
        .map(|judged| {
            format!(
                "{} {}",
                label(&judged.model),
                if judged.verdict.at_fault {
                    "at fault"
                } else {
                    "excused"
                }
            )
        })
        .collect();
    let cost = judge_cost(gate.judge_cost_usd(), gate.judge_unpriced_calls());
    format!("judge: {} ({cost} of the cost)", found.join(", "))
}

/// What the judge's calls cost: `$0.0123`; `$0.0123+?` when `unpriced`
/// of them had no known price, and `an unknown amount` when none had one.
pub(super) fn judge_cost(cost_usd: f64, unpriced: usize) -> String {
    match unpriced {
        0 => format!("${cost_usd:.4}"),
        _ if cost_usd > 0.0 => format!("${cost_usd:.4}+?"),
        _ => "an unknown amount".to_string(),
    }
}

/// Table of suite results.
#[must_use]
pub fn results_table(results: &[EvalResult]) -> String {
    let mut out = format!(
        "{:<24} {:<6} {:>9} {:>6} {:>9} {:>8}  {}\n",
        "task", "result", "tokens", "calls", "usd", "secs", "run"
    );
    for r in results {
        write_result_row(&mut out, r);
        if let Some(gate) = &r.gate {
            write_gate_rows(&mut out, gate);
        }
    }
    write_totals(&mut out, results);
    write_judge_summary(&mut out, results);
    out
}

/// One run's row of the results table.
#[allow(clippy::cast_precision_loss)]
fn write_result_row(out: &mut String, r: &EvalResult) {
    let _ = writeln!(
        out,
        "{:<24} {:<6} {:>9} {:>6} {:>9} {:>8.1}  {}{}",
        r.task,
        if r.passed { "pass" } else { "FAIL" },
        r.scorecard.total_tokens(),
        r.scorecard.model_calls,
        amount(r.cost_usd(), r.unpriced_calls()),
        r.scorecard.wall_ms as f64 / 1000.0,
        r.run_id,
        r.error
            .as_deref()
            .map(|e| format!("  ({e})"))
            .unwrap_or_default(),
    );
}

/// Where Gate routed a run and what its judge found.
fn write_gate_rows(out: &mut String, gate: &GateUse) {
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
    if !gate.judged.is_empty() {
        let _ = writeln!(out, "  {}", judged_note(gate));
    }
}

/// How many runs passed and what they cost.
fn write_totals(out: &mut String, results: &[EvalResult]) {
    let passed = results.iter().filter(|r| r.passed).count();
    let cost: f64 = results.iter().map(EvalResult::cost_usd).sum();
    let unpriced: u32 = results.iter().map(EvalResult::unpriced_calls).sum();
    let _ = writeln!(
        out,
        "{passed}/{} passed, {}",
        results.len(),
        dollars(cost, unpriced)
    );
    if unpriced > 0 {
        let _ = writeln!(
            out,
            "+? {unpriced} model call(s) have no known list price and are not in the cost"
        );
    }
}

/// What Gate's judge reviewed across the suite and what that cost.
fn write_judge_summary(out: &mut String, results: &[EvalResult]) {
    let judged: Vec<&aura_gate::live::Judged> = results
        .iter()
        .filter_map(|r| r.gate.as_ref())
        .flat_map(|gate| &gate.judged)
        .collect();
    if !judged.is_empty() {
        let at_fault = judged.iter().filter(|j| j.verdict.at_fault).count();
        let cost: f64 = judged.iter().filter_map(|j| j.cost_usd).sum();
        let unpriced = judged.iter().filter(|j| j.cost_usd.is_none()).count();
        let cost = judge_cost(cost, unpriced);
        let _ = writeln!(
            out,
            "judge: reviewed {} failed turn(s), {at_fault} the model's fault, {cost} of the cost spent on learning from them",
            judged.len()
        );
    }
}
