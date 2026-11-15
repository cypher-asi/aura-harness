//! Plain-text reports shared by the CLI and the TUI.

use std::fmt::Write as _;

use aura_core_types::trace::{Actor, HarnessAction, Step, StepAction};

use crate::bundle::RunMeta;
use crate::scorecard::{MetricDelta, RunScorecard};

fn pct(v: f64) -> String {
    format!("{:.0}%", v * 100.0)
}

fn secs(ms: u64) -> String {
    #[allow(clippy::cast_precision_loss)]
    let s = ms as f64 / 1000.0;
    if s >= 60.0 {
        format!("{:.1}m", s / 60.0)
    } else {
        format!("{s:.1}s")
    }
}

fn tokens(n: u64) -> String {
    #[allow(clippy::cast_precision_loss)]
    let f = n as f64;
    if n >= 1_000_000 {
        format!("{:.1}M", f / 1e6)
    } else if n >= 1_000 {
        format!("{:.1}k", f / 1e3)
    } else {
        n.to_string()
    }
}

fn short(id: &str) -> &str {
    id.get(..13).unwrap_or(id)
}

/// One line per run, newest first.
#[must_use]
pub fn runs_table(runs: &[(RunMeta, Option<RunScorecard>)]) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{:<13}  {:<7}  {:<22}  {:>5}  {:>5}  {:>7}  {:>7}  {:>7}  title",
        "run", "source", "model", "turns", "calls", "tokens", "wall", "outcome"
    );
    for (meta, card) in runs {
        let (turns, calls, toks, wall) = card.as_ref().map_or_else(
            || ("-".into(), "-".into(), "-".into(), "-".into()),
            |c| {
                (
                    c.turns.to_string(),
                    c.model_calls.to_string(),
                    tokens(c.total_tokens()),
                    secs(c.wall_ms),
                )
            },
        );
        let outcome = card
            .as_ref()
            .and_then(|c| c.outcome)
            .map_or_else(|| "-".into(), |o| format!("{o:+.2}"));
        let source = serde_json::to_value(meta.source)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default();
        let _ = writeln!(
            out,
            "{:<13}  {:<7}  {:<22}  {:>5}  {:>5}  {:>7}  {:>7}  {:>7}  {}",
            short(&meta.run_id),
            source,
            meta.model.chars().take(22).collect::<String>(),
            turns,
            calls,
            toks,
            wall,
            outcome,
            meta.title
        );
    }
    out
}

/// Multi-line scorecard.
#[must_use]
pub fn scorecard_text(card: &RunScorecard) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "run       {}  ({})", card.run_id, card.model);
    let _ = writeln!(
        out,
        "work      {} turns, {} model calls, {} tool calls ({} errors), wall {}, model {}",
        card.turns,
        card.model_calls,
        card.tool_calls,
        card.tool_errors,
        secs(card.wall_ms),
        secs(card.model_ms)
    );
    let _ = writeln!(
        out,
        "tokens    {} in, {} out, cache read {} ({}), cache write {}",
        tokens(card.input_tokens),
        tokens(card.output_tokens),
        tokens(card.cache_read_tokens),
        pct(card.cache_ratio()),
        tokens(card.cache_write_tokens)
    );
    let first_edit = match (card.ms_to_first_edit, card.tokens_to_first_edit) {
        (Some(ms), Some(t)) => format!("{} / {} tokens", secs(ms), tokens(t)),
        _ => "no edit".into(),
    };
    let _ = writeln!(
        out,
        "coding    {} reads, {} writes (r/w {}), {} redundant reads, {} failed edits, first edit {}",
        card.reads,
        card.writes,
        card.read_write_ratio()
            .map_or_else(|| "-".into(), |r| format!("{r:.1}")),
        card.redundant_reads,
        card.failed_edits,
        first_edit
    );
    let _ = writeln!(
        out,
        "harness   {} blocked, {} cache hits, {} retries, {} files changed",
        card.blocked_calls, card.cache_hits, card.retries, card.files_changed
    );
    if !card.decisions.is_empty() {
        let list: Vec<String> = card
            .decisions
            .iter()
            .map(|(k, v)| format!("{k}×{v}"))
            .collect();
        let _ = writeln!(out, "decisions {}", list.join(", "));
    }
    if !card.compactions.is_empty() {
        let list: Vec<String> = card
            .compactions
            .iter()
            .map(|(k, v)| format!("{k}×{v}"))
            .collect();
        let _ = writeln!(out, "compacted {}", list.join(", "));
    }
    let measured: Vec<String> = card
        .effects
        .iter()
        .filter_map(|(k, e)| {
            e.rate()
                .map(|r| format!("{k} {} ({}/{})", pct(r), e.effective, e.measured))
        })
        .collect();
    if !measured.is_empty() {
        let _ = writeln!(out, "effect    {}", measured.join(", "));
    }
    if !card.stop_reasons.is_empty() {
        let _ = writeln!(out, "stops     {}", card.stop_reasons.join(", "));
    }
    if !card.reward_totals.is_empty() {
        let list: Vec<String> = card
            .reward_totals
            .iter()
            .map(|(k, v)| format!("{k} {v:+.2}"))
            .collect();
        let _ = writeln!(out, "rewards   {}", list.join(", "));
    }
    let _ = writeln!(
        out,
        "outcome   {}",
        card.outcome
            .map_or_else(|| "unlabeled".into(), |o| format!("{o:+.2}"))
    );
    out
}

