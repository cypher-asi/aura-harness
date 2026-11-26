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
//! │ 1-24/80  Esc close  s source  ↑↓ PgUp/PgDn scroll         │
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
        for todo in &doc.todos {
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
    }

    if !doc.body.trim().is_empty() {
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
    while laid.lines.last().is_some_and(|line| line.width() == 0) {
        laid.lines.pop();
        laid.prose.pop();
    }
    laid
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

    let muted = Style::default().fg(theme.colors.muted);
    let footer = Line::from(vec![
        Span::styled(
            format!(" {}-{}/{total} ", (first + 1).min(total), first + shown),
            muted,
        ),
        Span::styled("Esc close  s source  ↑↓ PgUp/PgDn scroll ", muted),
    ]);
    frame.render_widget(Clear, area);
    frame.render_widget(body_block((true, hosted), theme).title_bottom(footer), area);

    let buffer = frame.buffer_mut();
    let mut prose_rows = Vec::new();
    for (offset, index) in (first..end).enumerate() {
        let (Some(line), Some(prose)) = (doc.layout.lines.get(index), doc.layout.prose.get(index))
        else {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::UiCommand;
    use std::path::Path;

    const PLAN: &str = "---\nname: Read plans\noverview: Show plans as documents.\ntodos:\n\
        - id: one\n  content: first\n  status: completed\n\
        - id: two\n  content: second\n  status: pending\n---\n\n\
        # Read plans\n\nShow plans as documents.\n\n## Todos\n\n- [x] first\n- [ ] second\n\n\
        <!-- end of generated header: edit below -->\n\n\
        ## Why\n\nPlans read like a diff in `src/a.rs` today.\n\n\
        ```rust\nfn main() {}\n```\n";

    fn texts(lines: &[Line<'_>]) -> Vec<String> {
        lines
            .iter()
            .map(|line| line.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    #[test]
    fn a_plan_lays_out_as_a_document_without_its_source() {
        let doc = PlanDoc::parse(PLAN, Path::new("/x/a.plan.md"), ".aura/plans/a.plan.md");
        let theme = Theme::cyber();
        let laid = layout(&doc, 60, Measure::GRID, &theme);
        let rows = texts(&laid.lines);
        let all = rows.join("\n");
        assert_eq!(rows[0], "Read plans", "{all}");
        assert!(all.contains("Show plans as documents."), "{all}");
        assert!(all.contains("TODOS  1/2"), "{all}");
        assert!(all.contains("Why"), "{all}");
        assert!(all.contains("fn main() {}"), "{all}");
        for source in ["---", "name:", "<!--", "- [x]", "```"] {
            assert!(
                !rows.iter().any(|row| row.starts_with(source)),
                "{source} in {all}"
            );
        }
        let todos: Vec<&str> = laid
            .todo_rows
            .iter()
            .map(|&row| rows[row].as_str())
            .collect();
        assert_eq!(todos, ["☒ first", "☐ second"]);
        assert_eq!(laid.prose.len(), laid.lines.len());
        assert_eq!(laid.prose[0], Some(0), "the title is prose");
        assert_eq!(
            laid.prose[laid.todo_rows[0]],
            Some(TODO_INDENT),
            "todo text after its box"
        );
        let rule = rows.iter().position(|row| row.starts_with('─')).unwrap();
        assert_eq!(laid.prose[rule], None, "the rule stays on the grid");
        let code = rows.iter().position(|row| row.contains("fn main")).unwrap();
        assert_eq!(laid.prose[code], None, "code stays on the grid");
    }

    #[test]
    fn titles_that_look_like_markdown_stay_text() {
        assert_eq!(escape_heading("# not a heading"), "\\# not a heading");
        assert_eq!(escape_heading("1. first"), "1\\. first");
        assert_eq!(escape_heading("2024 plans"), "2024 plans");
        assert_eq!(escape_heading("Plain"), "Plain");
    }

    fn screen(app: &mut App) -> String {
        screen_at(app, 120)
    }

    fn screen_at(app: &mut App, width: u16) -> String {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, 30)).unwrap();
        terminal
            .draw(|frame| super::super::render(frame, app, &Theme::cyber()))
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

    fn app_with(text: &str) -> (App, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".aura/plans")).unwrap();
        std::fs::write(dir.path().join(".aura/plans/a.plan.md"), text).unwrap();
        let mut app = App::new();
        app.process_command(UiCommand::SetSandboxRoot(dir.path().to_path_buf()));
        (app, dir)
    }

    #[test]
    fn an_open_plan_reads_as_a_document_with_prose_rows() {
        let (mut app, _dir) = app_with(PLAN);
        app.open_plan_at(".aura/plans/a.plan.md", None);
        let text = screen(&mut app);
        assert!(text.contains("PLAN Read plans"), "{text}");
        assert!(text.contains("1/2 TODOS"), "{text}");
        assert!(text.contains("☒ first"), "{text}");
        assert!(text.contains("Plans read like a diff"), "{text}");
        assert!(text.contains("s source"), "{text}");
        for source in ["name:", "<!--", "- [x]"] {
            assert!(!text.contains(source), "{source} shows: {text}");
        }
        assert!(
            app.prose_rows().len() >= 4,
            "title, overview, todos and body are prose"
        );
    }

    /// Every character as wide as a cell, but not on the grid.
    #[allow(clippy::cast_precision_loss)]
    fn narrow(text: &str, _: ratatui::style::Modifier) -> f32 {
        text.chars().count() as f32 * 0.8
    }

    #[test]
    fn a_proportional_face_gets_the_plans_prose() {
        let (mut app, _dir) = app_with(PLAN);
        app.set_prose_measure(narrow);
        app.open_plan_at(".aura/plans/a.plan.md", None);
        screen(&mut app);
        let rows = app.prose_rows();
        assert!(rows.len() >= 4, "{rows:?}");
        assert!(
            rows.iter().any(|row| !row.code.is_empty()),
            "inline code stays monospace: {rows:?}"
        );
    }

    #[test]
    fn a_plan_opened_at_a_todo_scrolls_to_it_once() {
        let todos: String = (0..60)
            .map(|i| format!("- id: t{i}\n  content: todo number {i}\n"))
            .collect();
        let (mut app, _dir) = app_with(&format!("---\nname: Long\ntodos:\n{todos}---\n"));
        app.open_plan_at(".aura/plans/a.plan.md", Some("t40"));
        let text = screen(&mut app);
        assert!(text.contains("todo number 40"), "{text}");
        assert!(!text.contains("todo number 10\n"), "{text}");
        let scroll = app.detail_scroll();
        assert!(scroll > 0);

        app.handle_key(crossterm::event::KeyEvent::from(
            crossterm::event::KeyCode::Up,
        ));
        screen(&mut app);
        assert_eq!(app.detail_scroll(), scroll - 1, "scrolling is not undone");
    }

    #[test]
    fn s_shows_the_plans_source() {
        let (mut app, _dir) = app_with(PLAN);
        app.open_plan_at(".aura/plans/a.plan.md", Some("two"));
        screen(&mut app);
        app.handle_key(crossterm::event::KeyEvent::from(
            crossterm::event::KeyCode::Char('s'),
        ));
        let Some(crate::app::DetailView::File(view)) = app.detail_view() else {
            panic!("s opens the file view");
        };
        assert_eq!(view.vim.line(), 19, "on the todo's checklist item");
    }

    #[test]
    fn a_plans_text_sits_in_the_chats_column() {
        let long = "A paragraph long enough to wrap across the chat's whole column, \
                    so its rows reach the right edge of the text. "
            .repeat(6);
        let plan = format!("---\nname: Read plans\n---\n\n{long}\n");
        for width in [80, 200] {
            let (mut app, _dir) = app_with(&plan);
            app.add_message(crate::components::Message::new(
                crate::components::MessageRole::Assistant,
                &long,
            ));
            screen_at(&mut app, width);
            let chat = app.areas.chat_text.expect("the chat is drawn");

            app.open_plan_at(".aura/plans/a.plan.md", None);
            screen_at(&mut app, width);
            let rows = app.prose_rows();
            assert!(!rows.is_empty(), "width {width}");
            let left = rows.iter().map(|row| row.x).min().unwrap();
            let right = rows.iter().map(|row| row.x + row.width).max().unwrap();
            assert_eq!(left, chat.x, "starts where the chat does at width {width}");
            assert!(
                right <= chat.right(),
                "{right} past the chat's edge {} at width {width}",
                chat.right()
            );
            assert!(
                right + 12 >= chat.right(),
                "{right} fills the chat's column up to {} at width {width}",
                chat.right()
            );
        }
    }
}
