//! A record or sandbox file shown in place of the chat panel.
//!
//! The view covers exactly the chat panel's rectangle, scrolls by source
//! line, and hard-wraps long lines so code keeps its indentation. A file
//! has a vim cursor the view follows.

mod file;
mod highlight;
mod source;
mod wrap;

use super::chat_panel::chat_scrollbar;
use super::chrome::{body_block, render_scrollbar, ScrollView};
use super::header::HeaderInfo;
use super::inbox::thread_lines;
use super::overlays::{record_detail_lines, record_status};
use super::processes::{process_detail_lines, status_glyph};
use crate::app::file_view::{FileBody, FileView};
use crate::app::image_preview::ImageView;
use crate::app::scrollbar::ScrollTarget;
use crate::app::DetailView;
use crate::components::truncate_record_info;
use crate::{App, Theme};
use file::draw_file;
use ratatui::{
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Clear, Paragraph},
    Frame,
};
use source::{last_page_start, Source};

/// The header's status: `icon` and `label` in `color`, or just the icon
/// when space is short.
fn status_info(icon: &str, label: &str, color: Color) -> HeaderInfo {
    HeaderInfo {
        text: format!("{icon} {}", label.to_uppercase()),
        short: icon.to_string(),
        color,
    }
}

/// The chat panel's header while a view covers its body: what is open on
/// the left, its status and a close button on the right. `None` leaves the
/// CHAT | TERMINAL tabs.
///
/// ```text
/// │ RECORD #60               ✓ POLICY ✕ │
/// │ src/main.rs             100 LINES ✕ │
/// ```
pub(super) fn detail_header(app: &App, theme: &Theme) -> Option<(String, Option<HeaderInfo>)> {
    match app.detail_view()? {
        DetailView::Record => {
            let record = app.selected_record_data()?;
            let (icon, color) = record_status(record, theme);
            let info = status_info(icon, &record.tx_kind, color);
            Some((format!("RECORD #{}", record.seq), Some(info)))
        }
        DetailView::File(file) => Some(file_header(file, theme)),
        DetailView::Process(process) => {
            let (icon, color) = status_glyph(process.status, theme);
            let title = format!(
                "{} {}",
                process.kind.label().to_uppercase(),
                truncate_record_info(&process.label, 40)
            );
            Some((
                title,
                Some(status_info(icon, process.status.label(), color)),
            ))
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

/// A file's header: its title, and its line count unless it is empty.
fn file_header(file: &FileView, theme: &Theme) -> (String, Option<HeaderInfo>) {
    let info = match &file.body {
        FileBody::Text(text) => {
            let lines = file.body.lines().len();
            let unit = if lines == 1 { "LINE" } else { "LINES" };
            (!text.is_empty()).then(|| HeaderInfo::count(lines, unit, theme.colors.muted))
        }
        FileBody::Note(_) => None,
    };
    (file.title.clone(), info)
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
    let (text, bounds) = draw_text_view(frame, (area, rows, hosted), app, theme);
    if let Some(drawn) = bounds {
        apply_drawn(frame, app, (area, text), drawn);
    }
}

/// Draw a plan, file or record, with the rect its text sits in.
fn draw_text_view(
    frame: &mut Frame,
    (area, rows, hosted): (Rect, Rect, bool),
    app: &mut App,
    theme: &Theme,
) -> (Rect, Option<Drawn>) {
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
    (text, bounds)
}

/// Tell `app` where the view was drawn: its bounds, rows and scrollbar.
fn apply_drawn(frame: &mut Frame, app: &mut App, (area, text): (Rect, Rect), mut drawn: Drawn) {
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

/// A record's, process's or thread's lines, the keys for its footer and,
/// for a thread, the line of each message header with its id.
type RecordLines<'a> = (Vec<Line<'a>>, &'static str, Vec<(usize, u64)>);

/// The lines of what `draw_record` shows; `None` for views drawn elsewhere.
fn record_lines<'a>(app: &'a App, rows: Rect, theme: &'a Theme) -> Option<RecordLines<'a>> {
    let hint = "Esc close  ↑↓ PgUp/PgDn scroll ";
    match app.detail_view()? {
        DetailView::Record => {
            let record = app.selected_record_data()?;
            let lines = record_detail_lines(record, usize::from(rows.width), theme);
            Some((lines, hint, Vec::new()))
        }
        DetailView::Process(process) => {
            let lines = process_detail_lines(process, usize::from(rows.width), theme);
            Some((lines, hint, Vec::new()))
        }
        DetailView::Thread(view) => {
            let (lines, header_lines) = thread_lines(app, view, rows.width, theme);
            let hint = "Esc close  n/p message  ⏎ fold  e/c all  ↑↓ scroll ";
            Some((lines, hint, header_lines))
        }
        // Drawn by `plan_doc::draw_plan`.
        DetailView::Plan(_) | DetailView::File(_) | DetailView::Graph | DetailView::Image(_) => {
            None
        }
    }
}

/// Draw the highlighted record.
fn draw_record(
    frame: &mut Frame,
    (area, rows, hosted): (Rect, Rect, bool),
    app: &App,
    theme: &Theme,
) -> Option<Drawn> {
    let (lines, hint, headers) = record_lines(app, rows, theme)?;
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

#[cfg(test)]
mod tests;
