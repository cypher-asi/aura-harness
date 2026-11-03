//! A record or sandbox file shown in place of the chat panel.
//!
//! The view covers exactly the chat panel's rectangle, scrolls by source
//! line, and hard-wraps long lines so code keeps its indentation.

use super::overlays::{record_detail_lines, record_status};
use super::panels::{render_scrollbar, ScrollView};
use super::text::display_width;
use crate::app::file_view::{FileBody, FileView};
use crate::app::DetailView;
use crate::components::highlight_code_line;
use crate::{App, Theme};
use ratatui::{
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};
use unicode_width::UnicodeWidthChar;

const TAB: &str = "    ";

/// Draw the open record or file over `area`, the chat panel.
pub(super) fn render_detail_view(frame: &mut Frame, area: Rect, app: &mut App, theme: &Theme) {
    let rows = Rect {
        x: area.x.saturating_add(2),
        y: area.y.saturating_add(1),
        width: area.width.saturating_sub(4),
        height: area.height.saturating_sub(2),
    };
    let Some((max_scroll, shown)) = draw_detail(frame, area, rows, app, theme) else {
        return;
    };
    app.set_detail_bounds(max_scroll, shown.saturating_sub(1));
}

/// Draw the view into `area` with its text in `rows`, returning the
/// largest useful scroll and how many source lines are on screen.
fn draw_detail(
    frame: &mut Frame,
    area: Rect,
    rows: Rect,
    app: &App,
    theme: &Theme,
) -> Option<(usize, usize)> {
    let body = match app.detail_view()? {
        DetailView::Record => {
            let record = app.selected_record_data()?;
            let (icon, color) = record_status(record, theme);
            Body {
                title: format!(" Record #{} {icon} ", record.seq),
                color,
                lines: Source::Styled(record_detail_lines(
                    record,
                    rows.width.saturating_add(2),
                    theme,
                )),
            }
        }
        DetailView::File(file) => Body {
            title: format!(" {} ", file.title),
            color: theme.colors.primary,
            lines: file_source(file, theme),
        },
    };

    let width = usize::from(rows.width).max(1);
    let visible = usize::from(rows.height);
    let total = body.lines.len();
    let max_scroll = last_page_start(&body.lines, width, visible);
    let first = app.detail_scroll().min(max_scroll);
    let (lines, shown) = body.lines.visible(first, width, visible, theme);

    let muted = Style::default().fg(theme.colors.muted);
    let footer = Line::from(vec![
        Span::styled(
            format!(" {}-{}/{total} ", (first + 1).min(total), first + shown),
            muted,
        ),
        Span::styled("Esc close  ↑↓ PgUp/PgDn scroll ", muted),
    ]);
    let block = Block::default()
        .title(body.title.as_str())
        .title_bottom(footer)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(body.color));
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);
    frame.render_widget(Paragraph::new(lines), rows);
    render_scrollbar(
        frame,
        area,
        rows,
        ScrollView {
            total,
            visible: shown,
            first,
        },
        true,
        theme,
    );
    Some((max_scroll, shown))
}

struct Body<'a> {
    title: String,
    color: Color,
    lines: Source<'a>,
}

/// The lines of a detail view, before wrapping.
enum Source<'a> {
    /// Already styled lines (a record).
    Styled(Vec<Line<'a>>),
    /// Source text highlighted on demand, with a line-number gutter.
    Code {
        language: String,
        lines: Vec<&'a str>,
        gutter: usize,
    },
}

fn file_source<'a>(file: &'a FileView, theme: &Theme) -> Source<'a> {
    match &file.body {
        FileBody::Text(text) if text.is_empty() => Source::Styled(vec![Line::from(Span::styled(
            "Empty file",
            Style::default().fg(theme.colors.muted),
        ))]),
        FileBody::Text(text) => {
            let lines: Vec<&str> = text.lines().collect();
            Source::Code {
                language: file.extension(),
                gutter: lines.len().max(1).to_string().len(),
                lines,
            }
        }
        FileBody::Note(note) => Source::Styled(vec![Line::from(Span::styled(
            note.clone(),
            Style::default().fg(theme.colors.muted),
        ))]),
    }
}

impl<'a> Source<'a> {
    fn len(&self) -> usize {
        match self {
            Self::Styled(lines) => lines.len(),
            Self::Code { lines, .. } => lines.len(),
        }
    }

    /// Columns taken by the line-number gutter, including its separator.
    fn gutter_width(&self) -> usize {
        match self {
            Self::Styled(_) => 0,
            Self::Code { gutter, .. } => gutter + 3,
        }
    }

