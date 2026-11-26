//! The Chat panel: the conversation, the terminal, and the other chat
//! tabs under its header bar.

use super::chrome::{render_body_block, render_scrollbar, scrollbar_track, ScrollView};
use super::detail::detail_header;
use super::header::{self, HeaderInfo, PanelHeader};
use super::markdown::is_inline_code;
use super::side_list::render_empty;
use super::text::{display_width, Measure};
use crate::{
    app::{
        scrollbar::{ScrollTarget, ScrollbarArea},
        tab_bar::TabLayout,
        ChatScroll, ChatTab, HoverTarget, ImageHit, Pane, PanelFocus, ProseRow, ToggleRow,
        ToggleTarget,
    },
    App, Theme,
};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget},
    Frame,
};
use std::ops::Range;

/// Render the Chat panel: a CHAT | TERMINAL | GRAPH header bar over the
/// conversation, the shell commands run this session, or the flow between
/// the agent and the agents it spawned. While a record, file, process or
/// the run graph covers the body, the bar names it. BENCH shows a
/// benchmark, or the form that sets one up, under a bar with no tabs.
pub(super) fn render_chat_panel(frame: &mut Frame, area: Rect, app: &mut App, theme: &Theme) {
    let is_focused = app.focus() == PanelFocus::Chat;
    let hosted = app.host_chrome();
    let panel = area;
    let (bar, area) = header::split(area, hosted);
    let expand = |cell| {
        Some(header::ExpandButton {
            expanded: app.chat_expanded(),
            hovered: app.hover() == Some(HoverTarget::ExpandChat),
            cell,
        })
    };
    if let Some((title, info)) =
        detail_header(app, theme).or_else(|| member_chat_header(app, theme))
    {
        let expand = expand(header::expand_cell(bar, true));
        if !hosted {
            app.set_detail_close_area(header::close_cell(bar));
            app.set_chat_expand_area(header::expand_cell(bar, true));
        }
        let header = PanelHeader::<()> {
            layout: TabLayout::untabbed(bar.width),
            active: None,
            hovered: None,
            title: Some(&title),
            info,
            close: Some(app.hover() == Some(HoverTarget::CloseDetail)),
            expand,
            scope: None,
            icon_tab: None,
            focused: true,
            hosted,
        };
        header::show(frame, app, (Pane::Chat, panel, bar), &header, theme);
    } else {
        let tabs = header::tabs_beside_expand(bar);
        let expand = expand(header::expand_cell(bar, false));
        if !hosted {
            app.set_chat_tabs_area(Rect { width: tabs, ..bar });
            app.set_chat_expand_area(header::expand_cell(bar, false));
        }
        let hovered = match app.hover() {
            Some(HoverTarget::ChatTab(tab)) => Some(tab),
            _ => None,
        };
        let app_title = app.app_chat_title();
        let (title, info) = match &app_title {
            Some((name, about)) => (
                Some(name.as_str()),
                Some(about)
                    .filter(|about| !about.trim().is_empty())
                    .map(|about| HeaderInfo {
                        text: about.clone(),
                        short: String::new(),
                        color: theme.colors.muted,
                    }),
            ),
            None => (None, None),
        };
        let header = PanelHeader {
            layout: app.chat_tab_layout(tabs),
            active: Some(app.chat_tab()),
            hovered,
            title,
            info,
            close: None,
            expand,
            scope: None,
            icon_tab: None,
            focused: is_focused,
            hosted,
        };
        header::show(frame, app, (Pane::Chat, panel, bar), &header, theme);
    }

    // An app showing nothing shows none of the agent's tabs either.
    let empty = app.app_empty();
    if !empty && app.bench_draws_center() {
        super::benchmarks::render_benchmarks(frame, area, app, theme, is_focused);
        return;
    }
    if !empty && matches!(app.chat_tab(), ChatTab::Graph | ChatTab::Time) {
        super::flow::render_flow(frame, area, app, theme, is_focused);
        return;
    }
    if !empty && app.chat_tab() == ChatTab::Log {
        super::run_log::render_run_log(frame, area, app, theme, is_focused);
        return;
    }

    let Some(body) = render_body_block(frame, area, (is_focused, hosted), theme) else {
        return;
    };

    let terminal = app.terminal_pane() && !empty;
    let padded = terminal_text_area(body, terminal);
    app.set_chat_width(padded.width);
    app.set_chat_text_area(padded);

    if empty {
        app.set_chat_images(Vec::new());
        app.set_chat_code_copies(Vec::new());
        app.set_chat_rows(Some((padded, 0)));
        render_empty(frame, padded, &app.app_empty_message(), theme);
        return;
    }

    let tail = if terminal {
        LiveTail::default()
    } else {
        live_tail(app, theme)
    };
    if !terminal && app.chat_loading() {
        // The agent's history is on its way: neither the welcome text nor
        // a partial chat, which would flash before the real one.
        app.set_chat_images(Vec::new());
        app.set_chat_code_copies(Vec::new());
        app.set_chat_rows(Some((padded, 0)));
        if app.chat_loading_shown() {
            render_chat_loading(frame, padded, theme);
        }
        return;
    }
    if app.pane_is_empty() && tail.lines.is_empty() {
        app.set_chat_images(Vec::new());
        app.set_chat_code_copies(Vec::new());
        app.set_chat_rows(Some((padded, 0)));
        render_welcome(frame, padded, (terminal, app.channel_open()), theme);
        return;
    }

    let rows = render_chat_lines(frame, (body, padded), app, theme, &tail);
    let view = ScrollView {
        total: rows.total,
        visible: padded.height as usize,
        first: rows.first,
    };
    render_scrollbar(frame, area, padded, view, is_focused, theme);
    let last = view.total.saturating_sub(view.visible);
    let scrollbar = chat_scrollbar(ScrollTarget::Chat, (area, padded), view, last);
    app.set_chat_scrollbar(scrollbar, frame.buffer_mut());
    let below = app.scroll_offset().saturating_sub(rows.padding);
    render_lines_below_hint(frame, area, below, theme);
}

