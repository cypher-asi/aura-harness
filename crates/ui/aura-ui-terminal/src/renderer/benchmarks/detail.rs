//! One benchmark's numbers: its head, its sections and its notes.

use super::format::{
    fit, headline, money, saving, setup_line, short_model, task_cells, task_name, task_prefix,
    tokens, tokens_label, tokens_so_far, turn_cell, verdict_mark,
};
use super::table::{section_rule, table, Cell, Col, Row};
use crate::events::{
    arm_letter, BenchCost, BenchLiveRun, BenchStatus, BenchTaskRow, BenchTaskSetup, BenchView,
};
use crate::renderer::text::display_width;
use crate::Theme;
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

/// One benchmark's numbers; `frame` lights what is under way while it
/// runs.
pub(super) fn detail_lines(
    view: &BenchView,
    run: Option<usize>,
    theme: &Theme,
    width: usize,
    frame: Option<usize>,
) -> Vec<Line<'static>> {
    let detail = Detail {
        view,
        theme,
        width,
        frame: frame.filter(|_| view.status == BenchStatus::Running),
        prefix: task_prefix(view),
    };
    let mut lines = detail.head_lines();
    lines.extend(detail.summary_lines());
    lines.extend(detail.task_lines());
    lines.extend(detail.prompt_lines());
    lines.extend(detail.token_lines());
    lines.extend(detail.classifier_lines());
    lines.extend(detail.judge_lines());
    lines.extend(detail.run_lines(run));
    lines.extend(detail.footer_lines());
    lines
}

/// What every section of the detail draws from.
pub(super) struct Detail<'v> {
    pub(super) view: &'v BenchView,
    pub(super) theme: &'v Theme,
    pub(super) width: usize,
    pub(super) frame: Option<usize>,
    pub(super) prefix: Option<String>,
}

/// The runs of arm `arm` (0 or 1) under way.
pub(super) fn live_of(view: &BenchView, arm: usize) -> impl Iterator<Item = &BenchLiveRun> + '_ {
    view.live.iter().filter(move |r| r.arm.min(1) == arm)
}

