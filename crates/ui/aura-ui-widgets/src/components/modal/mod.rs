//! A dialog over the whole screen. The screen around it goes dark
//! ([`backdrop`]), so nothing underneath shows or competes with it, and a
//! framed box in the middle holds the dialog's lines. Every dialog draws
//! through a [`Modal`], so they share one frame, title, padding, scrollbar
//! and status line:
//!
//! ```text
//! ┌ NEW CHANNEL ─────────────────────────┐
//! │                                      │
//! │  › Slug                              │
//! │      Zode█                           │
//! │    Title                             │
//! │      zode                            │
//! │                                      │
//! │  Slugs are lowercase letters, digits │
//! │  and dashes                          │
//! │                                      │
//! └──────────────────────────────────────┘
//! ```
//!
//! A dialog shows no key hints or explanations: its rows say what they
//! are, and the only other line is a status, such as an error, while there
//! is one.

mod backdrop;
mod parts;
#[cfg(test)]
mod tests;

pub use backdrop::backdrop;
pub use parts::{caret_line, choice_line, list_item, popup_block};

use super::info::{truncate_line, wrap};
use crate::themes::Theme;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, StatefulWidget,
        Widget,
    },
};

/// Columns of most dialogs, their border included.
pub const STANDARD_WIDTH: u16 = 64;

/// Columns of a dialog that lists paths.
pub const WIDE_WIDTH: u16 = 72;

/// Blank columns inside each side of the border. A cell is about twice as
/// tall as it is wide, so two columns look as wide as one row.
const PAD_X: u16 = 2;

/// Blank rows inside the top and the bottom of the border.
const PAD_Y: u16 = 1;

/// Rows of the border and the padding together.
const CHROME_ROWS: u16 = 2 + 2 * PAD_Y;

/// Columns of the border and the padding together.
const CHROME_COLUMNS: u16 = 2 + 2 * PAD_X;

/// Columns left clear between the box and each side of the screen.
const MARGIN_X: u16 = 2;

/// Rows left clear between the box and the top and bottom of the screen.
const MARGIN_Y: u16 = 1;

/// Most rows a status wraps to.
const STATUS_ROWS: usize = 3;

/// The color of a dialog's border.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tone {
    /// Most dialogs
    #[default]
    Neutral,
    /// A dialog the agent waits on, such as an approval
    Attention,
}

/// A list in a dialog's body, for the scrollbar on the right border
/// beside it while the list is longer than it shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListScroll {
    /// The body line the list's first shown row is on
    pub top: usize,
    /// Rows of the list shown
    pub visible: usize,
    /// Rows in the whole list
    pub total: usize,
    /// The first row shown
    pub first: usize,
}

/// Where a dialog was drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ModalArea {
    /// The box, its border included: a click outside it closes the dialog
    pub frame: Rect,
    /// Where the body's lines went, inside the padding: line `n` is on
    /// row `body.y + n`
    pub body: Rect,
}

/// A dialog's box, its title, border, width and status, which draws the
/// dialog's lines over the darkened screen.
#[derive(Debug, Clone)]
pub struct Modal<'t> {
    theme: &'t Theme,
    title: String,
    meta: Option<String>,
    tone: Tone,
    width: u16,
    status: Option<(String, Color)>,
    scroll: Option<ListScroll>,
}

impl<'t> Modal<'t> {
    /// A [`STANDARD_WIDTH`] dialog titled `title`, which shows as given:
    /// dialogs title themselves in capitals, as panels do.
    #[must_use]
    pub fn new(title: impl Into<String>, theme: &'t Theme) -> Self {
        Self {
            theme,
            title: title.into(),
            meta: None,
            tone: Tone::Neutral,
            width: STANDARD_WIDTH,
            status: None,
            scroll: None,
        }
    }

    /// Text at the right end of the top border, muted, such as `1 of 3`.
    #[must_use]
    pub fn meta(mut self, meta: impl Into<String>) -> Self {
        self.meta = Some(meta.into());
        self
    }

    /// The border's color.
    #[must_use]
    pub const fn tone(mut self, tone: Tone) -> Self {
        self.tone = tone;
        self
    }

    /// Columns of the box, its border included. A narrower screen narrows
    /// it.
    #[must_use]
    pub const fn width(mut self, width: u16) -> Self {
        self.width = width;
        self
    }

    /// A line under the body, after a blank one, in `color`: an error, or
    /// what just happened. It wraps to three rows at most.
    #[must_use]
    pub fn status(mut self, text: impl Into<String>, color: Color) -> Self {
        self.status = Some((text.into(), color));
        self
    }

    /// A scrollbar on the right border beside `scroll`'s rows, drawn
    /// while its list is longer than it shows.
    #[must_use]
    pub const fn scroll(mut self, scroll: ListScroll) -> Self {
        self.scroll = Some(scroll);
        self
    }

    /// Columns of a body line on `screen`.
    #[must_use]
    pub fn body_width(&self, screen: Rect) -> usize {
        usize::from(self.box_width(screen).saturating_sub(CHROME_COLUMNS))
    }

    /// The most body lines that fit on `screen`. A longer body is cut, so
    /// a dialog that lists more rows than this shows a window of them.
    #[must_use]
    pub fn body_room(&self, screen: Rect) -> usize {
        usize::from(usable(screen).height)
            .saturating_sub(usize::from(CHROME_ROWS))
            .saturating_sub(self.status_rows(screen))
    }