/// The chat panel's header over a channel member's own chat: the
/// member's name, its code, role and the channel's `#slug`, and the close
/// button that goes back to the channel's chat. `None` elsewhere.
///
/// ```text
/// │ Reviewer           RE36 · agent · #zode ✕ │
/// ```
fn member_chat_header(app: &App, theme: &Theme) -> Option<(String, Option<HeaderInfo>)> {
    let (name, about) = app.member_chat_title()?;
    let short = app
        .member_chat()
        .and_then(|member| member.code.clone())
        .unwrap_or_default();
    let info = HeaderInfo {
        text: about,
        short,
        color: theme.colors.muted,
    };
    Some((name, Some(info)))
}

/// The scrollbar [`render_scrollbar`] draws on the chat panel `panel`
/// beside the text in `rows` for `view`, as the pointer drags it to
/// scroll `target` as far as its first row `last`.
pub(super) fn chat_scrollbar(
    target: ScrollTarget,
    (panel, rows): (Rect, Rect),
    view: ScrollView,
    last: usize,
) -> Option<ScrollbarArea> {
    scrollbar_track(panel, rows, view).map(|track| ScrollbarArea {
        target,
        track,
        total: view.total,
        visible: view.visible,
        first: view.first,
        last,
        open: track.height,
        margin: track.x.saturating_sub(rows.right()),
    })
}

