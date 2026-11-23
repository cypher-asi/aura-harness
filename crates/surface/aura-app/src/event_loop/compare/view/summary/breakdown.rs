//! Each arm per example task (checks passed and mean run cost) and per
//! prompt of a replayed session (mean turn cost and how its commands
//! went).

use super::super::format::{cost, headline, plural};
use super::Summary;
use aura_core_types::CompareArm;
use aura_gate::compare::{task_cells, turn_cells, TaskCell, TurnCell};
use aura_ui_terminal::events::cost_shares;
use std::fmt::Write;

/// Characters the task column takes; a task's id is cut two short of it.
const TASK_WIDTH: usize = 34;

/// Characters the prompt number column takes.
const NUMBER_WIDTH: usize = 10;

impl Summary<'_> {
    /// A row per example task, then a row per prompt when there are more
    /// than one.
    pub(super) fn render_breakdown(&self, out: &mut String) {
        self.render_tasks(out);
        self.render_prompts(out);
    }

    /// A row per example task, each arm's cell with its share of the
    /// other's cost.
    fn render_tasks(&self, out: &mut String) {
        let examples = self.payload.source.examples();
        if examples.is_empty() {
            return;
        }
        out.push_str("\n\n  Per task (checks passed; mean run cost)");
        let a_cells = task_cells(self.payload, CompareArm::A);
        let b_cells = task_cells(self.payload, CompareArm::B);
        let width = self.width;
        for (i, task) in examples.iter().enumerate() {
            let (a, b) = (a_cells.get(i), b_cells.get(i));
            let known = |cell: Option<&TaskCell>| {
                cell.filter(|c| c.unpriced_calls == 0)
                    .and_then(|c| c.mean_cost_usd)
            };
            let [a_share, b_share] = cost_shares(known(a), known(b));
            let _ = write!(
                out,
                "\n    {:<TASK_WIDTH$}{:<width$}  {}",
                headline(&task.id, TASK_WIDTH - 2),
                task_cell(a) + &a_share,
                task_cell(b) + &b_share,
            );
        }
    }

    /// A row per prompt of a replayed session of more than one.
    fn render_prompts(&self, out: &mut String) {
        let prompts = &self.payload.prompts;
        if prompts.len() < 2 {
            return;
        }
        out.push_str(
            "\n\n  Per prompt (mean turn cost; test or build commands that passed or failed)",
        );
        let a_cells = turn_cells(self.payload, CompareArm::A);
        let b_cells = turn_cells(self.payload, CompareArm::B);
        let width = self.width;
        for (i, prompt) in prompts.iter().enumerate() {
            let _ = write!(
                out,
                "\n    {:<NUMBER_WIDTH$}{:<width$}  {}  \"{}\"",
                format!("{}.", i + 1),
                turn_cell(a_cells.get(i)),
                turn_cell(b_cells.get(i)),
                headline(prompt, 40),
            );
        }
    }
}

/// `2/3 passed, $0.40` for one arm's runs of one example task, `-` when
/// it has none.
fn task_cell(cell: Option<&TaskCell>) -> String {
    let Some(cell) = cell.filter(|cell| cell.runs > 0) else {
        return "-".to_string();
    };
    let cost = mean_cost(cell.mean_cost_usd, cell.unpriced_calls);
    if cell.checked == 0 {
        return format!("{} run{}, {cost}", cell.runs, plural(cell.runs));
    }
    format!("{}/{} passed, {cost}", cell.passed, cell.checked)
}

/// `$0.05, 2 pass, 1 fail` for one arm's turns on one prompt, `-` when it
/// has none.
fn turn_cell(cell: Option<&TurnCell>) -> String {
    let Some(cell) = cell.filter(|cell| cell.runs > 0) else {
        return "-".to_string();
    };
    let mut text = mean_cost(cell.mean_cost_usd, cell.unpriced_calls);
    for (n, what) in [
        (cell.passed, "pass"),
        (cell.failed, "fail"),
        (cell.errors, "error"),
    ] {
        if n > 0 {
            let _ = write!(text, ", {n} {what}");
        }
    }
    text
}

/// A mean cost as [`cost`] reads it, `-` when there is none.
fn mean_cost(usd: Option<f64>, unpriced_calls: u32) -> String {
    usd.map_or_else(|| "-".to_string(), |usd| cost(usd, unpriced_calls))
}
