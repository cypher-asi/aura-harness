//! Draw the chat panel's LOG tab: where the log is stored and a `Copy log`
//! button on top, then the run and the runs below it as panels that fold
//! open and shut, and the keys at the bottom.

use super::benchmarks::fit;
use super::panels::body_block;
use super::text::{display_width, wrap_words};
use crate::app::run_log::{LogPanel, LogRow, LogState};
use crate::events::{LogField, LogSection, LogStatus};
use crate::{App, Theme};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};
use unicode_width::UnicodeWidthChar;

/// Lines of a section shown before the rest is cut.
const SECTION_LINES: usize = 400;

/// The button on top that copies the whole log.
const COPY_BUTTON: &str = " ⧉ Copy log ";

const KEYS: &str =
    "c copy log · y copy panel · p copy path · o open file · Enter fold · e/E all · J json · r refresh · Esc chat";

/// Draw the LOG tab in `area`, the chat panel below its header bar.
pub(super) fn render_run_log(
    frame: &mut Frame,
    area: Rect,
    app: &mut App,
    theme: &Theme,
    is_focused: bool,
) {
    let block = body_block((is_focused, app.host_chrome()), theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    app.set_chat_layout(Vec::new(), 0);
    app.set_chat_images(Vec::new());
    app.set_chat_width(inner.width);
    app.set_chat_text_area(inner);
    let padded = Rect {
        x: inner.x.saturating_add(1),
        width: inner.width.saturating_sub(2),
        ..inner
    };
    if padded.width < 16 || padded.height < 5 {
        app.run_log_mut().set_hits(Vec::new(), None);
        return;
    }
    let width = usize::from(padded.width);
    let state = app.run_log();

    let (top, button) = header_lines(state, padded, theme);
    frame.render_widget(Paragraph::new(top), Rect { height: 3, ..padded });
    let footer = Rect {
        y: padded.bottom() - 1,
        height: 1,
        ..padded
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            fit(KEYS, width),
            Style::default().fg(theme.colors.faint),
        ))),
        footer,
    );
    let body = Rect {
        y: padded.y + 3,
        height: padded.height.saturating_sub(4),
        ..padded
    };

    let (lines, owners, selected_line) = document(state, width, theme);
    let height = usize::from(body.height);
    let mut scroll = state.scroll;
    if state.follow {
        if let Some(line) = selected_line {
            if line < scroll {
                scroll = line;
            } else if line >= scroll + height {
                scroll = line + 1 - height;
            }
        }
    }
    scroll = scroll.min(lines.len().saturating_sub(height));
    let hits: Vec<(u16, LogPanel)> = owners
        .into_iter()
        .enumerate()
        .skip(scroll)
        .take(height)
        .filter_map(|(i, owner)| {
            let y = body.y + u16::try_from(i - scroll).ok()?;
            Some((y, owner?))
        })
        .collect();
    let offset = u16::try_from(scroll).unwrap_or(u16::MAX);
    frame.render_widget(Paragraph::new(lines).scroll((offset, 0)), body);

    app.set_chat_rows(Some((body, scroll)));
    let state = app.run_log_mut();
    state.scroll = scroll;
    state.follow = false;
    state.set_hits(hits, button);
}

