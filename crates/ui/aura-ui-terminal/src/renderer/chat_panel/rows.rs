//! The conversation's rows: where everything falls in the whole chat,
//! which rows are on screen, and what is drawn over them.

use super::below::{hand_over_scroll, BelowRow};
use super::breathing_room;
use super::flash;
use super::links;
use super::live_tail::LiveTail;
use super::prose::{pack_prose_mapped, prose_columns, ColumnMap};
use super::user_band::paint_user_band;
use super::visible::{NextRow, VisibleRows};
use crate::renderer::chat_lines::ChatLineCache;
use crate::renderer::text::Measure;
use crate::{
    app::{CodeCopyHit, HoverTarget, ImageHit, LinkHit, ProseRow, ToggleRow, ToggleTarget},
    App, Theme,
};
use ratatui::{buffer::Buffer, layout::Rect, style::Style, text::Line, widgets::Paragraph, Frame};
use std::ops::Range;

/// The prose rows packed into a proportional face, by their row on
/// screen, with where each one's columns went.
type PackedRows = Vec<(usize, ColumnMap)>;

/// How the chat's rows fell in the last frame.
pub(super) struct ChatRows {
    /// Rows in the whole conversation, blank padding included
    pub(super) total: usize,
    /// First row on screen
    pub(super) first: usize,
    /// Blank rows after the end
    pub(super) padding: usize,
}

/// Draw the rows of the conversation that fall in `padded` at the current
/// scroll position, cloning only those rows out of the line cache, then
/// `tail` a blank row below them. With smooth scrolling on, the row below
/// `padded` is drawn on its own too, and `body` is what the front end
/// slides.
pub(super) fn render_chat_lines(
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
    let layout = layout_rows(app, padded, (measure, spinner), theme, tail);
    let height = padded.height as usize;
    let smooth = app.smooth_scroll();
    let rows = height + usize::from(smooth);
    let (visible, next) = visible_rows(
        app,
        &layout,
        (rows, padded),
        (tail, hovered_activity),
        theme,
    );
    let (fade, packed) = draw_visible(frame, app, (body, padded), visible, (measure, theme));
    highlight_hovered(
        frame.buffer_mut(),
        app.hover(),
        &layout,
        (padded, height),
        theme,
    );
    place_links(frame.buffer_mut(), app, &layout, &packed, (padded, height));
    let RowLayout {
        image_hits,
        code_hits,
        total,
        padding,
        start,
        ..
    } = layout;
    app.set_chat_images(image_hits);
    app.set_chat_code_copies(code_hits);
    if smooth {
        let below = BelowRow {
            next,
            measure,
            fade,
        };
        hand_over_scroll(frame, app, (body, padded, start), below, theme);
    }
    ChatRows {
        total,
        first: start,
        padding,
    }
}

/// Gather the `rows` of the cached blocks and of `tail` that fall on
/// screen, and take out the one below `padded`.
fn visible_rows(
    app: &mut App,
    layout: &RowLayout,
    (rows, padded): (usize, Rect),
    (tail, hovered_activity): (&LiveTail, Option<ToggleTarget>),
    theme: &Theme,
) -> (VisibleRows, NextRow) {
    let (_, cache, _) = app.chat_line_cache();
    let mut visible = VisibleRows::new(layout.start, rows);
    visible.push_blocks(cache, layout.gap, hovered_activity, theme);
    let tail_hover = (layout.tail_gap, layout.tail_toggle, hovered_activity);
    visible.push_tail(tail, tail_hover, theme);
    let next = visible.take_next(padded.height as usize, padded.width);
    (visible, next)
}

/// Draw the `visible` rows into `padded` with their prose packed and the
/// user's messages framed, then fade the chat in while it waits; returns
/// how far it has faded in, and where each packed row's columns went.
fn draw_visible(
    frame: &mut Frame,
    app: &mut App,
    (body, padded): (Rect, Rect),
    visible: VisibleRows,
    (measure, theme): (Measure, &Theme),
) -> (Option<(usize, usize)>, PackedRows) {
    let VisibleRows {
        lines,
        prose,
        user_rows,
        ..
    } = visible;
    let (prose, packed) = draw_rows(frame, padded, (lines, prose), measure, theme);
    app.set_prose_rows(prose);
    paint_user_band(frame.buffer_mut(), (body, padded), &user_rows, theme);
    // A chat that waited for its history fades in from the background.
    // Only colours change, so the prose rows read above stay valid, and
    // it comes before the smooth scroll takes its print of the body.
    let fade = app.chat_fading().filter(|_| !app.terminal_pane());
    if let Some(progress) = fade {
        crate::animation::fade_area(frame.buffer_mut(), body, progress, theme);
    }
    (fade, packed)
}