/// Draw the rows of the conversation that fall in `padded` at the current
/// scroll position, cloning only those rows out of the line cache, then
/// `tail` a blank row below them. With smooth scrolling on, the row below
/// `padded` is drawn on its own too, and `body` is what the front end
/// slides.
fn render_chat_lines(
    frame: &mut Frame,
    (body, padded): (Rect, Rect),
    app: &mut App,
    theme: &Theme,
    tail: &LiveTail,
) -> ChatRows {
    let spinner = app.spinner_char();
    let hovered_activity = match app.hover() {
        Some(HoverTarget::Activity(target)) => Some(target),
        _ => None,
    };
    let measure = Measure::prose(app.prose_measure(), theme);
    let (shown, cache, groups) = app.chat_line_cache();
    let width = padded.width as usize;
    let reflowed = cache.update(shown, (width, measure), theme, spinner, &groups);
    let gap = cache.block_gap();
    let mut toggle_rows = Vec::new();
    let mut image_hits = Vec::new();
    let mut code_hits = Vec::new();
    let mut message_rows = 0;
    let mut last_prompt_row = None;
    for (i, block) in cache.blocks().enumerate() {
        if i > 0 {
            message_rows += gap;
        }
        if block.user {
            last_prompt_row = Some(message_rows);
        }
        toggle_rows.extend(block.toggles.iter().map(|toggle| ToggleRow {
            line: message_rows + toggle.line,
            ..*toggle
        }));
        image_hits.extend(block.images.iter().map(|hit| ImageHit {
            line: message_rows + hit.line,
            ..*hit
        }));
        code_hits.extend(block.codes.iter().map(|hit| crate::app::CodeCopyHit {
            line: message_rows + hit.line,
            ..hit.clone()
        }));
        message_rows += block.lines.len();
    }
    let tail_gap = usize::from(message_rows > 0 && !tail.lines.is_empty());
    let tail_start = message_rows + tail_gap;
    let tail_toggle = app
        .turn_started_ms()
        .zip(tail.status_row)
        .map(|(started, row)| ToggleRow {
            line: tail_start + row,
            target: ToggleTarget::TurnRecords(started),
            open: false,
        });
    toggle_rows.extend(tail_toggle);
    let content = tail_start + tail.lines.len();
    let height = padded.height as usize;
    // Blank rows after the end, enough that scrolling to the bottom can
    // bring the latest prompt to the top with the reply filling in below,
    // and never fewer than the conversation's breathing room, so a reply
    // that reaches the bottom scrolls up with space left under it. The
    // terminal's output runs to the bottom row, as a terminal's does.
    let room = if app.terminal_pane() {
        0
    } else {
        breathing_room(height)
    };
    let padding = last_prompt_row
        .map_or(0, |row| (row + height).saturating_sub(content))
        .max(room);
    let total = content + padding;
    let start = app.sync_chat_scroll(total, height, reflowed);
    app.set_chat_layout(toggle_rows, start);
    app.set_chat_rows(Some((padded, start)));

    let smooth = app.smooth_scroll();
    let rows = height + usize::from(smooth);
    let (_, cache, _) = app.chat_line_cache();
    let blank = Line::default();
    let mut visible = Vec::with_capacity(rows);
    let mut prose = Vec::new();
    let mut user_rows = Vec::new();
    let mut line_index = 0usize;
    for (i, block) in cache.blocks().enumerate() {
        if visible.len() >= rows {
            break;
        }
        if i > 0 {
            for _ in 0..gap {
                push_visible(&mut visible, &mut line_index, start, rows, blank.clone());
            }
        }
        // A block above the screen only moves the ones below it down.
        if line_index + block.lines.len() <= start {
            line_index += block.lines.len();
            continue;
        }
        let hovered = |offset: usize| {
            block
                .toggles
                .iter()
                .any(|toggle| toggle.line == offset && hovered_activity == Some(toggle.target))
        };
        for (offset, line) in block.lines.iter().enumerate() {
            if line_index >= start && visible.len() < rows {
                let mut line = line.clone();
                if hovered(offset) {
                    apply_activity_hover(&mut line, theme);
                }
                if let Some(Some(indent)) = block.prose.get(offset).filter(|_| line.width() > 0) {
                    prose.push((visible.len(), *indent));
                }
                if block.user {
                    user_rows.push((visible.len(), BandEdge::at(offset, block.lines.len())));
                }
                visible.push(line);
            }
            line_index += 1;
        }
    }
    if tail_gap > 0 {
        push_visible(&mut visible, &mut line_index, start, rows, blank);
    }
    for (row, line) in tail.lines.iter().enumerate() {
        let mut line = line.clone();
        let status_hovered = tail.status_row == Some(row)
            && tail_toggle.is_some_and(|t| hovered_activity == Some(t.target));
        if status_hovered {
            apply_activity_hover(&mut line, theme);
        }
        push_visible(&mut visible, &mut line_index, start, rows, line);
    }
    let next = (visible.len() > height).then(|| visible.pop()).flatten();
    let next_prose = prose
        .last()
        .filter(|(row, _)| *row == height)
        .map(|&(_, indent)| indent.min(padded.width));
    prose.retain(|(row, _)| *row < height);
    let next_user = user_rows
        .last()
        .filter(|(row, _)| *row == height)
        .map(|&(_, edge)| edge);
    user_rows.retain(|(row, _)| *row < height);
    let prose_lines: Vec<(usize, u16, Line<'static>)> = prose
        .into_iter()
        .map(|(row, indent)| (row, indent.min(padded.width), visible[row].clone()))
        .collect();
    frame.render_widget(Paragraph::new(visible), padded);
    let buffer = frame.buffer_mut();
    let prose = prose_lines
        .into_iter()
        .map(|(row, indent, line)| {
            #[allow(clippy::cast_possible_truncation)]
            let row = Rect {
                y: padded.y + row as u16,
                height: 1,
                ..padded
            };
            let (width, code) = if measure.is_grid() {
                prose_columns(&line, row, theme)
            } else {
                pack_prose(buffer, &line, (row, indent), measure, theme)
            };
            ProseRow::new(buffer, (row.x, row.y), (width, indent), code)
        })
        .collect();
    app.set_prose_rows(prose);
    paint_user_band(frame.buffer_mut(), (body, padded), &user_rows, theme);
    // A chat that waited for its history fades in from the background.
    // Only colours change, so the prose rows read above stay valid, and
    // it comes before the smooth scroll takes its print of the body.
    let fade = app.chat_fading().filter(|_| !app.terminal_pane());
    if let Some(progress) = fade {
        crate::animation::fade_area(frame.buffer_mut(), body, progress, theme);
    }

    let hovered_image = match app.hover() {
        Some(HoverTarget::ChatImage(message, index)) => Some((message, index)),
        _ => None,
    };
    if let Some(hit) = image_hits
        .iter()
        .find(|hit| hovered_image == Some((hit.message, hit.index)))
    {
        let caption = hit.line + hit.rows - 1;
        if (start..start + height).contains(&caption) {
            let row = Rect {
                x: padded.x + hit.x,
                y: padded.y + u16::try_from(caption - start).unwrap_or(u16::MAX),
                width: hit.width,
                height: 1,
            }
            .intersection(padded);
            super::images::highlight_caption(frame.buffer_mut(), row, theme);
        }
    }
    if let Some(HoverTarget::CopyCode(line)) = app.hover() {
        if let Some(hit) = code_hits.iter().find(|hit| hit.line == line) {
            highlight_copy_button(frame.buffer_mut(), hit, (padded, start), theme);
        }
    }
    app.set_chat_images(image_hits);
    app.set_chat_code_copies(code_hits);
    if smooth {
        let width = frame.buffer_mut().area.width;
        let mut below = row_below(
            width,
            (body, padded),
            next,
            (next_prose, next_user),
            measure,
            theme,
        );
        if let Some(progress) = fade {
            let area = below.0.area;
            crate::animation::fade_area(&mut below.0, area, progress, theme);
        }
        let view = ChatScroll {
            body,
            above: start,
            below: app.scroll_offset(),
        };
        app.set_chat_scroll(view, frame.buffer_mut(), below);
    }
    ChatRows {
        total,
        first: start,
        padding,
    }
}

/// The chat's `line` below `padded`, drawn alone into a buffer a row high
/// and `width` wide, with its prose when it has an indent (`prose`), and
/// the band of a user's message under it when it is one (`user`).
fn row_below(
    width: u16,
    (body, padded): (Rect, Rect),
    line: Option<Line<'static>>,
    (prose, user): (Option<u16>, Option<BandEdge>),
    measure: Measure,
    theme: &Theme,
) -> (Buffer, Option<ProseRow>) {
    let mut buffer = Buffer::empty(Rect::new(0, 0, width, 1));
    let row = Rect {
        y: 0,
        height: 1,
        ..padded
    }
    .intersection(buffer.area);
    let Some(line) = line else {
        return (buffer, None);
    };
    let prose_line = prose.map(|indent| (indent, line.clone()));
    Paragraph::new(line).render(row, &mut buffer);
    let prose = prose_line.map(|(indent, line)| {
        let (width, code) = if measure.is_grid() {
            prose_columns(&line, row, theme)
        } else {
            pack_prose(&mut buffer, &line, (row, indent), measure, theme)
        };
        ProseRow::new(&buffer, (row.x, row.y), (width, indent), code)
    });
    if let Some(edge) = user {
        let body = Rect {
            y: 0,
            height: 1,
            ..body
        };
        paint_user_band(&mut buffer, (body, row), &[(0, edge)], theme);
    }
    (buffer, prose)
}

/// How the chat's rows fell in the last frame.
struct ChatRows {
    /// Rows in the whole conversation, blank padding included
    total: usize,
    /// First row on screen
    first: usize,
    /// Blank rows after the end
    padding: usize,
}

/// How many of `area`'s columns a prose `line` covers, and where its
/// inline code falls.
pub(super) fn prose_columns(line: &Line<'_>, area: Rect, theme: &Theme) -> (u16, Vec<Range<u16>>) {
    let mut code: Vec<Range<u16>> = Vec::new();
    let mut column = area.x;
    let end = area.right();
    for span in &line.spans {
        let width = u16::try_from(span.width()).unwrap_or(u16::MAX);
        let next = column.saturating_add(width).min(end);
        if is_inline_code(span.style, theme) && next > column {
            match code.last_mut() {
                Some(last) if last.end == column => last.end = next,
                _ => code.push(column..next),
            }
        }
        column = next;
    }
    (column - area.x, code)
}

/// Write a prose `line` into `row` with its text after `indent` columns
/// packed into the cells it covers in the front end's face, `measure`
/// wide: each run of one style gets the cells its width rounds to, its
/// characters spread across them. Inline code keeps a cell per column.
/// Returns the columns the line covers and where its inline code falls.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]
pub(super) fn pack_prose(
    buffer: &mut Buffer,
    line: &Line<'_>,
    (row, indent): (Rect, u16),
    measure: Measure,
    theme: &Theme,
) -> (u16, Vec<Range<u16>>) {
    let mut runs: Vec<(Vec<&str>, Style)> = Vec::new();
    let mut skipped = 0;
    for grapheme in line.styled_graphemes(Style::default()) {
        if skipped < usize::from(indent) {
            skipped += display_width(grapheme.symbol);
            continue;
        }
        match runs.last_mut() {
            Some((symbols, style)) if *style == grapheme.style => symbols.push(grapheme.symbol),
            _ => runs.push((vec![grapheme.symbol], grapheme.style)),
        }
    }

    let mut code: Vec<Range<u16>> = Vec::new();
    let mut cell = indent;
    let mut position = f32::from(indent);
    let mut last = None;
    for (symbols, style) in runs {
        let width = if is_inline_code(style, theme) {
            symbols.iter().map(|s| display_width(s)).sum::<usize>() as f32
        } else {
            measure.width(&symbols.concat(), style)
        };
        let start = cell;
        position += width;
        let end = if is_inline_code(style, theme) {
            start.saturating_add(width as u16)
        } else {
            (position.round() as u16).max(start + 1)
        }
        .min(row.width);
        if end <= start {
            // Past the edge: the rest joins the last cell.
            if let Some(cell) = last.and_then(|x| buffer.cell_mut((x, row.y))) {
                let joined = format!("{}{}", cell.symbol(), symbols.concat());
                cell.set_symbol(&joined);
            }
            continue;
        }
        let cells = usize::from(end - start);
        for i in 0..cells {
            let share = &symbols[i * symbols.len() / cells..(i + 1) * symbols.len() / cells];
            if let Some(slot) = buffer.cell_mut((row.x + start + i as u16, row.y)) {
                slot.reset();
                slot.set_symbol(&share.concat()).set_style(style);
            }
        }
        if is_inline_code(style, theme) {
            code.push(row.x + start..row.x + end);
        }
        last = Some(row.x + end - 1);
        cell = end;
        position = position.max(f32::from(cell) - 0.5);
    }
    for x in cell..row.width {
        if let Some(slot) = buffer.cell_mut((row.x + x, row.y)) {
            slot.reset();
        }
    }
    (cell, code)
}

/// Where a row falls in the frame around a user's message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BandEdge {
    /// The frame's top border
    Top,
    /// A row between the borders: padding or text
    Side,
    /// The frame's bottom border
    Bottom,
}