/// The title with `Copy log`, the log's path, and a rule; and where the
/// button was drawn.
fn header_lines(state: &LogState, area: Rect, theme: &Theme) -> (Vec<Line<'static>>, Option<Rect>) {
    let width = usize::from(area.width);
    let muted = Style::default().fg(theme.colors.muted);
    let bold = Style::default()
        .fg(theme.colors.text)
        .add_modifier(Modifier::BOLD);
    let Some(view) = &state.view else {
        let note = if state.loading {
            "Loading the log…".to_string()
        } else {
            state
                .notice
                .clone()
                .unwrap_or_else(|| "No log yet".to_string())
        };
        return (
            vec![
                Line::from(Span::styled("Run log", bold)),
                Line::from(Span::styled(fit(&note, width), muted)),
                rule(width, theme),
            ],
            None,
        );
    };
    let button_width = display_width(COPY_BUTTON);
    let title = view.root().map_or("", |r| r.title.as_str());
    let mut spans = vec![Span::styled("Run log  ", bold)];
    let room = width.saturating_sub(button_width + 11);
    spans.push(Span::styled(fit(title, room), Style::default().fg(theme.colors.text)));
    let used: usize = spans.iter().map(|s| display_width(&s.content)).sum();
    let button = (width > used + button_width).then(|| {
        spans.push(Span::raw(" ".repeat(width - used - button_width)));
        spans.push(Span::styled(
            COPY_BUTTON,
            Style::default()
                .fg(theme.colors.background)
                .bg(theme.colors.primary)
                .add_modifier(Modifier::BOLD),
        ));
        Rect {
            x: area.x + u16::try_from(width - button_width).unwrap_or(0),
            y: area.y,
            width: u16::try_from(button_width).unwrap_or(0),
            height: 1,
        }
    });
    let label = "Stored at ";
    let path = fit_start(&view.path, width.saturating_sub(label.len()));
    let mut where_line = vec![
        Span::styled(label, muted),
        Span::styled(path, Style::default().fg(theme.colors.primary)),
    ];
    if view.root().is_some_and(|r| r.status == LogStatus::Running) {
        where_line.push(Span::styled(
            "  ● live",
            Style::default().fg(theme.colors.success),
        ));
    } else if state.loading {
        where_line.push(Span::styled("  refreshing…", muted));
    }
    (
        vec![Line::from(spans), Line::from(where_line), rule(width, theme)],
        button,
    )
}

fn rule(width: usize, theme: &Theme) -> Line<'static> {
    Line::from(Span::styled(
        "─".repeat(width),
        Style::default().fg(theme.colors.faint),
    ))
}

/// `text` cut from the start to `max` columns, as `…/runs/x/log.json`.
fn fit_start(text: &str, max: usize) -> String {
    if display_width(text) <= max || max == 0 {
        return text.to_string();
    }
    let mut kept: Vec<char> = Vec::new();
    let mut used = 1;
    for c in text.chars().rev() {
        let w = c.width().unwrap_or(0);
        if used + w > max {
            break;
        }
        kept.push(c);
        used += w;
    }
    std::iter::once('…').chain(kept.into_iter().rev()).collect()
}

/// `text` broken into lines at most `width` columns wide, at any
/// character.
fn hard_wrap(text: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        let mut current = String::new();
        let mut used = 0;
        for c in line.chars() {
            let c = if c == '\t' { ' ' } else { c };
            let w = c.width().unwrap_or(0);
            if used + w > width && !current.is_empty() {
                out.push(std::mem::take(&mut current));
                used = 0;
            }
            current.push(c);
            used += w;
        }
        out.push(current);
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

/// `text` wrapped at words, keeping its line breaks and blank lines.
fn prose_wrap(text: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            out.push(String::new());
        } else if display_width(line) <= width {
            out.push(line.to_string());
        } else {
            out.extend(wrap_words(line, width));
        }
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

fn status_mark(status: LogStatus, theme: &Theme) -> Span<'static> {
    let (mark, color) = match status {
        LogStatus::Ok => ("✓ ", theme.colors.success),
        LogStatus::Warn => ("! ", theme.colors.warning),
        LogStatus::Error => ("✗ ", theme.colors.error),
        LogStatus::Running => ("◌ ", theme.colors.pending),
    };
    Span::styled(mark, Style::default().fg(color))
}