    /// Display width of line `index`, not counting the gutter.
    fn line_width(&self, index: usize) -> usize {
        match self {
            Self::Styled(lines) => lines[index].width(),
            Self::Code { lines, .. } => display_width(&sanitize(lines[index])),
        }
    }

    /// Wrapped rows for lines `first..` that fit in `visible` rows, and how
    /// many source lines they came from.
    fn visible(
        &self,
        first: usize,
        width: usize,
        visible: usize,
        theme: &Theme,
    ) -> (Vec<Line<'a>>, usize) {
        let indent = " ".repeat(self.gutter_width());
        let mut out: Vec<Line<'a>> = Vec::new();
        let mut shown = 0;
        for index in first..self.len() {
            if out.len() >= visible {
                break;
            }
            let line = match self {
                Self::Styled(lines) => lines[index].clone(),
                Self::Code {
                    language,
                    lines,
                    gutter,
                } => {
                    let mut spans = vec![Span::styled(
                        format!("{:>gutter$} │ ", index + 1),
                        Style::default().fg(theme.colors.muted),
                    )];
                    spans.extend(highlight_code_line(
                        language,
                        &sanitize(lines[index]),
                        theme,
                    ));
                    Line::from(spans)
                }
            };
            out.extend(hard_wrap(line, width, &indent));
            shown += 1;
        }
        out.truncate(visible);
        (out, shown)
    }
}

/// First line to show so the last line sits at the bottom of `visible`
/// rows, measuring backwards from the end.
fn last_page_start(source: &Source<'_>, width: usize, visible: usize) -> usize {
    let gutter = source.gutter_width();
    let text_width = width.saturating_sub(gutter).max(1);
    let mut rows = 0;
    let mut start = source.len();
    while start > 0 {
        let needed = source.line_width(start - 1).div_ceil(text_width).max(1);
        if rows + needed > visible {
            break;
        }
        rows += needed;
        start -= 1;
    }
    start.min(source.len().saturating_sub(1))
}

/// Tabs as spaces and other control characters as `�`, so the terminal
/// draws exactly what is measured.
fn sanitize(line: &str) -> String {
    if !line.chars().any(char::is_control) {
        return line.to_string();
    }
    line.chars()
        .map(|c| match c {
            '\t' => TAB.to_string(),
            c if c.is_control() => '\u{fffd}'.to_string(),
            c => c.to_string(),
        })
        .collect()
}

