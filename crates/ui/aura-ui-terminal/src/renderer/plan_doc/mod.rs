//! A plan shown over the chat panel as a document to read, not as its
//! source: its title, its overview, its todos as a checklist, then its
//! body set like an agent's reply.
//!
//! ```text
//! │ PLAN Plans that read like a document          2/5 TODOS ✕ │
//! │                                                           │
//! │  Plans that read like a document                          │
//! │                                                           │
//! │  Change the guidance the agent gets for writing plans...  │
//! │                                                           │
//! │  TODOS  2/5                                               │
//! │  ☒ Rewrite the plan-writing bullet                        │
//! │  ▣ Update the create_plan description                     │
//! │  ☐ Extend the plan_mode test                              │
//! │  ──────────────────────────────────────────────────────── │
//! │  Why plans read badly today                               │
//! │                                                           │
//! │  The agent writes plans the way it is told to. Two ...    │
//! └ 1-24/80 ──────────────────────────────────────────────────┘
//! ```
//!
//! The frontmatter and the generated header's marker never show. Flowing
//! text (title, overview, todos, the body's paragraphs, headings, lists
//! and quotes) is published as prose rows, so a front end with a
//! proportional face sets it in that face, as it does the chat; code,
//! tables and rules stay on the cell grid.
//!
//! Lines are laid out once per width, face and theme and kept on the
//! [`PlanDoc`]; each is exactly one screen row, so the view scrolls by
//! row.

use super::chat_panel::{pack_prose, prose_columns, terminal_text_area};
use super::chrome::{body_block, render_scrollbar, ScrollView};
use super::markdown::{parse_content_segments, render_markdown_flow, ContentSegment};
use super::plans::{marker, todo_style};
use super::segments::render_code_segment;
use super::text::Measure;
use crate::app::{PlanDoc, ProseRow};
use crate::{App, Theme};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, Paragraph, Widget},
    Frame,
};

/// Rows kept above the focused todo when a plan opens at it.
const FOCUS_CONTEXT: usize = 2;

/// Columns before a todo's text: its box and a space.
const TODO_INDENT: u16 = 2;

/// Lay `doc` out for `width` columns in `measure`'s face, unless it
/// already is.
fn ensure_layout(doc: &mut PlanDoc, width: usize, measure: Measure, theme: &Theme) {
    let key = (width, measure.is_grid(), theme.name.clone());
    if doc.layout.key.as_ref() == Some(&key) {
        return;
    }
    let laid = layout(doc, width, measure, theme);
    doc.layout.lines = laid.lines;
    doc.layout.prose = laid.prose;
    doc.layout.todo_rows = laid.todo_rows;
    doc.layout.key = Some(key);
}

/// A plan's rows, built top to bottom.
#[derive(Default)]
struct Laid {
    lines: Vec<Line<'static>>,
    prose: Vec<Option<u16>>,
    todo_rows: Vec<usize>,
}

impl Laid {
    /// Add `lines`, each prose or not as `prose` says.
    fn extend(&mut self, lines: Vec<Line<'static>>, prose: Vec<Option<u16>>) {
        self.prose.extend(prose);
        self.lines.extend(lines);
        self.prose.resize(self.lines.len(), None);
    }

    /// Add a blank row, unless the last one is blank already.
    fn gap(&mut self) {
        if self.lines.last().is_some_and(|line| line.width() > 0) {
            self.lines.push(Line::default());
            self.prose.push(None);
        }
    }

    /// Add one row on the cell grid.
    fn push(&mut self, line: Line<'static>) {
        self.lines.push(line);
        self.prose.push(None);
    }
}

/// The rows of `doc` for a panel `width` columns wide.
fn layout(doc: &PlanDoc, width: usize, measure: Measure, theme: &Theme) -> Laid {
    let width = width.max(1);
    let mut laid = Laid::default();

    let title = Style::default()
        .fg(theme.colors.foreground)
        .add_modifier(Modifier::BOLD);
    let (lines, prose) =
        render_markdown_flow(&escape_heading(&doc.title), width, title, theme, measure);
    laid.extend(lines, prose);

    if !doc.overview.is_empty() {
        laid.gap();
        let overview = Style::default().fg(theme.colors.muted);
        let (lines, prose) = render_markdown_flow(&doc.overview, width, overview, theme, measure);
        laid.extend(lines, prose);
    }

    if !doc.todos.is_empty() {
        push_todos(&mut laid, doc, (width, measure), theme);
    }

    if !doc.body.trim().is_empty() {
        push_body(&mut laid, doc, (width, measure), theme);
    }
    while laid.lines.last().is_some_and(|line| line.width() == 0) {
        laid.lines.pop();
        laid.prose.pop();
    }
    laid
}