/// Every line of the panels on screen; the panel whose header each line
/// is; and the line of the selected panel's header.
fn document(
    state: &LogState,
    width: usize,
    theme: &Theme,
) -> (Vec<Line<'static>>, Vec<Option<LogPanel>>, Option<usize>) {
    let mut lines = Vec::new();
    let mut owners = Vec::new();
    let mut selected = None;
    if state.view.is_none() {
        let muted = Style::default().fg(theme.colors.muted);
        let text = if state.loading {
            "Reading the run log of the agent on screen…"
        } else {
            "The log of the agent on screen shows here once it has run a turn: every turn, model \
             call, thought, tool call and harness decision, with the runs of the agents it spawned. \
             r tries again."
        };
        for line in prose_wrap(text, width) {
            lines.push(Line::from(Span::styled(line, muted)));
            owners.push(None);
        }
        return (lines, owners, None);
    }
    for row in state.rows() {
        if state.selected.as_ref() == Some(&row.panel) {
            selected = Some(lines.len());
        }
        let picked = state.selected.as_ref() == Some(&row.panel);
        lines.push(header_line(state, &row, width, picked, theme));
        owners.push(Some(row.panel.clone()));
        if state.is_expanded(&row.panel) {
            let indent = row.depth * 2 + 4;
            for line in body_lines(state, &row.panel, width.saturating_sub(indent), theme) {
                let mut spans = vec![Span::raw(" ".repeat(indent))];
                spans.extend(line.spans);
                lines.push(Line::from(spans));
                owners.push(None);
            }
        }
    }
    (lines, owners, selected)
}

fn header_line(
    state: &LogState,
    row: &LogRow,
    width: usize,
    picked: bool,
    theme: &Theme,
) -> Line<'static> {
    let muted = Style::default().fg(theme.colors.muted);
    let text = Style::default().fg(theme.colors.text);
    let bold = text.add_modifier(Modifier::BOLD);
    let marker = if !state.opens(&row.panel) {
        "  "
    } else if state.is_expanded(&row.panel) {
        "▾ "
    } else {
        "▸ "
    };
    let mut spans = vec![
        Span::raw("  ".repeat(row.depth)),
        Span::styled(marker, muted),
    ];
    let (label, detail) = match &row.panel {
        LogPanel::Run(id) => {
            let Some(run) = state.run(id) else {
                return Line::default();
            };
            spans.push(status_mark(run.status, theme));
            let (kind, style) = if run.depth == 0 {
                ("RUN  ", bold)
            } else {
                ("SUBAGENT  ", Style::default().fg(theme.colors.secondary).add_modifier(Modifier::BOLD))
            };
            spans.push(Span::styled(kind, style));
            (Span::styled(run.title.clone(), bold), run.detail.clone())
        }
        LogPanel::Details(id) => {
            let problems = state.run(id).map_or(0, |r| r.problems.len());
            let detail = if problems > 0 {
                format!("{problems} problem{}", if problems == 1 { "" } else { "s" })
            } else {
                String::new()
            };
            (Span::styled("Details".to_string(), text), detail)
        }
        LogPanel::Files(_) => (Span::styled("Files on disk".to_string(), text), String::new()),
        LogPanel::Turn(id, n) => {
            let Some(turn) = state.turn(id, *n) else {
                return Line::default();
            };
            spans.push(status_mark(turn.status, theme));
            spans.push(Span::styled(format!("Turn {n}  "), bold));
            (Span::styled(turn.prompt.clone(), text), turn.detail.clone())
        }
        LogPanel::Step(id, seq) => {
            let Some(step) = state.step(id, *seq) else {
                return Line::default();
            };
            spans.push(status_mark(step.status, theme));
            spans.push(Span::styled(format!("{seq:>3} {:<9}", step.kind), muted));
            (Span::styled(step.label.clone(), text), step.detail.clone())
        }
    };
    let used: usize = spans.iter().map(|s| display_width(&s.content)).sum();
    let detail_width = display_width(&detail);
    let room = width.saturating_sub(used + if detail.is_empty() { 0 } else { detail_width + 2 });
    let label_text = fit(&label.content, room);
    let pad = width.saturating_sub(used + display_width(&label_text) + detail_width);
    spans.push(Span::styled(label_text, label.style));
    if !detail.is_empty() && used + detail_width + 2 <= width {
        spans.push(Span::raw(" ".repeat(pad)));
        spans.push(Span::styled(detail, muted));
    } else if picked {
        spans.push(Span::raw(" ".repeat(pad + detail_width)));
    }
    if picked {
        let style = Style::default()
            .bg(theme.colors.selection)
            .fg(theme.colors.on_selection);
        spans = spans.into_iter().map(|s| s.patch_style(style)).collect();
    }
    Line::from(spans)
}