/// Break `line` into rows at most `width` columns wide, character by
/// character, starting each continuation row with `indent`.
fn hard_wrap<'a>(line: Line<'a>, width: usize, indent: &str) -> Vec<Line<'a>> {
    if line.width() <= width {
        return vec![line];
    }
    let indent_width = display_width(indent);
    let mut rows: Vec<Line<'a>> = Vec::new();
    let mut current: Vec<Span<'a>> = Vec::new();
    let mut used = 0;
    for span in line.spans {
        let mut piece = String::new();
        for c in span.content.chars() {
            let w = c.width().unwrap_or(0);
            if used + w > width && used > indent_width {
                if !piece.is_empty() {
                    current.push(Span::styled(std::mem::take(&mut piece), span.style));
                }
                rows.push(Line::from(std::mem::take(&mut current)));
                current.push(Span::raw(indent.to_string()));
                used = indent_width;
            }
            piece.push(c);
            used += w;
        }
        if !piece.is_empty() {
            current.push(Span::styled(piece, span.style));
        }
    }
    rows.push(Line::from(current));
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(line: &Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn hard_wrap_keeps_indentation_and_indents_continuations() {
        let rows = hard_wrap(Line::from("    let x = 1234567;"), 10, "  ");
        let rows: Vec<String> = rows.iter().map(text).collect();
        assert_eq!(rows, ["    let x ", "  = 123456", "  7;"]);
    }

    #[test]
    fn short_line_is_untouched() {
        let rows = hard_wrap(Line::from("  ok"), 10, "");
        assert_eq!(rows.len(), 1);
        assert_eq!(text(&rows[0]), "  ok");
    }

    #[test]
    fn sanitize_expands_tabs_and_masks_controls() {
        assert_eq!(sanitize("\tx\u{1b}"), "    x\u{fffd}");
    }

    fn screen(app: &mut App) -> Vec<Vec<String>> {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 30)).unwrap();
        terminal
            .draw(|frame| super::super::render(frame, app, &Theme::cyber()))
            .unwrap();
        let buffer = terminal.backend().buffer();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol().to_string())
                    .collect()
            })
            .collect()
    }

    /// Top-left and bottom-right corners of the box whose top border
    /// starts with `title`.
    fn box_corners(rows: &[Vec<String>], title: &str) -> ((usize, usize), (usize, usize)) {
        let len = title.chars().count();
        let (top, left) = rows
            .iter()
            .enumerate()
            .find_map(|(y, row)| {
                let x = (0..row.len().saturating_sub(len))
                    .find(|&x| row[x] == "┌" && row[x + 1..=x + len].concat() == title)?;
                Some((y, x))
            })
            .unwrap_or_else(|| panic!("no box titled {title:?}"));
        let right = (left + 1..rows[top].len())
            .find(|&x| rows[top][x] == "┐")
            .unwrap();
        let bottom = (top + 1..rows.len())
            .find(|&y| rows[y][left] == "└")
            .unwrap();
        assert_eq!(rows[bottom][right], "┘");
        ((top, left), (bottom, right))
    }

    #[test]
    fn record_view_covers_exactly_the_chat_panel() {
        use crate::events::{RecordStatus, RecordSummary, UiCommand};
        use crossterm::event::{KeyCode, KeyEvent};

        let mut app = App::new();
        app.process_command(UiCommand::NewRecord(RecordSummary {
            seq: 60,
            timestamp: "06:16:19".to_string(),
            full_hash: "355d52f6".repeat(8),
            hash_suffix: String::new(),
            tx_kind: "Policy".to_string(),
            sender: "KERNEL".to_string(),
            message: "approved".to_string(),
            action_count: 0,
            effect_status: "-".to_string(),
            status: RecordStatus::Ok,
            info: String::new(),
            what: String::new(),
            group: String::new(),
            detail: String::new(),
            error_details: String::new(),
            tx_id: "a71e37b3".repeat(8),
            agent_id: String::new(),
            ts_ms: 0,
            policy: None,
        }));
        let chat = box_corners(&screen(&mut app), " CHAT ");

        while app.focus() != crate::app::PanelFocus::Records {
            app.handle_key(KeyEvent::from(KeyCode::Tab));
        }
        app.handle_key(KeyEvent::from(KeyCode::Enter));
        let rows = screen(&mut app);
        let text = rows
            .iter()
            .map(|r| r.concat())
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(box_corners(&rows, " Record #60 ✓ "), chat, "{text}");
        assert!(!text.contains(" CHAT "), "{text}");
        assert!(text.contains("Sequence:    60"), "{text}");
        assert!(text.contains("Esc close"), "{text}");

        app.handle_key(KeyEvent::from(KeyCode::Esc));
        let text = screen(&mut app)
            .iter()
            .map(|r| r.concat())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains(" CHAT "), "{text}");
    }

    #[test]
    fn file_view_numbers_lines_and_scrolls() {
        use crate::events::UiCommand;
        use crossterm::event::{KeyCode, KeyEvent};

        let dir = tempfile::tempdir().unwrap();
        let body: String = (1..=100).map(|n| format!("let x{n} = {n};\n")).collect();
        std::fs::write(dir.path().join("main.rs"), body).unwrap();
        let mut app = App::new();
        app.process_command(UiCommand::SetSandboxRoot(dir.path().to_path_buf()));
        while app.focus() != crate::app::PanelFocus::Sandbox {
            app.handle_key(KeyEvent::from(KeyCode::Tab));
        }
        app.handle_key(KeyEvent::from(KeyCode::Enter));

        let text = |app: &mut App| {
            screen(app)
                .iter()
                .map(|r| r.concat())
                .collect::<Vec<_>>()
                .join("\n")
        };
        let first = text(&mut app);
        assert!(first.contains(" main.rs "), "{first}");
        assert!(first.contains("  1 │ let x1 = 1;"), "{first}");

        app.handle_key(KeyEvent::from(KeyCode::End));
        let last = text(&mut app);
        assert!(last.contains("100 │ let x100 = 100;"), "{last}");
        assert!(last.contains("/100 "), "{last}");
        assert!(!last.contains("  1 │ "), "{last}");
    }

    #[test]
    fn last_page_counts_wrapped_rows() {
        let source = Source::Styled(vec![
            Line::from("a"),
            Line::from("b"),
            Line::from("cccccccccc"),
        ]);
        assert_eq!(last_page_start(&source, 5, 2), 2);
        assert_eq!(last_page_start(&source, 5, 3), 1);
        assert_eq!(last_page_start(&source, 5, 4), 0);
        assert_eq!(last_page_start(&source, 5, 1), 2);
    }
}