/// Put the links of the rows on screen where they were drawn, underline
/// the one the pointer is on, and hand them to `app` for the pointer.
fn place_links(
    buffer: &mut Buffer,
    app: &mut App,
    layout: &RowLayout,
    packed: &[(usize, ColumnMap)],
    (padded, height): (Rect, usize),
) {
    let shown = links::place(&layout.link_hits, packed, (layout.start, height));
    links::underline_hovered(buffer, &shown, app.hover(), (padded, layout.start));
    app.set_chat_links(shown, buffer);
}

/// Where the chat's blocks and its live tail fall in the whole
/// conversation, and where it is scrolled to.
struct RowLayout {
    /// Image thumbnails, `line` counted from the conversation's first row
    image_hits: Vec<ImageHit>,
    /// Code blocks' copy buttons, `line` counted from the first row
    code_hits: Vec<CodeCopyHit>,
    /// Rows of links, `line` counted from the first row, on the grid
    link_hits: Vec<LinkHit>,
    /// The working line's row, which opens the turn's records
    tail_toggle: Option<ToggleRow>,
    /// Blank rows between the conversation and the live tail
    tail_gap: usize,
    /// Blank rows between two blocks
    gap: usize,
    /// Rows in the whole conversation, blank padding included
    total: usize,
    /// Blank rows after the end
    padding: usize,
    /// First row on screen
    start: usize,
    /// Rows of the message lit up after a jump from HISTORY
    flash: Option<Range<usize>>,
}

/// Bring the line cache up to date, lay out where its blocks and `tail`
/// fall, and scroll the chat to fit.
fn layout_rows(
    app: &mut App,
    padded: Rect,
    (measure, spinner): (Measure, &str),
    theme: &Theme,
    tail: &LiveTail,
) -> RowLayout {
    let targets = flash::Targets::of(app);
    let (shown, cache, groups) = app.chat_line_cache();
    let width = padded.width as usize;
    let reflowed = cache.update(shown, (width, measure), theme, spinner, &groups);
    let gap = cache.block_gap();
    let BlockHits {
        mut toggle_rows,
        image_hits,
        code_hits,
        link_hits,
        message_rows,
        last_prompt_row,
    } = BlockHits::collect(cache, gap);
    let spans = targets.spans(cache);
    let tail_gap = usize::from(message_rows > 0 && !tail.lines.is_empty());
    let tail_start = message_rows + tail_gap;
    let tail_toggle = push_tail_toggles(&mut toggle_rows, app, tail, tail_start);
    let content = tail_start + tail.lines.len();
    let height = padded.height as usize;
    let padding = end_padding(app.terminal_pane(), last_prompt_row, content, height);
    let total = content + padding;
    let flash = flash::settle_jump(app, spans, (total, height));
    let start = app.sync_chat_scroll(total, height, reflowed);
    app.set_chat_layout(toggle_rows, start);
    app.set_chat_rows(Some((padded, start)));
    RowLayout {
        image_hits,
        code_hits,
        link_hits,
        tail_toggle,
        tail_gap,
        gap,
        total,
        padding,
        start,
        flash,
    }
}

/// What the cached blocks hold that the pointer acts on, counted from
/// the conversation's first row.
struct BlockHits {
    toggle_rows: Vec<ToggleRow>,
    image_hits: Vec<ImageHit>,
    code_hits: Vec<CodeCopyHit>,
    link_hits: Vec<LinkHit>,
    /// Rows the blocks take, the gaps between them included
    message_rows: usize,
    /// First row of the latest prompt
    last_prompt_row: Option<usize>,
}

