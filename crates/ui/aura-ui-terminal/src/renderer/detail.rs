//! A record or sandbox file shown in place of the chat panel.
//!
//! The view covers exactly the chat panel's rectangle, scrolls by source
//! line, and hard-wraps long lines so code keeps its indentation. A file
//! has a vim cursor the view follows.

use super::chat_panel::chat_scrollbar;
use super::chrome::{body_block, render_scrollbar, ScrollView};
use super::header::HeaderInfo;
use super::inbox::thread_lines;
use super::overlays::{record_detail_lines, record_status};
use super::processes::{process_detail_lines, status_glyph};
use super::text::display_width;
use crate::app::file_view::{FileBody, FileView};
use crate::app::image_preview::ImageView;
use crate::app::scrollbar::ScrollTarget;
use crate::app::vim::{Align, Vim};
use crate::app::DetailView;
use crate::components::highlight_code_line;
use crate::components::syntax::{highlight_line, syntax_for_path};
use crate::components::truncate_record_info;
use crate::{App, Theme};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, Paragraph},
    Frame,
};
use unicode_width::UnicodeWidthChar;

const TAB: &str = "    ";

/// The chat panel's header while a view covers its body: what is open on
/// the left, its status and a close button on the right. `None` leaves the
/// CHAT | TERMINAL tabs.
///
/// ```text
/// │ RECORD #60               ✓ POLICY ✕ │
/// │ src/main.rs             100 LINES ✕ │
/// ```
pub(super) fn detail_header(app: &App, theme: &Theme) -> Option<(String, Option<HeaderInfo>)> {
    let status = |icon: &str, label: &str, color| HeaderInfo {
        text: format!("{icon} {}", label.to_uppercase()),
        short: icon.to_string(),
        color,
    };
    match app.detail_view()? {
        DetailView::Record => {
            let record = app.selected_record_data()?;
            let (icon, color) = record_status(record, theme);
            let info = status(icon, &record.tx_kind, color);
            Some((format!("RECORD #{}", record.seq), Some(info)))
        }
        DetailView::File(file) => {
            let info = match &file.body {
                FileBody::Text(text) => {
                    let lines = file.body.lines().len();
                    let unit = if lines == 1 { "LINE" } else { "LINES" };
                    (!text.is_empty()).then(|| HeaderInfo::count(lines, unit, theme.colors.muted))
                }
                FileBody::Note(_) => None,
            };
            Some((file.title.clone(), info))
        }
        DetailView::Process(process) => {
            let (icon, color) = status_glyph(process.status, theme);
            let title = format!(
                "{} {}",
                process.kind.label().to_uppercase(),
                truncate_record_info(&process.label, 40)
            );
            Some((title, Some(status(icon, process.status.label(), color))))
        }
        DetailView::Graph => Some(super::graph::graph_header(app, theme)),
        DetailView::Image(view) => {
            let info = HeaderInfo {
                text: view.info(),
                short: String::new(),
                color: theme.colors.muted,
            };
            Some((view.title.clone(), Some(info)))
        }
        DetailView::Thread(view) => {
            let count = app.thread_messages(&view.thread_id).len();
            let unit = if count == 1 { "MESSAGE" } else { "MESSAGES" };
            let title = format!("THREAD {}", truncate_record_info(&view.subject, 40));
            Some((
                title,
                Some(HeaderInfo::count(count, unit, theme.colors.muted)),
            ))
        }
        DetailView::Plan(doc) => {
            let (closed, total) = doc.progress();
            let title = format!("PLAN {}", truncate_record_info(&doc.title, 40));
            Some((
                title,
                Some(HeaderInfo::count(
                    format!("{closed}/{total}"),
                    "TODOS",
                    theme.colors.muted,
                )),
            ))
        }
    }
}

