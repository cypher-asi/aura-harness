//! The detail's runs table: every run, ended or under way.

use super::detail::Detail;
use super::format::{
    category, category_name, doing_text, duration, headline, led, money, moved_up, short_model,
    task_name,
};
use super::table::{table, Cell, Col, Row};
use crate::events::{run_order, BenchLiveRun, BenchRun, BenchView};
use ratatui::{style::Style, text::Line};

/// The column of the runs table a run under way shows what it is doing
/// from: the last, after `files`, where an ended run shows its error.
pub(super) const LIVE_STATUS_AT: usize = 13;

enum Entry {
    Ended(usize),
    Live(usize),
}

/// Which of the runs table's optional columns have something in them.
#[derive(Clone, Copy)]
struct Shown {
    moved: bool,
    led: bool,
}

/// The runs of `view`, ended and under way, in run order.
fn entries(view: &BenchView) -> Vec<((u32, usize), Entry)> {
    let mut entries: Vec<((u32, usize), Entry)> = view
        .runs
        .iter()
        .enumerate()
        .map(|(i, r)| (run_order(&r.name, r.arm), Entry::Ended(i)))
        .chain(
            view.live
                .iter()
                .enumerate()
                .map(|(i, live)| (run_order(&live.name, live.arm), Entry::Live(i))),
        )
        .collect();
    entries.sort_by_key(|(order, _)| *order);
    entries
}

impl Detail<'_> {
    /// The runs section; `run` is the one picked.
    pub(super) fn run_lines(&self, run: Option<usize>) -> Vec<Line<'static>> {
        let rows = self.run_rows(run);
        let mut lines = vec![
            Line::default(),
            super::table::section_rule("runs", self.width, self.theme),
        ];
        lines.extend(table(
            &[
                Col::L,
                Col::L,
                Col::L.flex(),
                Col::L.flex(),
                Col::L.flex(),
                Col::L.flex(),
                Col::L.flex(),
                Col::L.flex(),
                Col::L.flex(),
                Col::R,
                Col::R,
                Col::L,
                Col::R,
                Col::L.flex(),
            ],
            None,
            &rows,
            self.width,
            self.theme,
        ));
        lines
    }

    fn run_rows(&self, run: Option<usize>) -> Vec<Row> {
        let view = self.view;
        let shown = Shown {
            moved: view.runs.iter().any(|r| r.escalated_to.is_some()),
            led: view.runs.iter().any(|r| r.led),
        };
        let mut rows = vec![self.run_header(shown)];
        let pending = self.style(|t| t.colors.pending);
        let mut number = None;
        for ((n, _), entry) in entries(view) {
            if number.is_some_and(|last| last != n) {
                rows.push(Row::Rule);
            }
            number = Some(n);
            rows.push(match entry {
                Entry::Ended(i) => self.ended_row(&view.runs[i], run == Some(i), shown),
                Entry::Live(i) => live_row(&view.live[i], self, self.text(), pending),
            });
        }
        rows
    }

    fn run_header(&self, shown: Shown) -> Row {
        let view = self.view;
        let has_task = view.runs.iter().any(|r| r.task.is_some())
            || view.live.iter().any(|r| r.task.is_some());
        let any_drivers = view.runs.iter().any(|r| r.drivers.is_some());
        let header = [
            "",
            "run",
            "model",
            "classified",
            "difficulty",
            if any_drivers { "why" } else { "" },
            if shown.moved { "moved up" } else { "" },
            if shown.led { "led" } else { "" },
            if has_task { "task" } else { "" },
            "time",
            "cost",
            "check",
            "files",
            "",
        ];
        let muted = self.muted();
        Row::Cells(header.iter().map(|h| ((*h).to_string(), muted)).collect())
    }

    /// The row of `r`, an ended run, in the selection style if `selected`.
    fn ended_row(&self, r: &BenchRun, selected: bool, shown: Shown) -> Row {
        let errored = self.good();
        let base = if r.error.is_some() {
            errored
        } else {
            self.text()
        };
        let check = match r.check_passed {
            Some(true) => ("✓", self.good()),
            Some(false) => ("✗", self.style(|t| t.colors.error)),
            None => ("", base),
        };
        let optional = |show: bool, f: fn(&BenchRun) -> String| {
            (if show { f(r) } else { String::new() }, base)
        };
        let cells: Vec<Cell> = vec![
            ((if selected { "›" } else { " " }).to_string(), base),
            (r.name.clone(), base),
            (short_model(&r.model).to_string(), base),
            (category(r), base),
            (r.difficulty.clone().unwrap_or_default(), base),
            (r.drivers.clone().unwrap_or_default(), self.muted()),
            optional(shown.moved, moved_up),
            optional(shown.led, led),
            (
                r.task
                    .as_deref()
                    .map_or("", |t| task_name(t, self.prefix.as_deref()))
                    .to_string(),
                base,
            ),
            (duration(r.wall_ms), base),
            (r.cost().read(money), base),
            (check.0.to_string(), check.1),
            (r.files_changed.to_string(), base),
            (
                r.error
                    .as_deref()
                    .map_or_else(String::new, |e| headline(e, usize::MAX)),
                errored,
            ),
        ];
        if selected {
            let picked = Style::default()
                .bg(self.theme.colors.selection)
                .fg(self.theme.colors.on_selection);
            Row::Picked(cells, picked)
        } else {
            Row::Cells(cells)
        }
    }
}

/// The row of `live`, a run under way, lit by the detail's frame while
/// it works.
fn live_row(live: &BenchLiveRun, detail: &Detail<'_>, text: Style, pending: Style) -> Row {
    let arm = &detail.view.arms[live.arm.min(1)];
    let empty = || (String::new(), text);
    let model = live.model.as_deref().unwrap_or(&arm.label);
    let cells = vec![
        (" ".to_string(), text),
        (live.name.clone(), text),
        (short_model(model).to_string(), text),
        (category_name(live.category.as_deref()), text),
        (live.difficulty.clone().unwrap_or_default(), text),
        empty(),
        empty(),
        empty(),
        (
            live.task
                .as_deref()
                .map_or("", |t| task_name(t, detail.prefix.as_deref()))
                .to_string(),
            text,
        ),
        (live.wall_ms.map_or_else(String::new, duration), text),
        (live.cost().read(money), text),
    ];
    let status = match (&live.doing, live.wall_ms) {
        (_, None) => "waiting for its task's files".to_string(),
        (Some(doing), Some(_)) => doing_text(doing),
        (None, Some(_)) => "working".to_string(),
    };
    Row::Live {
        cells,
        status_at: LIVE_STATUS_AT,
        status: (status, pending),
        frame: detail.frame.filter(|_| live.wall_ms.is_some()),
    }
}