    /// Darken `screen` and draw the box in its middle holding `lines`,
    /// cut to [`Modal::body_room`] of them.
    pub fn render(self, buf: &mut Buffer, screen: Rect, lines: Vec<Line<'_>>) -> ModalArea {
        backdrop(buf, screen, self.theme);
        let area = self.layout(screen, lines.len());
        self.draw_frame(buf, area.frame);
        Paragraph::new(lines).render(area.body, buf);
        self.draw_status(buf, screen, area);
        self.draw_scrollbar(buf, area);
        area
    }

    fn box_width(&self, screen: Rect) -> u16 {
        self.width.min(usable(screen).width)
    }

    /// Where the box and its body go on `screen` for a body `lines` long.
    /// The box is centered as if it had no status, which then grows it
    /// downward, so the box keeps still as a status comes and goes.
    fn layout(&self, screen: Rect, lines: usize) -> ModalArea {
        let room = usable(screen);
        let width = self.box_width(screen);
        let body_rows = u16::try_from(lines.min(self.body_room(screen))).unwrap_or(u16::MAX);
        let bare = body_rows.saturating_add(CHROME_ROWS).min(room.height);
        let status = u16::try_from(self.status_rows(screen)).unwrap_or(0);
        let height = bare.saturating_add(status).min(room.height);
        let x = room.x + (room.width - width) / 2;
        let y = (room.y + (room.height - bare) / 2).min(room.bottom() - height);
        let frame = Rect::new(x, y, width, height);
        let body = Rect {
            x: x + 1 + PAD_X,
            y: y + 1 + PAD_Y,
            width: width.saturating_sub(CHROME_COLUMNS),
            height: body_rows,
        };
        ModalArea {
            frame,
            body: body.intersection(frame),
        }
    }

    /// The box's fill, border and title over `frame`.
    fn draw_frame(&self, buf: &mut Buffer, frame: Rect) {
        let colors = &self.theme.colors;
        buf.set_style(frame, Style::default().bg(colors.background));
        let title = Span::styled(
            format!(" {} ", self.title),
            Style::default()
                .fg(colors.primary)
                .add_modifier(Modifier::BOLD),
        );
        let mut block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(self.border_color()))
            .title(Line::from(title));
        if let Some(meta) = &self.meta {
            let meta = Span::styled(format!(" {meta} "), Style::default().fg(colors.muted));
            block = block.title(Line::from(meta).right_aligned());
        }
        block.render(frame, buf);
    }

    const fn border_color(&self) -> Color {
        match self.tone {
            Tone::Neutral => self.theme.colors.muted,
            Tone::Attention => self.theme.colors.warning,
        }
    }

    /// The status's lines, wrapped to the body's width.
    fn status_lines(&self, screen: Rect) -> Vec<String> {
        let Some((text, _)) = &self.status else {
            return Vec::new();
        };
        let width = self.body_width(screen);
        let mut lines = wrap(text, width);
        if lines.len() > STATUS_ROWS {
            lines.truncate(STATUS_ROWS);
            if let Some(last) = lines.last_mut() {
                *last = truncate_line(&format!("{last} …"), width);
            }
        }
        lines
    }

    /// Rows the status takes, the blank one above it included.
    fn status_rows(&self, screen: Rect) -> usize {
        match self.status_lines(screen).len() {
            0 => 0,
            n => n + 1,
        }
    }

    /// The status under the body, after a blank row, where the box has
    /// room for it.
    fn draw_status(&self, buf: &mut Buffer, screen: Rect, area: ModalArea) {
        let Some((_, color)) = &self.status else {
            return;
        };
        let style = Style::default().fg(*color);
        let lines: Vec<Line<'_>> = self
            .status_lines(screen)
            .into_iter()
            .map(|line| Line::from(Span::styled(line, style)))
            .collect();
        let top = area.body.bottom() + 1;
        let inside = area.frame.bottom().saturating_sub(1 + PAD_Y);
        let rows = Rect {
            y: top,
            height: inside.saturating_sub(top),
            ..area.body
        };
        Paragraph::new(lines).render(rows, buf);
    }

    /// The scrollbar on the right border beside the scrolled list's rows,
    /// as a panel draws one, while the list is longer than it shows.
    fn draw_scrollbar(&self, buf: &mut Buffer, area: ModalArea) {
        let Some(scroll) = self.scroll else {
            return;
        };
        let top = area
            .body
            .y
            .saturating_add(u16::try_from(scroll.top).unwrap_or(u16::MAX));
        let bottom = top
            .saturating_add(u16::try_from(scroll.visible).unwrap_or(u16::MAX))
            .min(area.body.bottom());
        if scroll.total <= scroll.visible || area.frame.width == 0 || bottom <= top {
            return;
        }
        let track = Rect::new(area.frame.right() - 1, top, 1, bottom - top);
        let mut state = ScrollbarState::new(scroll.total - scroll.visible + 1)
            .position(scroll.first)
            .viewport_content_length(scroll.visible);
        Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None)
            .track_symbol(Some("│"))
            .track_style(Style::default().fg(self.border_color()))
            .thumb_symbol("┃")
            .thumb_style(Style::default().fg(self.theme.colors.foreground))
            .render(track, buf, &mut state);
    }
}

/// `screen` less the margins the box keeps from its edges, where the
/// screen has room for them.
fn usable(screen: Rect) -> Rect {
    let margin_x = if screen.width > 2 * MARGIN_X {
        MARGIN_X
    } else {
        0
    };
    let margin_y = if screen.height > 2 * MARGIN_Y {
        MARGIN_Y
    } else {
        0
    };
    Rect {
        x: screen.x + margin_x,
        y: screen.y + margin_y,
        width: screen.width - 2 * margin_x,
        height: screen.height - 2 * margin_y,
    }
}