fn field_lines(fields: &[LogField], width: usize, theme: &Theme, accent: Option<&str>) -> Vec<Line<'static>> {
    let label_width = fields
        .iter()
        .map(|(label, _)| display_width(label))
        .max()
        .unwrap_or(0)
        .min(20);
    let value_width = width.saturating_sub(label_width + 2).max(8);
    let mut lines = Vec::new();
    for (label, value) in fields {
        let style = if accent == Some(label.as_str()) {
            Style::default()
                .fg(theme.colors.primary)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.colors.text)
        };
        for (i, part) in hard_wrap(value, value_width).into_iter().enumerate() {
            let head = if i == 0 {
                format!("{:<label_width$}  ", fit(label, label_width))
            } else {
                " ".repeat(label_width + 2)
            };
            lines.push(Line::from(vec![
                Span::styled(head, Style::default().fg(theme.colors.muted)),
                Span::styled(part, style),
            ]));
        }
    }
    lines
}

fn section_lines(section: &LogSection, width: usize, theme: &Theme) -> Vec<Line<'static>> {
    let faint = Style::default().fg(theme.colors.faint);
    let mut lines = vec![Line::from(vec![
        Span::styled("── ", faint),
        Span::styled(
            section.title.clone(),
            Style::default()
                .fg(theme.colors.muted)
                .add_modifier(Modifier::BOLD),
        ),
    ])];
    let inner = width.saturating_sub(2).max(8);
    let body = if section.code {
        hard_wrap(&section.body, inner)
    } else {
        prose_wrap(&section.body, inner)
    };
    let style = if section.title == "Thinking" {
        Style::default()
            .fg(theme.colors.muted)
            .add_modifier(Modifier::ITALIC)
    } else {
        Style::default().fg(theme.colors.text)
    };
    let total = body.len();
    for line in body.into_iter().take(SECTION_LINES) {
        lines.push(Line::from(vec![
            Span::styled("│ ", faint),
            Span::styled(line, style),
        ]));
    }
    if total > SECTION_LINES {
        lines.push(Line::from(Span::styled(
            format!("… {} more lines; y copies the whole step", total - SECTION_LINES),
            Style::default().fg(theme.colors.muted),
        )));
    }
    lines
}

