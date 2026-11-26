//! Image thumbnails: under the messages they were sent with, and in a
//! strip over the bottom of the chat while they wait in the input. Each
//! one opens full size when clicked.

use super::text::display_width;
use crate::app::image_preview::ImagePreview;
use crate::app::{HoverTarget, ImageHit};
use crate::components::Message;
use crate::{App, Theme};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};
use std::sync::Arc;

/// Blank columns between two thumbnails.
const GAP: usize = 2;

/// Where one thumbnail landed in [`tile`]'s lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Tile {
    /// First line, counted from the first returned line
    pub line: usize,
    /// Lines it takes, its caption included
    pub rows: usize,
    /// First column
    pub x: usize,
    /// Columns it takes
    pub width: usize,
}

/// `images` side by side from column `indent`, each over its caption,
/// wrapping to another row of thumbnails when `width` runs out.
pub(super) fn tile(
    images: &[(String, &Arc<ImagePreview>)],
    indent: usize,
    width: usize,
    theme: &Theme,
) -> (Vec<Line<'static>>, Vec<Tile>) {
    let caption_style = Style::default().fg(theme.colors.muted);
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut tiles = Vec::with_capacity(images.len());
    let mut rest = images;
    while !rest.is_empty() {
        // As many as fit on this row, at least one.
        let mut x = indent;
        let mut row = Vec::new();
        for (caption, preview) in rest {
            let w = usize::from(preview.thumbnail_width()).max(display_width(caption));
            if !row.is_empty() && x + w > width {
                break;
            }
            row.push((caption, preview, x, w));
            x += w + GAP;
        }
        rest = &rest[row.len()..];
        let thumb_rows = row
            .iter()
            .map(|(_, p, _, _)| p.thumbnail().len())
            .max()
            .unwrap_or(0);
        let top = lines.len();
        for r in 0..=thumb_rows {
            let mut spans = vec![Span::raw(" ".repeat(indent))];
            for (i, (caption, preview, _, w)) in row.iter().enumerate() {
                if i > 0 {
                    spans.push(Span::raw(" ".repeat(GAP)));
                }
                let used = if r == thumb_rows {
                    spans.push(Span::styled((*caption).clone(), caption_style));
                    display_width(caption)
                } else if let Some(line) = preview.thumbnail().get(r) {
                    spans.extend(line.spans.iter().cloned());
                    line.width()
                } else {
                    0
                };
                spans.push(Span::raw(" ".repeat(w.saturating_sub(used))));
            }
            lines.push(Line::from(spans));
        }
        tiles.extend(row.iter().map(|&(_, _, x, w)| Tile {
            line: top,
            rows: thumb_rows + 1,
            x,
            width: w,
        }));
    }
    (lines, tiles)
}

/// Thumbnails of the images sent with `message`, under its text after
/// the `> ` prompt, and where each was drawn.
pub(super) fn message_image_lines(
    message: &Message,
    width: usize,
    theme: &Theme,
) -> (Vec<Line<'static>>, Vec<ImageHit>) {
    let images: Vec<(String, &Arc<ImagePreview>)> = message
        .images()
        .iter()
        .enumerate()
        .map(|(i, p)| (format!("image {}", i + 1), p))
        .collect();
    let (lines, tiles) = tile(&images, 2, width, theme);
    let hits = tiles
        .into_iter()
        .enumerate()
        .map(|(index, t)| ImageHit {
            line: t.line,
            rows: t.rows,
            x: u16::try_from(t.x).unwrap_or(u16::MAX),
            width: u16::try_from(t.width).unwrap_or(u16::MAX),
            message: message.version().0,
            index,
        })
        .collect();
    (lines, hits)
}

/// Underline the caption, the last row, of a hovered thumbnail at `area`.
pub(super) fn highlight_caption(buffer: &mut Buffer, area: Rect, theme: &Theme) {
    if area.height == 0 {
        return;
    }
    let caption = Rect {
        y: area.y + area.height - 1,
        height: 1,
        ..area
    }
    .intersection(buffer.area);
    buffer.set_style(
        caption,
        Style::default()
            .fg(theme.colors.primary)
            .add_modifier(Modifier::UNDERLINED),
    );
}

/// The images waiting in the input, over the bottom of the chat panel
/// `chat`, recording where each thumbnail went for clicks.
pub(super) fn render_pending_images(frame: &mut Frame, chat: Rect, app: &mut App, theme: &Theme) {
    let previews = app.pending_previews();
    if previews.is_empty() || app.detail_view().is_some() || chat.width < 8 || chat.height < 4 {
        app.set_pending_image_areas(Vec::new());
        return;
    }
    let inner_width = usize::from(chat.width.saturating_sub(2));
    let images: Vec<(String, &Arc<ImagePreview>)> = previews
        .iter()
        .map(|(label, p)| (format!("[image {label}]"), p))
        .collect();
    let (lines, tiles) = tile(&images, 1, inner_width, theme);
    let height = u16::try_from(lines.len())
        .unwrap_or(u16::MAX)
        .saturating_add(2)
        .min(chat.height);
    let area = Rect {
        y: chat.bottom() - height,
        height,
        ..chat
    };
    let inner = Rect {
        x: area.x + 1,
        y: area.y + 1,
        width: area.width - 2,
        height: height.saturating_sub(2),
    };
    let block = Block::default()
        .title(" ATTACHED ")
        .title_bottom(Line::from(" click to view ").right_aligned())
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.colors.muted));
    frame.render_widget(Clear, area);
    frame.render_widget(Paragraph::new(lines).block(block), area);

    let areas = pending_image_areas(frame, inner, app, &tiles, &previews, theme);
    app.set_pending_image_areas(areas);
}

/// Where each pending image's thumbnail went inside `inner`, lighting the
/// caption of the one under the pointer.
fn pending_image_areas(
    frame: &mut Frame,
    inner: Rect,
    app: &App,
    tiles: &[Tile],
    previews: &[(usize, Arc<ImagePreview>)],
    theme: &Theme,
) -> Vec<(Rect, usize)> {
    let hovered = match app.hover() {
        Some(HoverTarget::PendingImage(label)) => Some(label),
        _ => None,
    };
    let mut areas = Vec::with_capacity(tiles.len());
    for (t, (label, _)) in tiles.iter().zip(previews) {
        let rect = Rect {
            x: inner.x + u16::try_from(t.x).unwrap_or(u16::MAX),
            y: inner.y + u16::try_from(t.line).unwrap_or(u16::MAX),
            width: u16::try_from(t.width).unwrap_or(u16::MAX),
            height: u16::try_from(t.rows).unwrap_or(u16::MAX),
        }
        .intersection(inner);
        if rect.is_empty() {
            continue;
        }
        if hovered == Some(*label) {
            highlight_caption(frame.buffer_mut(), rect, theme);
        }
        areas.push((rect, *label));
    }
    areas
}
