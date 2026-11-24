//! One line per run, how its turns were routed, the comparison's notes,
//! and what can be done with its copies.

use super::super::format::{cost, duration, headline, plural};
use super::Summary;
use aura_core_types::{CompareRun, CompareStatus, RunTurn};
use aura_model::model_catalog::route_label;
use std::fmt::Write;

impl Summary<'_> {
    /// A line per run, then the notes, then the commands that act on the
    /// copies while they are kept.
    pub(super) fn render_runs(&self, out: &mut String) {
        let payload = self.payload;
        out.push_str("\n\n  Runs");
        for run in &payload.runs {
            write_run(out, run);
        }
        for note in &payload.notes {
            let _ = write!(out, "\n  Note: {note}");
        }
        if payload.status.has_copies() && payload.status != CompareStatus::Running {
            out.push_str(if payload.source.examples().is_empty() {
                "\n\n  /compare diff [run] · /compare apply <run> · /compare dismiss"
            } else {
                "\n\n  /compare diff [run] · /compare dismiss"
            });
        }
    }
}

/// The run's name, model, time, cost, check and files changed on a line
/// of its own, then its task, how its turns were routed and its error
/// when it has them.
fn write_run(out: &mut String, run: &CompareRun) {
    let check = match run.check_passed {
        Some(true) => "check passed",
        Some(false) => "check failed",
        None => "",
    };
    let files = run.files_changed;
    let _ = write!(
        out,
        "\n    {:<8} {:<34} {:>8}  {:>8}  {check:<12}  {files} file{}",
        run.name,
        route_label(&run.model),
        duration(run.wall_ms),
        cost(run.cost_usd, run.unpriced_calls()),
        plural(files),
    );
    if let Some(task) = &run.task {
        let _ = write!(out, "  {task}");
    }
    write_routing(out, run);
    if let Some(error) = &run.error {
        let _ = write!(out, "  ({})", headline(error, 80));
    }
}

/// How a run's turns went: what they asked for and how hard, whether
/// they were led (and then what the lead and its subtasks cost), and how
/// often Gate asked the model to check its change, sent the work back and
/// moved a failed attempt up a tier, e.g. `  · debugging/hard, direct,
/// moved up 1×`. Nothing when its turns do not say.
fn write_routing(out: &mut String, run: &CompareRun) {
    let routed: Vec<&RunTurn> = run.turns.iter().filter(|t| t.category.is_some()).collect();
    if routed.is_empty() {
        return;
    }
    out.push_str("  · ");
    write_asked(out, &routed);
    match routed.iter().filter(|t| t.led == Some(true)).count() {
        0 => out.push_str(", direct"),
        led if led == routed.len() => out.push_str(", led"),
        led => {
            let _ = write!(out, ", {led} of {} turns led", routed.len());
        }
    }
    let subtasks: f64 = routed.iter().map(|t| t.subtask_cost_usd).sum();
    if subtasks > 0.0 {
        let total: f64 = routed.iter().map(|t| t.cost_usd).sum();
        // Which part the unpriced calls were in is not kept, so both
        // parts are marked.
        let unpriced: u32 = routed.iter().map(|t| t.unpriced_calls).sum();
        let lead = cost((total - subtasks).max(0.0), unpriced);
        let _ = write!(out, ": lead {lead} + subtasks {}", cost(subtasks, unpriced));
    }
    for (times, what) in [
        (
            routed.iter().map(|t| t.nudges).sum::<u32>(),
            "asked to check",
        ),
        (routed.iter().map(|t| t.sent_back).sum(), "sent back"),
        (routed.iter().map(|t| t.escalations).sum(), "moved up"),
    ] {
        if times > 0 {
            let _ = write!(out, ", {what} {times}×");
        }
    }
}

/// What the `routed` turns asked for and how hard, each once in the order
/// they first asked, with what the classifier found drove it when it
/// said, e.g. `debugging/hard (many files, loose spec, deep exploring),
/// question`.
fn write_asked(out: &mut String, routed: &[&RunTurn]) {
    let mut asked = Vec::new();
    for turn in routed {
        let Some(category) = turn.category else {
            continue;
        };
        if asked.contains(&(category, turn.difficulty)) {
            continue;
        }
        if !asked.is_empty() {
            out.push_str(", ");
        }
        asked.push((category, turn.difficulty));
        let _ = write!(out, "{category}");
        if let Some(difficulty) = turn.difficulty {
            let _ = write!(out, "/{difficulty}");
        }
        if let Some(assessment) = &turn.assessment {
            let _ = write!(out, " ({})", assessment.summary());
        }
    }
}