fn body_lines(state: &LogState, panel: &LogPanel, width: usize, theme: &Theme) -> Vec<Line<'static>> {
    match panel {
        LogPanel::Run(_) | LogPanel::Turn(..) => Vec::new(),
        LogPanel::Details(id) => {
            let Some(run) = state.run(id) else {
                return Vec::new();
            };
            let mut lines = field_lines(&run.fields, width, theme, None);
            for problem in &run.problems {
                for (i, part) in prose_wrap(problem, width.saturating_sub(2)).into_iter().enumerate() {
                    let head = if i == 0 { "! " } else { "  " };
                    lines.push(Line::from(Span::styled(
                        format!("{head}{part}"),
                        Style::default().fg(theme.colors.warning),
                    )));
                }
            }
            lines
        }
        LogPanel::Files(id) => {
            let Some(run) = state.run(id) else {
                return Vec::new();
            };
            let mut lines = field_lines(&run.files, width, theme, Some("log"));
            if run.depth == 0 {
                lines.push(Line::from(Span::styled(
                    "log.json holds this run and every subagent's: c or Copy log copies it",
                    Style::default().fg(theme.colors.muted),
                )));
            }
            lines
        }
        LogPanel::Step(id, seq) => {
            let Some(step) = state.step(id, *seq) else {
                return Vec::new();
            };
            if state.raw.contains(panel) {
                return hard_wrap(&step.json, width)
                    .into_iter()
                    .map(|line| Line::from(Span::styled(line, Style::default().fg(theme.colors.text))))
                    .collect();
            }
            let mut lines = field_lines(&step.fields, width, theme, None);
            for section in &step.sections {
                lines.extend(section_lines(section, width, theme));
            }
            lines
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{LogRunView, LogStepView, LogTurnView, LogView, UiCommand};
    use ratatui::{backend::TestBackend, Terminal};

    fn view() -> LogView {
        LogView {
            agent_id: String::new(),
            path: "/home/me/.aura/runs/1a-x/log.json".into(),
            json: "{}".into(),
            runs: vec![
                LogRunView {
                    run_id: "1a-x".into(),
                    title: "fix the parser".into(),
                    detail: "sonnet-5-5 · 1 turn".into(),
                    files: vec![("log".into(), "/home/me/.aura/runs/1a-x/log.json".into())],
                    fields: vec![("model".into(), "claude/claude-sonnet-5-5".into())],
                    turns: vec![LogTurnView {
                        turn: 1,
                        prompt: "fix the parser".into(),
                        detail: "2 calls".into(),
                        steps: vec![LogStepView {
                            seq: 2,
                            kind: "model".into(),
                            label: "→ task".into(),
                            fields: vec![("model".into(), "m".into())],
                            sections: vec![LogSection {
                                title: "Thinking".into(),
                                body: "the parser drops the last token".into(),
                                code: false,
                            }],
                            spawned: vec![1],
                            ..LogStepView::default()
                        }],
                        ..LogTurnView::default()
                    }],
                    ..LogRunView::default()
                },
                LogRunView {
                    run_id: "1b-y".into(),
                    title: "find the token bug".into(),
                    depth: 1,
                    ..LogRunView::default()
                },
            ],
        }
    }

    fn screen(app: &mut App) -> String {
        let mut terminal = Terminal::new(TestBackend::new(110, 30)).unwrap();
        terminal
            .draw(|frame| render_run_log(frame, frame.area(), app, &Theme::default(), true))
            .unwrap();
        let buffer = terminal.backend().buffer();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn shows_where_the_log_is_the_copy_button_and_nested_subagents() {
        let mut app = App::new();
        app.process_command(UiCommand::LogView(Box::new(view())));
        app.run_log_mut()
            .toggle(&LogPanel::Step("1a-x".into(), 2));
        let text = screen(&mut app);
        assert!(text.contains("Copy log"), "{text}");
        assert!(text.contains("Stored at /home/me/.aura/runs/1a-x/log.json"), "{text}");
        assert!(text.contains("RUN  fix the parser"), "{text}");
        assert!(text.contains("Turn 1"), "{text}");
        assert!(text.contains("the parser drops the last token"), "{text}");
        assert!(text.contains("SUBAGENT  find the token bug"), "{text}");

        let button = app.run_log().copy_button_area().expect("drawn");
        assert!(app.click_log(button.x + 1, button.y));
        let buffer = ratatui::buffer::Buffer::empty(Rect::new(0, 0, 1, 1));
        assert_eq!(app.take_copy_text(&buffer).as_deref(), Some("{}"));
    }

    #[test]
    fn says_why_there_is_no_log() {
        let mut app = App::new();
        app.process_command(UiCommand::LogNotice {
            agent_id: String::new(),
            notice: "This agent has no run log yet; send it a prompt".into(),
        });
        let text = screen(&mut app);
        assert!(text.contains("no run log yet"), "{text}");
        assert!(!text.contains("Copy log"));
    }
}