/// The TODOS section: its label with the progress, then every todo.
fn push_todos(laid: &mut Laid, doc: &PlanDoc, (width, measure): (usize, Measure), theme: &Theme) {
    laid.gap();
    let (closed, total) = doc.progress();
    let label = Style::default()
        .fg(theme.colors.muted)
        .add_modifier(Modifier::BOLD);
    laid.push(Line::from(vec![
        Span::styled("TODOS", label),
        Span::styled(
            format!("  {closed}/{total}"),
            Style::default().fg(theme.colors.muted),
        ),
    ]));
    let room = width.saturating_sub(usize::from(TODO_INDENT)).max(1);
    for index in 0..doc.todos.len() {
        push_todo(laid, (doc, index), (room, measure), theme);
    }
}

/// Todo `index` of `doc`, `room` columns wide: its marker, then its text
/// and assignee.
fn push_todo(
    laid: &mut Laid,
    (doc, index): (&PlanDoc, usize),
    (room, measure): (usize, Measure),
    theme: &Theme,
) {
    let todo = &doc.todos[index];
    laid.todo_rows.push(laid.lines.len());
    let style = todo_style(todo.state, theme);
    let mut text = todo.content.clone();
    if let Some(assignee) = &todo.assignee {
        text.push_str(&format!(" (@{assignee})"));
    }
    let (mut lines, prose) = render_markdown_flow(&text, room, style, theme, measure);
    if lines.is_empty() {
        lines.push(Line::default());
    }
    let box_style = Style::default().fg(super::plans::marker_color(todo.state, theme));
    for (i, line) in lines.iter_mut().enumerate() {
        let lead = if i == 0 {
            Span::styled(format!("{} ", marker(todo.state)), box_style)
        } else {
            Span::raw("  ")
        };
        line.spans.insert(0, lead);
    }
    let prose = prose
        .into_iter()
        .map(|cols| cols.map(|cols| cols.saturating_add(TODO_INDENT)))
        .collect();
    laid.extend(lines, prose);
}

/// The body below a rule: its text and code blocks.
fn push_body(laid: &mut Laid, doc: &PlanDoc, (width, measure): (usize, Measure), theme: &Theme) {
    let rule = Style::default().fg(theme.colors.faint);
    laid.push(Line::default());
    laid.push(Line::from(Span::styled("─".repeat(width), rule)));
    laid.push(Line::default());
    let base = Style::default().fg(theme.colors.text);
    for segment in parse_content_segments(&doc.body) {
        match segment {
            ContentSegment::Text(text) => {
                let (lines, prose) = render_markdown_flow(&text, width, base, theme, measure);
                laid.extend(lines, prose);
            }
            ContentSegment::CodeBlock { language, code } => {
                let mut lines = Vec::new();
                // The copy button is the chat's; here the block is
                // only read, so its hit is not kept.
                let _ = render_code_segment(&mut lines, &language, &code, width, theme);
                laid.extend(lines, Vec::new());
            }
        }
    }
}

/// `title` as a paragraph of Markdown: a leading `#`, `-`, `>` or `1.`
/// would make it a heading, list or quote, so that mark is escaped.
fn escape_heading(title: &str) -> String {
    match title.chars().next() {
        Some('#' | '-' | '*' | '+' | '>') => format!("\\{title}"),
        Some('0'..='9') => {
            let digits = title.len() - title.trim_start_matches(|c: char| c.is_ascii_digit()).len();
            match title.get(digits..digits + 1) {
                Some(mark @ ("." | ")")) => {
                    format!("{}\\{mark}{}", &title[..digits], &title[digits + 1..])
                }
                _ => title.to_string(),
            }
        }
        _ => title.to_string(),
    }
}