impl BandEdge {
    /// The edge of row `offset` of a user's block `len` rows long. The
    /// block's first and last rows are left blank for the borders (see
    /// `chat_lines::USER_FRAME_ROWS`).
    const fn at(offset: usize, len: usize) -> Self {
        if offset == 0 {
            Self::Top
        } else if offset + 1 == len {
            Self::Bottom
        } else {
            Self::Side
        }
    }
}

/// Frame the rows of the user's messages: a border two columns out from
/// the text on each side, clipped to `body`. The frame is not shaded, so
/// a sent message sits on the chat's own background.
fn paint_user_band(
    buffer: &mut Buffer,
    (body, padded): (Rect, Rect),
    rows: &[(usize, BandEdge)],
    theme: &Theme,
) {
    let border = Style::default().fg(theme.colors.faint);
    let symbols = ratatui::widgets::BorderType::border_symbols(theme.border_style.to_border_type());
    let left = padded.x.saturating_sub(2).max(body.x);
    let right = padded
        .right()
        .saturating_add(1)
        .min(body.right().saturating_sub(1));
    if right <= left {
        return;
    }
    for &(row, edge) in rows {
        let Ok(offset) = u16::try_from(row) else {
            continue;
        };
        let y = padded.y + offset;
        let (start, middle, end) = match edge {
            BandEdge::Top => (symbols.top_left, symbols.horizontal_top, symbols.top_right),
            BandEdge::Bottom => (
                symbols.bottom_left,
                symbols.horizontal_bottom,
                symbols.bottom_right,
            ),
            BandEdge::Side => (symbols.vertical_left, "", symbols.vertical_right),
        };
        let mut put = |x: u16, symbol: &str| {
            if let Some(cell) = buffer.cell_mut((x, y)) {
                cell.set_symbol(symbol).set_style(border);
            }
        };
        put(left, start);
        put(right, end);
        if !middle.is_empty() {
            for x in left + 1..right {
                put(x, middle);
            }
        }
    }
}