/// One-line summary of a step.
#[must_use]
pub fn step_line(step: &Step) -> String {
    let what = match &step.action {
        StepAction::UserPrompt { text } => {
            format!("user: {}", text.lines().next().unwrap_or_default())
        }
        StepAction::Model(a) => {
            let calls: Vec<&str> = a.tool_calls.iter().map(|c| c.name.as_str()).collect();
            let text = a.text.lines().next().unwrap_or_default();
            if calls.is_empty() {
                format!("model: {}", text.chars().take(80).collect::<String>())
            } else {
                format!("model → {}", calls.join(", "))
            }
        }
        StepAction::Harness { name, action, .. } => {
            let source = match step.actor {
                Actor::Harness { source } => source.as_str(),
                _ => "harness",
            };
            let detail = match action {
                HarnessAction::Block { reason, .. } | HarnessAction::Warn { message: reason } => {
                    format!(": {}", reason.chars().take(60).collect::<String>())
                }
                _ => String::new(),
            };
            format!("{source}/{name} {}{detail}", action.label())
        }
        StepAction::ToolResult {
            tool,
            is_error,
            source,
            excerpt,
            ..
        } => {
            let mark = if *is_error { "✗" } else { "✓" };
            let first = excerpt.lines().next().unwrap_or_default();
            let via = if source == "executor" {
                String::new()
            } else {
                format!(" [{source}]")
            };
            format!(
                "{mark} {tool}{via}: {}",
                first.chars().take(70).collect::<String>()
            )
        }
        StepAction::TurnEnd { stop_reason, .. } => format!("turn end ({stop_reason})"),
    };
    let iter = step
        .iteration
        .map_or_else(|| "  ".into(), |i| format!("{i:>2}"));
    format!("{:>4}  t{} i{iter}  {what}", step.seq, step.turn)
}

/// Metric deltas, regressions marked.
#[must_use]
pub fn diff_table(deltas: &[MetricDelta]) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{:<24} {:>12} {:>12} {:>9}",
        "metric", "baseline", "candidate", "change"
    );
    for d in deltas {
        let change = if d.baseline.abs() > f64::EPSILON {
            format!(
                "{:+.0}%",
                (d.candidate - d.baseline) / d.baseline.abs() * 100.0
            )
        } else if d.candidate.abs() > f64::EPSILON {
            "new".into()
        } else {
            "=".into()
        };
        let mark = if d.regression { "  ← regression" } else { "" };
        let _ = writeln!(
            out,
            "{:<24} {:>12.2} {:>12.2} {:>9}{mark}",
            d.metric, d.baseline, d.candidate, change
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::RunSource;
    use crate::rewards::tests::{model, nudge};
    use crate::RunTrace;

    #[test]
    fn renders_run_reports() {
        let meta = RunMeta::new("model-x", RunSource::Test);
        let trace = RunTrace::from_parts(
            meta.clone(),
            vec![nudge(1, "implement_now"), model(2, &[])],
            Vec::new(),
        );
        let card = trace.scorecard();
        assert!(scorecard_text(&card).contains("model-x"));
        assert!(runs_table(&[(meta, Some(card))]).contains("model-x"));
        assert!(step_line(&trace.steps[0]).contains("steering/implement_now"));
    }
}