impl Detail<'_> {
    pub(super) fn style(&self, pick: fn(&Theme) -> ratatui::style::Color) -> Style {
        Style::default().fg(pick(self.theme))
    }

    pub(super) fn text(&self) -> Style {
        self.style(|t| t.colors.text)
    }

    pub(super) fn muted(&self) -> Style {
        self.style(|t| t.colors.muted)
    }

    pub(super) fn good(&self) -> Style {
        self.style(|t| t.colors.success)
    }

    pub(super) fn bold(&self) -> Style {
        self.text().add_modifier(Modifier::BOLD)
    }

    pub(super) fn cell(&self, s: String) -> Cell {
        (s, self.text())
    }

    /// A blank line, a rule titled `title` and `rows` in `cols` under `header`.
    fn section(
        &self,
        title: &str,
        cols: &[Col],
        header: Option<&[&str]>,
        rows: &[Row],
    ) -> Vec<Line<'static>> {
        let mut lines = vec![Line::default(), section_rule(title, self.width, self.theme)];
        lines.extend(table(cols, header, rows, self.width, self.theme));
        lines
    }

    /// The name, title and status, what was set up, and a blank line.
    fn head_lines(&self) -> Vec<Line<'static>> {
        let (view, theme, width) = (self.view, self.theme, self.width);
        let status = view.status.label();
        let status_color = match view.status {
            BenchStatus::Running => theme.colors.pending,
            _ => theme.colors.success,
        };
        let room = width.saturating_sub(display_width(&view.name) + display_width(&status) + 4);
        let mut head = vec![
            Span::styled(view.name.clone(), self.bold()),
            Span::styled(
                format!("  {}  ", fit(&headline(&view.title, usize::MAX), room)),
                self.text(),
            ),
        ];
        head.extend(super::format::under_way(
            &status,
            Style::default().fg(status_color),
            self.frame,
            (theme.colors.muted, theme.colors.foreground),
        ));
        let mut lines = vec![Line::from(head)];
        lines.push(Line::from(Span::styled(
            fit(&self.about().join(" · "), width),
            self.muted(),
        )));
        if let Some(setup) = setup_line(view) {
            lines.push(Line::from(Span::styled(
                fit(&setup, width),
                Style::default().fg(theme.colors.pending),
            )));
        }
        lines.push(Line::default());
        lines
    }

    /// What the benchmark was set up with.
    fn about(&self) -> Vec<String> {
        let view = self.view;
        let mut about = Vec::new();
        if view.tasks.is_empty() {
            about.push(view.source.clone());
        } else if let Some(prefix) = &self.prefix {
            about.push(prefix.clone());
        }
        about.push(format!(
            "{} run{} per {}arm",
            view.repeats,
            if view.repeats == 1 { "" } else { "s" },
            if view.tasks.is_empty() {
                ""
            } else {
                "task and "
            },
        ));
        about.push(match view.check.as_deref() {
            _ if !view.tasks.is_empty() => "own checks".to_string(),
            Some(c) => format!("check `{c}`"),
            None => "no check".to_string(),
        });
        if let Some(usd) = view.budget_usd {
            about.push(format!("budget {}", money(usd)));
        }
        if !view.classifier.is_empty() {
            about.push(format!("classifier {}", view.classifier));
        }
        about
    }

    /// The example tasks, one row each.
    fn task_lines(&self) -> Vec<Line<'static>> {
        let view = self.view;
        if view.tasks.is_empty() {
            return Vec::new();
        }
        let rows: Vec<Row> = view.tasks.iter().map(|task| self.task_row(task)).collect();
        let judge = if view.judge.is_some() { "judge" } else { "" };
        self.section(
            "tasks",
            &[
                Col::L.flex(),
                Col::L.sep(),
                Col::R,
                Col::L,
                Col::L.sep(),
                Col::R,
                Col::L,
                Col::L.sep(),
            ],
            Some(&["task", "A", "", "", "B", "", "", judge]),
            &rows,
        )
    }

    /// The judge's marks for the runs of task `id`.
    fn judged(&self, id: &str) -> String {
        let view = self.view;
        let task_of = |name: &str| {
            view.runs
                .iter()
                .find(|r| r.name == name)
                .and_then(|r| r.task.as_deref())
        };
        view.judge
            .iter()
            .flat_map(|j| &j.verdicts)
            .filter(|v| task_of(&v.a_run) == Some(id))
            .map(verdict_mark)
            .collect()
    }

    /// One example task's row.
    fn task_row(&self, task: &BenchTaskRow) -> Row {
        let (theme, good) = (self.theme, self.good());
        let [da, db] = saving(task.arms[0].mean_cost(), task.arms[1].mean_cost());
        let [check_a, cost_a] = task_cells(&task.arms[0], theme);
        let [check_b, cost_b] = task_cells(&task.arms[1], theme);
        let last = match &task.setup {
            BenchTaskSetup::Ready => (self.judged(&task.id), self.bold()),
            BenchTaskSetup::Failed(_) => {
                (task.setup.label(), Style::default().fg(theme.colors.error))
            }
            setup => (setup.label(), Style::default().fg(theme.colors.pending)),
        };
        Row::Cells(vec![
            self.cell(task_name(&task.id, self.prefix.as_deref()).to_string()),
            check_a,
            cost_a,
            (da, good),
            check_b,
            cost_b,
            (db, good),
            last,
        ])
    }

    /// The prompts replayed, when there are several.
    fn prompt_lines(&self) -> Vec<Line<'static>> {
        let view = self.view;
        if view.prompts.len() <= 1 {
            return Vec::new();
        }
        let [a, b] = &view.arms;
        let rows: Vec<Row> = view
            .prompts
            .iter()
            .enumerate()
            .map(|(i, prompt)| {
                Row::Cells(vec![
                    ((i + 1).to_string(), self.muted()),
                    self.cell(headline(prompt, usize::MAX)),
                    self.cell(turn_cell(a.turns.get(i))),
                    self.cell(turn_cell(b.turns.get(i))),
                ])
            })
            .collect();
        self.section(
            "prompts · mean turn cost, test or build ✓ ✗",
            &[Col::R, Col::L.flex(), Col::L.sep(), Col::L.sep()],
            Some(&["#", "prompt", "A", "B"]),
            &rows,
        )
    }

    /// Each arm's tokens per model and purpose.
    fn token_lines(&self) -> Vec<Line<'static>> {
        let mut rows = Vec::new();
        for (i, arm) in self.view.arms.iter().enumerate() {
            if i > 0 {
                rows.push(Row::Rule);
            }
            let used = tokens_so_far(&arm.tokens, live_of(self.view, i));
            if used.is_empty() {
                rows.push(Row::Cells(vec![
                    self.cell(arm_letter(i).to_string()),
                    ("no model calls".to_string(), self.muted()),
                ]));
            }
            for (n, t) in used.iter().enumerate() {
                rows.push(Row::Cells(vec![
                    self.cell(if n == 0 { arm_letter(i) } else { "" }.to_string()),
                    self.cell(tokens_label(t)),
                    self.cell(t.calls.to_string()),
                    self.cell(tokens(t.input_tokens)),
                    self.cell(tokens(t.output_tokens)),
                    self.cell(t.cost().read(money)),
                ]));
            }
        }
        self.section(
            "tokens",
            &[Col::L, Col::L.flex(), Col::R.sep(), Col::R, Col::R, Col::R],
            Some(&["", "model", "calls", "in", "out", "cost"]),
            &rows,
        )
    }

    /// What classifying the runs' prompts took.
    fn classifier_lines(&self) -> Vec<Line<'static>> {
        let classify = &self.view.classify;
        if classify.is_empty() {
            return Vec::new();
        }
        let calls: u32 = classify.iter().map(|t| t.calls).sum();
        let cost = BenchCost::new(
            classify.iter().map(|t| t.cost_usd).sum(),
            classify.iter().map(|t| t.unpriced_calls).sum(),
        );
        let models: Vec<&str> = classify.iter().map(|t| short_model(&t.model)).collect();
        vec![
            Line::default(),
            section_rule(
                &format!(
                    "classifier {} · {calls} call{} · {}, not in arm costs",
                    models.join(", "),
                    if calls == 1 { "" } else { "s" },
                    cost.read(money)
                ),
                self.width,
                self.theme,
            ),
        ]
    }

    /// The judge's verdicts.
    fn judge_lines(&self) -> Vec<Line<'static>> {
        let Some(judge) = &self.view.judge else {
            return Vec::new();
        };
        let rows: Vec<Row> = judge
            .verdicts
            .iter()
            .map(|v| {
                let reference = match (&v.a_vs_reference, &v.b_vs_reference) {
                    (None, None) => String::new(),
                    (x, y) => format!(
                        "A{} B{}",
                        super::format::reference_mark(x.as_deref()),
                        super::format::reference_mark(y.as_deref())
                    ),
                };
                Row::Cells(vec![
                    self.cell(format!("{} {}", v.a_run, v.b_run)),
                    (verdict_mark(v).to_string(), self.bold()),
                    self.cell(reference),
                    (headline(&v.reasoning, usize::MAX), self.muted()),
                ])
            })
            .collect();
        let title = format!(
            "judge {} · {} blind call{} · {}, not in arm costs",
            short_model(&judge.model),
            judge.verdicts.len(),
            if judge.verdicts.len() == 1 { "" } else { "s" },
            judge.cost().read(money)
        );
        self.section(
            &title,
            &[Col::L, Col::L, Col::L, Col::L.flex()],
            None,
            &rows,
        )
    }

    /// The notes.
    fn footer_lines(&self) -> Vec<Line<'static>> {
        let (view, width) = (self.view, self.width);
        let accent = self.good();
        view.notes
            .iter()
            .map(|note| Line::from(Span::styled(fit(&format!("note: {note}"), width), accent)))
            .collect()
    }
}