/// Draw the open record or file over `area`, the chat panel.
pub(super) fn render_detail_view(frame: &mut Frame, area: Rect, app: &mut App, theme: &Theme) {
    if app.detail_view().is_some() {
        app.set_chat_rows(None);
        app.set_chat_scrollbar(None, frame.buffer_mut());
    }
    if app.detail_view() == Some(&DetailView::Graph) {
        super::graph::render_graph(frame, area, app, theme);
        return;
    }
    let rows = Rect {
        x: area.x.saturating_add(2),
        y: area.y.saturating_add(1),
        width: area.width.saturating_sub(4),
        height: area.height.saturating_sub(2),
    };
    let hosted = app.host_chrome();
    if let Some(view) = app.image_view_mut() {
        let rect = draw_image(frame, (area, rows, hosted), view, theme);
        app.set_detail_image_rect(Some(rect));
        return;
    }
    // Not an image view: clear any stale detail image rect.
    app.set_detail_image_rect(None);
    // A plan's text sits in the chat's column, so selection and the
    // scrollbar follow that column rather than the detail view's rows.
    let mut text = rows;
    let bounds = if matches!(app.detail_view(), Some(DetailView::Plan(_))) {
        super::plan_doc::draw_plan(frame, (area, rows, hosted), app, theme).map(|plan| {
            text = plan.column;
            Drawn {
                max_scroll: plan.max_scroll,
                shown: plan.shown,
                top_row: plan.first,
                view: plan.view,
                thread_headers: Vec::new(),
            }
        })
    } else {
        match app.file_view_mut() {
            Some((file, scroll)) => {
                Some(draw_file(frame, (area, rows, hosted), file, scroll, theme))
            }
            None => draw_record(frame, (area, rows, hosted), app, theme),
        }
    };
    if let Some(mut drawn) = bounds {
        if matches!(app.detail_view(), Some(DetailView::Thread(_))) {
            app.set_thread_header_rows(std::mem::take(&mut drawn.thread_headers));
        }
        app.set_detail_bounds(drawn.max_scroll, drawn.shown);
        app.set_chat_rows(Some((text, drawn.top_row)));
        let scrollbar = chat_scrollbar(
            ScrollTarget::Detail,
            (area, text),
            drawn.view,
            drawn.max_scroll,
        );
        app.set_chat_scrollbar(scrollbar, frame.buffer_mut());
    }
}

/// How a record or file was drawn over the chat panel.
struct Drawn {
    /// Largest useful scroll, in source lines
    max_scroll: usize,
    /// Source lines on screen
    shown: usize,
    /// Wrapped row on top of the view
    top_row: usize,
    /// What its scrollbar shows
    view: ScrollView,
    /// For a thread, the wrapped row of each message header, from the
    /// top of the content, with the message's id
    thread_headers: Vec<(usize, u64)>,
}

/// Draw the highlighted record.
fn draw_record(
    frame: &mut Frame,
    (area, rows, hosted): (Rect, Rect, bool),
    app: &App,
    theme: &Theme,
) -> Option<Drawn> {
    let mut hint = "Esc close  ↑↓ PgUp/PgDn scroll ";
    let mut headers = Vec::new();
    let lines = match app.detail_view()? {
        DetailView::Record => {
            let record = app.selected_record_data()?;
            record_detail_lines(record, usize::from(rows.width), theme)
        }
        DetailView::Process(process) => {
            process_detail_lines(process, usize::from(rows.width), theme)
        }
        DetailView::Thread(view) => {
            let (lines, header_lines) = thread_lines(app, view, rows.width, theme);
            hint = "Esc close  n/p message  ⏎ fold  e/c all  ↑↓ scroll ";
            headers = header_lines;
            lines
        }
        // Drawn by `plan_doc::draw_plan`.
        DetailView::Plan(_) | DetailView::File(_) | DetailView::Graph | DetailView::Image(_) => {
            return None
        }
    };
    let source = Source::Styled(lines);
    let width = usize::from(rows.width).max(1);
    let thread_headers = headers
        .into_iter()
        .map(|(line, id)| (source.rows_before(line, width), id))
        .collect();
    let visible = usize::from(rows.height);
    let total = source.len();
    let max_scroll = last_page_start(&source, width, visible);
    let first = app.detail_scroll().min(max_scroll);
    let (lines, shown) = source.visible(first, width, visible, theme, None);

    let muted = Style::default().fg(theme.colors.muted);
    let footer = Line::from(vec![
        Span::styled(
            format!(" {}-{}/{total} ", (first + 1).min(total), first + shown),
            muted,
        ),
        Span::styled(hint, muted),
    ]);
    let view = ScrollView {
        total,
        visible: shown,
        first,
    };
    draw_frame(frame, (area, rows, hosted), footer, lines, view, theme);
    Some(Drawn {
        max_scroll,
        shown,
        top_row: source.rows_before(first, width),
        view,
        thread_headers,
    })
}