/// Keep `line` when it falls inside the chat viewport.
fn push_visible<'a>(
    visible: &mut Vec<Line<'a>>,
    line_index: &mut usize,
    start: usize,
    height: usize,
    line: Line<'a>,
) {
    if *line_index >= start && visible.len() < height {
        visible.push(line);
    }
    *line_index += 1;
}

/// Draw a hovered code block's copy button in the accent, so it reads as
/// something to click; `start` is the chat line at the top of `padded`.
fn highlight_copy_button(
    buffer: &mut ratatui::buffer::Buffer,
    hit: &crate::app::CodeCopyHit,
    (padded, start): (Rect, usize),
    theme: &Theme,
) {
    let Some(row) = hit
        .line
        .checked_sub(start)
        .and_then(|row| u16::try_from(row).ok())
        .filter(|row| *row < padded.height)
    else {
        return;
    };
    let button = Rect {
        x: padded.x.saturating_add(hit.x),
        y: padded.y + row,
        width: hit.width,
        height: 1,
    }
    .intersection(padded);
    buffer.set_style(button, Style::default().fg(theme.colors.success));
}

/// Hover style on an activity header. Status colors stay.
fn apply_activity_hover(line: &mut Line<'_>, theme: &Theme) {
    let status = [
        theme.colors.success,
        theme.colors.error,
        theme.colors.warning,
    ];
    for span in &mut line.spans {
        let keep_color = span.style.fg.is_some_and(|color| status.contains(&color));
        if !keep_color {
            span.style = span.style.fg(theme.colors.secondary);
        }
    }
}