/// What drawing the plan left for the detail view's bookkeeping.
pub(super) struct DrawnPlan {
    /// Largest useful scroll, in rows
    pub(super) max_scroll: usize,
    /// Rows on screen
    pub(super) shown: usize,
    /// Row on top
    pub(super) first: usize,
    /// What the scrollbar shows
    pub(super) view: ScrollView,
    /// Where the text went: the chat's text column, over the detail
    /// view's rows
    pub(super) column: Rect,
}

/// The columns of `rows` the plan's text takes in the chat `panel`: the
/// chat's own text column (its side padding, width cap and centering),
/// so the text does not move sideways when a plan opens over the chat.
fn text_column(panel: Rect, rows: Rect, hosted: bool, theme: &Theme) -> Rect {
    let inner = body_block((true, hosted), theme).inner(panel);
    let column = terminal_text_area(inner, false);
    Rect {
        x: column.x,
        width: column.width,
        ..rows
    }
}

/// Draw the open plan over `area`, the chat panel, on the detail view's
/// `rows` in the chat's text column, and publish its prose rows. `None`
/// when no plan is open.
pub(super) fn draw_plan(
    frame: &mut Frame,
    (area, rows, hosted): (Rect, Rect, bool),
    app: &mut App,
    theme: &Theme,
) -> Option<DrawnPlan> {
    let measure = Measure::prose(app.prose_measure(), theme);
    let rows = text_column(area, rows, hosted, theme);
    let (doc, scroll) = app.plan_view_mut()?;
    ensure_layout(doc, usize::from(rows.width), measure, theme);
    let total = doc.layout.lines.len();
    let visible = usize::from(rows.height);
    let max_scroll = total.saturating_sub(visible);
    if doc.scroll_to_focus {
        doc.scroll_to_focus = false;
        if let Some(&row) = doc.focus.and_then(|index| doc.layout.todo_rows.get(index)) {
            *scroll = row.saturating_sub(FOCUS_CONTEXT);
        }
    }
    let first = (*scroll).min(max_scroll);
    *scroll = first;
    let end = (first + visible).min(total);
    let shown = end - first;

    let footer = footer_line((first, shown, total), theme);
    frame.render_widget(Clear, area);
    frame.render_widget(body_block((true, hosted), theme).title_bottom(footer), area);

    let prose_rows = draw_rows(
        frame.buffer_mut(),
        (&doc.layout.lines, &doc.layout.prose),
        (first, end),
        rows,
        (measure, theme),
    );
    // The plan covers the chat, whose prose rows no longer show.
    app.set_prose_rows(prose_rows);
    let view = ScrollView {
        total,
        visible: shown,
        first,
    };
    render_scrollbar(frame, area, rows, view, true, theme);
    Some(DrawnPlan {
        max_scroll,
        shown,
        first,
        view,
        column: rows,
    })
}

/// The footer: the rows shown out of all.
fn footer_line((first, shown, total): (usize, usize, usize), theme: &Theme) -> Line<'static> {
    Line::from(Span::styled(
        format!(" {}-{}/{total} ", (first + 1).min(total), first + shown),
        Style::default().fg(theme.colors.muted),
    ))
}

/// Draw rows `first..end` of the laid-out `lines` (with their `prose`
/// indents) on `rows`, and return the prose rows.
fn draw_rows(
    buffer: &mut Buffer,
    (lines, prose): (&[Line<'static>], &[Option<u16>]),
    (first, end): (usize, usize),
    rows: Rect,
    (measure, theme): (Measure, &Theme),
) -> Vec<ProseRow> {
    let mut prose_rows = Vec::new();
    for (offset, index) in (first..end).enumerate() {
        let (Some(line), Some(prose)) = (lines.get(index), prose.get(index)) else {
            continue;
        };
        let row = Rect {
            y: rows.y + u16::try_from(offset).unwrap_or(u16::MAX),
            height: 1,
            ..rows
        };
        Paragraph::new(line.clone()).render(row, buffer);
        let Some(indent) = prose.filter(|_| line.width() > 0) else {
            continue;
        };
        let indent = indent.min(row.width);
        let (width, code) = if measure.is_grid() {
            prose_columns(line, row, theme)
        } else {
            pack_prose(buffer, line, (row, indent), measure, theme)
        };
        prose_rows.push(ProseRow::new(buffer, (row.x, row.y), (width, indent), code));
    }
    prose_rows
}

#[cfg(test)]
mod tests;