/// Draw the image as large as fits, centered in the panel.
/// Returns the absolute cell rect of the drawn image for the graphics layer.
fn draw_image(
    frame: &mut Frame,
    (area, rows, hosted): (Rect, Rect, bool),
    view: &mut ImageView,
    theme: &Theme,
) -> Rect {
    let lines = view.lines(rows.width, rows.height).to_vec();
    let width = lines.iter().map(Line::width).max().unwrap_or(0);
    let width = u16::try_from(width).unwrap_or(u16::MAX).min(rows.width);
    let height = u16::try_from(lines.len())
        .unwrap_or(u16::MAX)
        .min(rows.height);
    let centered = Rect {
        x: rows.x + (rows.width - width) / 2,
        y: rows.y + (rows.height - height) / 2,
        width,
        height,
    };
    let muted = Style::default().fg(theme.colors.muted);
    let footer = Line::from(Span::styled(" o open externally  Esc close ", muted));
    let block = body_block((true, hosted), theme).title_bottom(footer);
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);
    frame.render_widget(Paragraph::new(lines), centered);
    centered
}

/// Draw `file` with its cursor in view, moving `scroll` to follow it.
fn draw_file(
    frame: &mut Frame,
    (area, rows, hosted): (Rect, Rect, bool),
    file: &mut FileView,
    scroll: &mut usize,
    theme: &Theme,
) -> Drawn {
    let syntax = file.syntax();
    let mut source = file_source(&file.body, file.extension(), file.first_line, theme);
    let vim = &mut file.vim;
    let width = usize::from(rows.width).max(1);
    let visible = usize::from(rows.height);
    let total = source.len();
    let max_scroll = last_page_start(&source, width, visible);
    let align = vim.take_align();
    let first = fit(&source, align, vim.line(), *scroll, width, visible).min(max_scroll);
    *scroll = first;
    if let Source::Code { lines, .. } = &source {
        let last = first + fully_shown(&source, first, width, visible) - 1;
        vim.keep_within(first, last, lines);
    }
    let vim = &*vim;
    if let Source::Code {
        language,
        lines,
        highlighted,
        ..
    } = &mut source
    {
        if !is_pseudo_language(language) {
            // Each source line takes at least one row, so at most
            // `visible` lines starting at `first` reach the screen.
            let range = first..first.saturating_add(visible);
            let spans = file.highlight.spans(syntax, lines, range, sanitize, theme);
            *highlighted = Highlighted {
                first,
                lines: spans,
            };
        }
    }
    let (lines, shown) = source.visible(first, width, visible, theme, Some(vim));

    let muted = Style::default().fg(theme.colors.muted);
    let mut footer = Vec::new();
    if let Some(prompt) = vim.prompt() {
        footer.push(Span::styled(
            format!(" {}", prompt.display()),
            Style::default().fg(theme.colors.text),
        ));
        footer.push(Span::styled(
            "█ ",
            Style::default().fg(theme.colors.primary),
        ));
    } else if let Some(message) = vim.message() {
        footer.push(Span::styled(
            format!(" {message} "),
            Style::default().fg(theme.colors.warning),
        ));
    } else {
        if matches!(source, Source::Code { .. }) {
            footer.push(Span::styled(
                format!(
                    " Ln {}/{}, Col {} ",
                    vim.line() + file.first_line,
                    total + file.first_line - 1,
                    vim.col() + 1
                ),
                muted,
            ));
        }
        let keys = vim.pending_keys();
        if !keys.is_empty() {
            footer.push(Span::styled(
                format!("{keys} "),
                Style::default().fg(theme.colors.text),
            ));
        }
        footer.push(Span::styled(" q close  / search  :N line ", muted));
    }
    let view = ScrollView {
        total,
        visible: shown,
        first,
    };
    let footer = Line::from(footer);
    draw_frame(frame, (area, rows, hosted), footer, lines, view, theme);
    Drawn {
        max_scroll,
        shown,
        top_row: source.rows_before(first, width),
        view,
        thread_headers: Vec::new(),
    }
}