/// `↓ 42 lines below` on the chat panel's bottom border while scrolled up.
fn render_lines_below_hint(frame: &mut Frame, panel: Rect, below: usize, theme: &Theme) {
    if below == 0 || panel.width < 4 || panel.height < 2 {
        return;
    }
    let label = format!(
        " ↓ {below} line{} below ",
        if below == 1 { "" } else { "s" }
    );
    let border_row = Rect {
        x: panel.x + 1,
        y: panel.y + panel.height - 1,
        width: panel.width - 2,
        height: 1,
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            label,
            Style::default().fg(theme.colors.muted),
        )))
        .alignment(ratatui::layout::Alignment::Right),
        border_row,
    );
}

/// The text area of the chat and the terminal, every row of the body from
/// its top border to its bottom one. The terminal keeps a single column
/// on each side, so output starts at the top left like a terminal's; the
/// chat is inset further, stops at [`MAX_CHAT_WIDTH`] columns so long
/// lines stay readable, then gives up [`CHAT_NARROW_PERCENT`] of that
/// width and sits centered, with equal padding on both sides.
pub(super) fn terminal_text_area(area: Rect, terminal: bool) -> Rect {
    if terminal {
        return Rect {
            x: area.x.saturating_add(1),
            width: area.width.saturating_sub(2),
            ..area
        };
    }
    let inset = chat_inset(area.width);
    let base = area.width.saturating_sub(inset * 2).min(MAX_CHAT_WIDTH);
    let width = narrowed(base);
    // Center the narrowed text so the blank space on either side matches.
    let left = (area.width - width) / 2;
    Rect {
        x: area.x.saturating_add(left),
        width,
        ..area
    }
}