impl BlockHits {
    fn collect(cache: &ChatLineCache, gap: usize) -> Self {
        let mut toggle_rows = Vec::new();
        let mut image_hits = Vec::new();
        let mut code_hits = Vec::new();
        let mut link_hits: Vec<LinkHit> = Vec::new();
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
            code_hits.extend(block.codes.iter().map(|hit| CodeCopyHit {
                line: message_rows + hit.line,
                ..hit.clone()
            }));
            // Numbered across the chat, so hovering one link lights up
            // its rows and not another block's link of the same number.
            let shift = (message_rows, 0, LinkHit::numbers(&link_hits));
            link_hits.extend(block.links.iter().map(|hit| hit.clone().moved(shift)));
            message_rows += block.lines.len();
        }
        Self {
            toggle_rows,
            image_hits,
            code_hits,
            link_hits,
            message_rows,
            last_prompt_row,
        }
    }
}

/// Add the row of `tail` starting at `tail_start` that toggles something,
/// the working line's, and return it.
fn push_tail_toggles(
    toggle_rows: &mut Vec<ToggleRow>,
    app: &App,
    tail: &LiveTail,
    tail_start: usize,
) -> Option<ToggleRow> {
    let tail_toggle = app
        .turn_started_ms()
        .zip(tail.status_row)
        .map(|(started, row)| ToggleRow {
            line: tail_start + row,
            target: ToggleTarget::TurnRecords(started),
            open: false,
        });
    toggle_rows.extend(tail_toggle);
    tail_toggle
}

/// Blank rows after the end, enough that scrolling to the bottom can
/// bring the latest prompt to the top with the reply filling in below,
/// and never fewer than the conversation's breathing room, so a reply
/// that reaches the bottom scrolls up with space left under it. The
/// terminal's output runs to the bottom row, as a terminal's does.
fn end_padding(
    terminal: bool,
    last_prompt_row: Option<usize>,
    content: usize,
    height: usize,
) -> usize {
    let room = if terminal { 0 } else { breathing_room(height) };
    last_prompt_row
        .map_or(0, |row| (row + height).saturating_sub(content))
        .max(room)
}
/// Draw `lines` into `padded`, then pack its `prose` rows; returns what
/// the front end needs of each prose row, and for each packed one, by its
/// row in `lines`, where its columns went.
fn draw_rows(
    frame: &mut Frame,
    padded: Rect,
    (lines, prose): (Vec<Line<'static>>, Vec<(usize, u16)>),
    measure: Measure,
    theme: &Theme,
) -> (Vec<ProseRow>, PackedRows) {
    let mut packed = Vec::new();
    let prose_lines: Vec<(usize, u16, Line<'static>)> = prose
        .into_iter()
        .map(|(row, indent)| (row, indent.min(padded.width), lines[row].clone()))
        .collect();
    frame.render_widget(Paragraph::new(lines), padded);
    let buffer = frame.buffer_mut();
    let rows = prose_lines
        .into_iter()
        .map(|(index, indent, line)| {
            #[allow(clippy::cast_possible_truncation)]
            let row = Rect {
                y: padded.y + index as u16,
                height: 1,
                ..padded
            };
            let (width, code) = if measure.is_grid() {
                prose_columns(&line, row, theme)
            } else {
                let (columns, map) =
                    pack_prose_mapped(buffer, &line, (row, indent), measure, theme);
                packed.push((index, map));
                columns
            };
            ProseRow::new(buffer, (row.x, row.y), (width, indent), code)
        })
        .collect();
    (rows, packed)
}

/// Light up the image caption or the copy button the pointer (`hover`)
/// is on, when it falls in the `height` rows of `padded`.
fn highlight_hovered(
    buffer: &mut Buffer,
    hover: Option<HoverTarget>,
    layout: &RowLayout,
    (padded, height): (Rect, usize),
    theme: &Theme,
) {
    let start = layout.start;
    flash::paint(buffer, layout.flash.clone(), (padded, start), theme);
    let hovered_image = match hover {
        Some(HoverTarget::ChatImage(message, index)) => Some((message, index)),
        _ => None,
    };
    if let Some(hit) = layout
        .image_hits
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
            crate::renderer::images::highlight_caption(buffer, row, theme);
        }
    }
    if let Some(HoverTarget::CopyCode(line)) = hover {
        if let Some(hit) = layout.code_hits.iter().find(|hit| hit.line == line) {
            highlight_copy_button(buffer, hit, (padded, start), theme);
        }
    }
}

/// Draw a hovered code block's copy button in the accent, so it reads as
/// something to click; `start` is the chat line at the top of `padded`.
fn highlight_copy_button(
    buffer: &mut Buffer,
    hit: &CodeCopyHit,
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