/// The chat panel's body border around `lines`, with `footer` on its
/// bottom edge; the header bar above names what is shown.
fn draw_frame(
    frame: &mut Frame,
    (area, rows, hosted): (Rect, Rect, bool),
    footer: Line<'_>,
    lines: Vec<Line<'_>>,
    view: ScrollView,
    theme: &Theme,
) {
    let block = body_block((true, hosted), theme).title_bottom(footer);
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);
    frame.render_widget(Paragraph::new(lines), rows);
    render_scrollbar(frame, area, rows, view, true, theme);
}

/// First line to show so the cursor line sits where `align` asks,
/// starting from the current `scroll`.
fn fit(
    source: &Source<'_>,
    align: Align,
    cursor: usize,
    scroll: usize,
    width: usize,
    visible: usize,
) -> usize {
    let cursor = cursor.min(source.len().saturating_sub(1));
    let text_width = source.text_width(width);
    let center = || {
        let own = source.rows(cursor, text_width);
        start_ending_at(source, cursor, width, own + visible.saturating_sub(own) / 2)
    };
    let half = visible / 2;
    match align {
        Align::View => scroll,
        Align::Top => cursor,
        Align::Center => center(),
        Align::Bottom => start_ending_at(source, cursor, width, visible),
        Align::Cursor if cursor < scroll => {
            if scroll - cursor > half {
                center()
            } else {
                cursor
            }
        }
        Align::Cursor => {
            let needed = start_ending_at(source, cursor, width, visible);
            if needed <= scroll {
                scroll
            } else if needed - scroll > half {
                center()
            } else {
                needed
            }
        }
    }
}

/// How many lines from `first` fit whole in `visible` rows, at least 1.
fn fully_shown(source: &Source<'_>, first: usize, width: usize, visible: usize) -> usize {
    let text_width = source.text_width(width);
    let mut used = 0;
    let mut count = 0;
    for index in first..source.len() {
        used += source.rows(index, text_width);
        if used > visible {
            break;
        }
        count += 1;
    }
    count.max(1)
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
        /// Number of the first line in the gutter.
        first_line: usize,
        /// Lines already highlighted in the context of the whole file;
        /// others are highlighted on their own when drawn.
        highlighted: Highlighted,
    },
}

/// Highlighted spans of consecutive source lines, from line `first`.
#[derive(Default)]
struct Highlighted {
    first: usize,
    lines: Vec<Vec<Span<'static>>>,
}

impl Highlighted {
    fn get(&self, index: usize) -> Option<Vec<Span<'static>>> {
        self.lines.get(index.checked_sub(self.first)?).cloned()
    }
}