/// Widest the chat's text runs, in columns, before [`CHAT_NARROW_PERCENT`]
/// is taken off.
const MAX_CHAT_WIDTH: u16 = 110;

/// Share of the chat's text width, in percent, given up to extra padding
/// split evenly on both sides.
const CHAT_NARROW_PERCENT: u32 = 17;

/// `width` less [`CHAT_NARROW_PERCENT`], rounded to the nearest column.
fn narrowed(width: u16) -> u16 {
    let kept = (u32::from(width) * (100 - CHAT_NARROW_PERCENT) + 50) / 100;
    u16::try_from(kept).unwrap_or(width)
}

/// Share of the chat's height, in percent, kept blank below the
/// conversation once it reaches the bottom.
const CHAT_ROOM_PERCENT: usize = 25;

/// Blank rows kept below the conversation's last line in a chat `height`
/// rows tall: [`CHAT_ROOM_PERCENT`] of them, rounded down, so the newest
/// text stops short of the bottom border and the input box below it, and
/// a short panel keeps its few rows for text.
pub(super) const fn breathing_room(height: usize) -> usize {
    height * CHAT_ROOM_PERCENT / 100
}

/// Columns of blank space on each side of the chat: two, or one on narrow
/// panels where every column of text counts.
const fn chat_inset(width: u16) -> u16 {
    if width < 40 {
        1
    } else {
        2
    }
}

/// What follows the conversation while a turn is in flight: the working
/// line. The to-do list lives in the drawer above the input, not here.
#[derive(Default)]
struct LiveTail {
    lines: Vec<Line<'static>>,
    /// Row in `lines` of the working line, which opens the turn's records
    /// when clicked.
    status_row: Option<usize>,
}

fn live_tail(app: &App, theme: &Theme) -> LiveTail {
    match working_indicator_line(app, theme) {
        Some(line) => LiveTail {
            lines: vec![line],
            status_row: Some(0),
        },
        None => LiveTail::default(),
    }
}

/// `⬢ Thinking… 12s` — shown below the conversation, flush with the
/// prose, while a turn is in flight and no reply text is streaming; a
/// running tool call counts its own time, and a command that has printed
/// nothing for a while says so (`· no output 45s`). It
/// stays the last line even while a group of steps is still growing above
/// it, and a band of light sweeps across its label to show work goes on.
pub(super) fn working_indicator_line<'a>(app: &App, theme: &Theme) -> Option<Line<'a>> {
    let label = format!("{}…", capitalized(&app.working_label()?));
    let label_style = Style::default().add_modifier(Modifier::BOLD);
    let mut spans = vec![Span::styled(
        "⬢ ",
        Style::default().fg(theme.colors.success),
    )];
    if theme.animate_spinners {
        spans.extend(crate::animation::shimmer_spans(
            &label,
            app.animation_frame(),
            label_style,
            (theme.colors.muted, theme.colors.foreground),
        ));
    } else {
        spans.push(Span::styled(label, label_style.fg(theme.colors.foreground)));
    }
    let (elapsed, quiet) = app.working_times();
    if let Some(elapsed) = elapsed.filter(|e| e.as_secs() > 0) {
        spans.push(Span::styled(
            format!(" {}", format_elapsed(elapsed)),
            Style::default().fg(theme.colors.muted),
        ));
    }
    if let Some(quiet) = quiet {
        spans.push(Span::styled(
            format!(" · no output {}", format_elapsed(quiet)),
            Style::default().fg(theme.colors.success),
        ));
    }
    Some(Line::from(spans))
}

/// `text` with its first letter in upper case.
fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// `7s`, `2m 05s`.
pub(crate) fn format_elapsed(elapsed: std::time::Duration) -> String {
    let secs = elapsed.as_secs();
    if secs < 60 {
        format!("{secs}s")
    } else {
        format!("{}m {:02}s", secs / 60, secs % 60)
    }
}

/// What the chat says once it has waited a moment for its history.
const CHAT_LOADING: &str = "Loading chat…";

fn render_chat_loading(frame: &mut Frame, area: Rect, theme: &Theme) {
    let line = Line::from(Span::styled(
        CHAT_LOADING,
        Style::default().fg(theme.colors.faint),
    ));
    frame.render_widget(Paragraph::new(line), area);
}

fn render_welcome(frame: &mut Frame, area: Rect, (terminal, channel): (bool, bool), theme: &Theme) {
    let text = if terminal {
        "Enter runs a command. Esc returns to chat."
    } else if channel {
        // A channel has no terminal: `$` does nothing there.
        "Message the coordinator, @CODE a member, /help for commands."
    } else {
        "Type a message, $ for a terminal, /help for commands."
    };
    let welcome = Line::from(Span::styled(text, Style::default().fg(theme.colors.muted)));
    frame.render_widget(Paragraph::new(welcome), area);
}