fn file_source<'a>(
    body: &'a FileBody,
    language: String,
    first_line: usize,
    theme: &Theme,
) -> Source<'a> {
    match body {
        FileBody::Text(text) if text.is_empty() => Source::Styled(vec![Line::from(Span::styled(
            "Empty file",
            Style::default().fg(theme.colors.muted),
        ))]),
        FileBody::Text(_) => {
            let lines = body.lines();
            let last = first_line.max(1) + lines.len().max(1) - 1;
            Source::Code {
                language,
                gutter: last.to_string().len(),
                lines,
                first_line: first_line.max(1),
                highlighted: Highlighted::default(),
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

    /// Columns left for text in each row of a view `width` wide.
    fn text_width(&self, width: usize) -> usize {
        width.saturating_sub(self.gutter_width()).max(1)
    }

    /// Display width of line `index`, not counting the gutter.
    fn line_width(&self, index: usize) -> usize {
        match self {
            Self::Styled(lines) => lines[index].width(),
            Self::Code { lines, .. } => display_width(&sanitize(lines[index])),
        }
    }

    /// Rows line `index` wraps to with `text_width` columns per row.
    fn rows(&self, index: usize, text_width: usize) -> usize {
        self.line_width(index).div_ceil(text_width).max(1)
    }

    /// Wrapped rows of the lines before `first`, in a view `width` wide.
    fn rows_before(&self, first: usize, width: usize) -> usize {
        let text_width = self.text_width(width);
        (0..first.min(self.len()))
            .map(|index| self.rows(index, text_width))
            .sum()
    }

    /// Wrapped rows for lines `first..` that fit in `visible` rows, and how
    /// many source lines they came from. With `vim`, code shows its cursor
    /// and search matches.
    fn visible(
        &self,
        first: usize,
        width: usize,
        visible: usize,
        theme: &Theme,
        vim: Option<&Vim>,
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
                    first_line,
                    highlighted,
                } => {
                    let on_cursor = vim.is_some_and(|vim| vim.line() == index);
                    let number = if on_cursor {
                        Style::default()
                            .fg(theme.colors.primary)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(theme.colors.muted)
                    };
                    let mut spans = vec![Span::styled(
                        format!("{:>gutter$} │ ", index + first_line),
                        number,
                    )];
                    let code = highlighted.get(index).unwrap_or_else(|| {
                        highlight_view_line(language, &sanitize(lines[index]), theme)
                    });
                    match vim {
                        Some(vim) => spans.extend(mark(code, lines[index], vim, on_cursor, theme)),
                        None => spans.extend(code),
                    }
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

/// Whether `language` is one of the view-only formats below rather than
/// a source language.
fn is_pseudo_language(language: &str) -> bool {
    matches!(language, "grep" | "log")
}

/// Highlight one line of a detail view. Besides source languages, `grep`
/// colors `path:line:` before highlighting the match by the path's
/// extension, and `log` colors errors and warnings in command output.
fn highlight_view_line(language: &str, line: &str, theme: &Theme) -> Vec<Span<'static>> {
    match language {
        "grep" => grep_line(line, theme),
        "log" => vec![log_line(line, theme)],
        _ => highlight_code_line(language, line, theme),
    }
}

/// `path:line:text`, as search tools print matches.
fn grep_line(line: &str, theme: &Theme) -> Vec<Span<'static>> {
    let split = line.find(':').and_then(|colon| {
        let rest = &line[colon + 1..];
        let digits = rest.find(|c: char| !c.is_ascii_digit())?;
        (digits > 0 && rest[digits..].starts_with(':')).then_some((colon, colon + 1 + digits))
    });
    let Some((colon, end)) = split else {
        return vec![Span::styled(
            line.to_string(),
            Style::default().fg(theme.colors.secondary),
        )];
    };
    let path = &line[..colon];
    let syntax = syntax_for_path(std::path::Path::new(path), "");
    let muted = Style::default().fg(theme.colors.muted);
    let mut spans = vec![
        Span::styled(
            path.to_string(),
            Style::default().fg(theme.colors.secondary),
        ),
        Span::styled(":".to_string(), muted),
        Span::styled(line[colon + 1..end].to_string(), muted),
        Span::styled(":".to_string(), muted),
    ];
    spans.extend(highlight_line(syntax, &line[end + 1..], theme));
    spans
}

/// Command output: errors and warnings stand out, `-->` locations and
/// the `stderr:` divider are subdued.
fn log_line(line: &str, theme: &Theme) -> Span<'static> {
    let trimmed = line.trim_start();
    let lower = trimmed.to_lowercase();
    let color =
        if lower.starts_with("error") || lower.contains("panicked") || lower.contains("failed") {
            theme.colors.error
        } else if lower.starts_with("warning") {
            theme.colors.warning
        } else if trimmed.starts_with("-->")
            || trimmed.starts_with("stderr:")
            || trimmed.starts_with('|')
        {
            theme.colors.secondary
        } else if lower.starts_with("test ") && lower.ends_with(" ok") || lower.contains("finished")
        {
            theme.colors.success
        } else {
            theme.colors.foreground
        };
    Span::styled(line.to_string(), Style::default().fg(color))
}

/// Restyle the highlighted `spans` of `raw` to show search matches and,
/// on the cursor line, the cursor cell.
fn mark<'a>(
    mut spans: Vec<Span<'a>>,
    raw: &str,
    vim: &Vim,
    on_cursor: bool,
    theme: &Theme,
) -> Vec<Span<'a>> {
    let found = Style::default()
        .bg(theme.colors.selection)
        .fg(theme.colors.on_selection);
    let mut ranges: Vec<(usize, usize, Style)> = vim
        .matches(raw)
        .into_iter()
        .map(|(start, end)| (shown_index(raw, start), shown_index(raw, end), found))
        .collect();
    if on_cursor {
        let cursor = Style::default().add_modifier(Modifier::REVERSED);
        let start = shown_index(raw, vim.col());
        let end = shown_index(raw, vim.col() + 1);
        if end == start {
            spans.push(Span::raw(" "));
        }
        ranges.push((start, end.max(start + 1), cursor));
    }
    if ranges.is_empty() {
        return spans;
    }
    let mut out = Vec::new();
    let mut index = 0;
    for span in spans {
        let mut piece = String::new();
        let mut piece_style = span.style;
        for c in span.content.chars() {
            let style = ranges
                .iter()
                .rev()
                .find(|(start, end, _)| (*start..*end).contains(&index))
                .map_or(span.style, |(_, _, extra)| span.style.patch(*extra));
            if style != piece_style && !piece.is_empty() {
                out.push(Span::styled(std::mem::take(&mut piece), piece_style));
            }
            piece_style = style;
            piece.push(c);
            index += 1;
        }
        if !piece.is_empty() {
            out.push(Span::styled(piece, piece_style));
        }
    }
    out
}

/// Index in the sanitized line of character `col` of `raw`.
fn shown_index(raw: &str, col: usize) -> usize {
    raw.chars()
        .take(col)
        .map(|c| if c == '\t' { TAB.len() } else { 1 })
        .sum()
}

/// First line to show so the last line sits at the bottom of `visible`
/// rows, measuring backwards from the end.
fn last_page_start(source: &Source<'_>, width: usize, visible: usize) -> usize {
    match source.len() {
        0 => 0,
        len => start_ending_at(source, len - 1, width, visible),
    }
}

/// First line to show so line `last` ends at the bottom of `rows` rows.
fn start_ending_at(source: &Source<'_>, last: usize, width: usize, rows: usize) -> usize {
    let text_width = source.text_width(width);
    let mut used = 0;
    let mut start = last + 1;
    while start > 0 {
        let needed = source.rows(start - 1, text_width);
        if used + needed > rows {
            break;
        }
        used += needed;
        start -= 1;
    }
    start.min(last)
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

    /// Corners of the chat panel's body, the box right below the header
    /// bar holding its CHAT | TERMINAL tabs.
    fn chat_corners(rows: &[Vec<String>]) -> ((usize, usize), (usize, usize)) {
        let (top, left) = rows
            .iter()
            .enumerate()
            .find_map(|(y, row)| {
                let text = row.concat();
                if ![" TERMINAL ", "│TERMINAL│", " TRM "]
                    .iter()
                    .any(|tab| text.contains(tab))
                {
                    return None;
                }
                let chat =
                    (0..row.len().saturating_sub(4)).find(|&x| row[x..x + 4].concat() == "CHAT")?;
                let x = (0..chat).rev().find(|&x| row[x] == "│")?;
                Some((y + 2, x))
            })
            .expect("no chat header bar");
        assert_eq!(rows[top][left], "┌");
        let right = (left + 1..rows[top].len())
            .find(|&x| rows[top][x] == "┐")
            .unwrap();
        let bottom = (top + 1..rows.len())
            .find(|&y| rows[y][left] == "└")
            .unwrap();
        ((top, left), (bottom, right))
    }

    #[test]
    fn record_view_covers_exactly_the_chat_body() {
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
        let chat = chat_corners(&screen(&mut app));

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
        let ((top, left), (bottom, right)) = chat;
        let corners = [(top, left), (top, right), (bottom, left), (bottom, right)]
            .map(|(y, x)| rows[y][x].as_str());
        assert_eq!(corners, ["┌", "┐", "└", "┘"], "{text}");
        let header: String = rows[top - 2][left..=right].concat();
        assert!(header.starts_with("│ RECORD #60 "), "{header}");
        assert!(header.ends_with(" ✓ POLICY ⤢ ✕ │"), "{header}");
        assert!(!text.contains("TERMINAL"), "the tabs give way: {text}");
        let border: String = rows[top][left..=right].concat();
        assert!(border.trim_matches(['┌', '─', '┐']).is_empty(), "{border}");
        assert!(text.contains("  Sequence      60"), "{text}");
        assert!(text.contains("Esc close"), "{text}");

        app.handle_key(KeyEvent::from(KeyCode::Esc));
        let rows = screen(&mut app);
        assert_eq!(chat_corners(&rows), chat, "closing brings the tabs back");

        app.handle_key(KeyEvent::from(KeyCode::Enter));
        screen(&mut app);
        let at = |column: usize| {
            move |kind| crossterm::event::MouseEvent {
                kind,
                column: u16::try_from(column).unwrap(),
                row: u16::try_from(top - 2).unwrap(),
                modifiers: crossterm::event::KeyModifiers::empty(),
            }
        };
        let expand = at(right - 4);
        app.handle_mouse(expand(crossterm::event::MouseEventKind::Moved));
        assert_eq!(app.hover(), Some(crate::app::HoverTarget::ExpandChat));
        app.handle_mouse(expand(crossterm::event::MouseEventKind::Down(
            crossterm::event::MouseButton::Left,
        )));
        assert!(app.chat_expanded(), "the ⤢ expands the chat");
        assert!(app.detail_view().is_some(), "with the record still on it");
        let rows = screen(&mut app);
        let header = &rows[top - 2];
        let wide_left = header.iter().position(|cell| cell == "│").unwrap();
        let wide_right = header.iter().rposition(|cell| cell == "│").unwrap();
        assert!(wide_left < left && wide_right > right, "the chat widens");
        let header: String = header[wide_left..=wide_right].concat();
        assert!(header.starts_with("│ RECORD #60 "), "{header}");
        assert!(header.ends_with(" ✓ POLICY ⤡ ✕ │"), "{header}");
        app.handle_mouse(at(wide_right - 4)(crossterm::event::MouseEventKind::Down(
            crossterm::event::MouseButton::Left,
        )));
        assert!(!app.chat_expanded(), "the ⤡ restores it");
        screen(&mut app);

        let close = at(right - 2);
        app.handle_mouse(close(crossterm::event::MouseEventKind::Moved));
        assert_eq!(app.hover(), Some(crate::app::HoverTarget::CloseDetail));
        app.handle_mouse(close(crossterm::event::MouseEventKind::Down(
            crossterm::event::MouseButton::Left,
        )));
        assert!(app.detail_view().is_none(), "the ✕ closes the record");
        let rows = screen(&mut app);
        assert_eq!(chat_corners(&rows), chat);
    }

    #[test]
    fn the_named_header_bar_has_no_tabs_to_click() {
        use crate::app::ChatTab;
        use crate::events::UiCommand;
        use crossterm::event::{
            KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
        };

        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("notes.md"), "# Notes\n").unwrap();
        let mut app = App::new();
        app.process_command(UiCommand::SetSandboxRoot(dir.path().to_path_buf()));
        let rows = screen(&mut app);
        let (row, column) = rows
            .iter()
            .enumerate()
            .find_map(|(y, row)| {
                let x = (0..row.len().saturating_sub(8))
                    .find(|&x| row[x..x + 8].concat() == "TERMINAL")?;
                Some((y, x))
            })
            .expect("no TERMINAL tab");

        while app.focus() != crate::app::PanelFocus::Sandbox {
            app.handle_key(KeyEvent::from(KeyCode::Tab));
        }
        app.handle_key(KeyEvent::from(KeyCode::Enter));
        screen(&mut app);
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: u16::try_from(column).unwrap(),
            row: u16::try_from(row).unwrap(),
            modifiers: KeyModifiers::empty(),
        });
        assert_eq!(app.chat_tab(), ChatTab::Chat);
        assert!(matches!(app.detail_view(), Some(DetailView::File(_))));
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
        let header = first
            .lines()
            .find(|row| row.contains("│ main.rs "))
            .unwrap_or_else(|| panic!("no file header: {first}"));
        assert!(header.contains(" 100 LINES ⤢ ✕ │"), "{header}");
        assert!(!first.contains("TERMINAL"), "{first}");
        assert!(first.contains("  1 │ let x1 = 1;"), "{first}");

        app.handle_key(KeyEvent::from(KeyCode::Char('G')));
        let last = text(&mut app);
        assert!(last.contains("100 │ let x100 = 100;"), "{last}");
        assert!(last.contains("Ln 100/100, Col 1"), "{last}");
        assert!(!last.contains("  1 │ "), "{last}");

        for key in ":50\n".chars() {
            let code = if key == '\n' {
                KeyCode::Enter
            } else {
                KeyCode::Char(key)
            };
            app.handle_key(KeyEvent::from(code));
        }
        let middle = text(&mut app);
        assert!(middle.contains("Ln 50/100"), "{middle}");
        assert!(middle.contains(" 50 │ let x50 = 50;"), "{middle}");
    }

    #[test]
    fn cursor_and_matches_restyle_cells() {
        let mut vim = Vim::default();
        let mut scroll = 0;
        let screen = crate::app::vim::Screen { first: 0, shown: 5 };
        let lines = ["\tab ab"];
        for c in "/ab\n".chars() {
            let code = match c {
                '\n' => crossterm::event::KeyCode::Enter,
                c => crossterm::event::KeyCode::Char(c),
            };
            vim.handle_key(code.into(), &lines, &mut scroll, screen);
        }
        assert_eq!((vim.line(), vim.col()), (0, 1));
        let theme = Theme::cyber();
        let spans = mark(vec![Span::raw("    ab ab")], lines[0], &vim, true, &theme);
        let pieces: Vec<(&str, bool)> = spans
            .iter()
            .map(|s| {
                let reversed = s.style.add_modifier.contains(Modifier::REVERSED);
                (s.content.as_ref(), reversed)
            })
            .collect();
        assert_eq!(
            pieces,
            [
                ("    ", false),
                ("a", true),
                ("b", false),
                (" ", false),
                ("ab", false)
            ]
        );
        assert_eq!(spans[2].style.bg, Some(theme.colors.selection));
        assert_eq!(spans[3].style.bg, None);
    }

    #[test]
    fn fit_follows_the_cursor_and_centers_long_jumps() {
        let lines: Vec<String> = (0..100).map(|n| n.to_string()).collect();
        let source = Source::Code {
            language: String::new(),
            lines: lines.iter().map(String::as_str).collect(),
            gutter: 3,
            first_line: 1,
            highlighted: Highlighted::default(),
        };
        assert_eq!(fit(&source, Align::Cursor, 5, 0, 40, 10), 0);
        assert_eq!(fit(&source, Align::Cursor, 10, 0, 40, 10), 1);
        assert_eq!(fit(&source, Align::Cursor, 50, 0, 40, 10), 46);
        assert_eq!(fit(&source, Align::Cursor, 49, 50, 40, 10), 49);
        assert_eq!(fit(&source, Align::Top, 30, 0, 40, 10), 30);
        assert_eq!(fit(&source, Align::Bottom, 30, 0, 40, 10), 21);
        assert_eq!(fit(&source, Align::View, 30, 7, 40, 10), 7);
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
