//! A panel's header bar: its tabs and an info cell inside a bordered
//! frame of their own, above the panel's body.
//!
//! ```text
//! ┌──────────┬──────────┬───────────┐
//! │   CHAT   │▓TERMINAL▓│  PLANNER  │
//! └──────────┴──────────┴───────────┘
//! ```

use super::panels::truncate_record_info;
use super::text::display_width;
use crate::app::tab_bar::TabLayout;
use crate::Theme;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    Frame,
};

/// Rows of a framed header bar.
const FRAMED_HEIGHT: u16 = 3;

/// Shortest panel given a framed bar; shorter ones get its middle row.
const FRAMED_MIN_PANEL_HEIGHT: u16 = 6;

/// Split a panel's area into its header bar and the body below it.
pub(super) fn split(area: Rect) -> (Rect, Rect) {
    let height = if area.height >= FRAMED_MIN_PANEL_HEIGHT {
        FRAMED_HEIGHT
    } else {
        area.height.min(1)
    };
    let bar = Rect { height, ..area };
    let body = Rect {
        y: area.y + height,
        height: area.height - height,
        ..area
    };
    (bar, body)
}

/// Text for a header's info cell. `short` stands in when `text` does not
/// fit.
pub(super) struct HeaderInfo {
    pub text: String,
    pub short: String,
    pub color: Color,
}

impl HeaderInfo {
    /// `4 AGENTS`, shortened to `4`.
    pub(super) fn count(count: impl std::fmt::Display, unit: &str, color: Color) -> Self {
        Self {
            text: format!("{count} {unit}"),
            short: count.to_string(),
            color,
        }
    }
}

/// What a panel shows in its header bar.
pub(super) struct PanelHeader<'a, T> {
    pub layout: TabLayout<T>,
    pub active: Option<T>,
    pub hovered: Option<T>,
    /// Drawn on the left of the info cell, for panels without tabs
    pub title: Option<&'a str>,
    pub info: Option<HeaderInfo>,
    pub focused: bool,
}

/// Draw `header` into `bar`, the area `split` gave it.
pub(super) fn render_panel_header<T: Copy + PartialEq>(
    frame: &mut Frame,
    bar: Rect,
    header: &PanelHeader<'_, T>,
    theme: &Theme,
) {
    if bar.width < 2 || bar.height == 0 {
        return;
    }
    let colors = &theme.colors;
    let frame_style = Style::default().fg(if header.focused {
        colors.primary
    } else {
        colors.muted
    });
    let buf = frame.buffer_mut();
    let framed = bar.height >= FRAMED_HEIGHT;
    let middle = if framed { bar.y + 1 } else { bar.y };

    let last = bar.width - 1;
    let dividers: Vec<u16> = header
        .layout
        .cells
        .iter()
        .map(|cell| cell.start + cell.width)
        .filter(|&column| column < last)
        .collect();
    if framed {
        let edge = |left: &str, join: &str, right: &str| -> String {
            (0..bar.width)
                .map(|column| match column {
                    0 => left,
                    c if c == last => right,
                    c if dividers.contains(&c) => join,
                    _ => "─",
                })
                .collect()
        };
        buf.set_string(bar.x, bar.y, edge("┌", "┬", "┐"), frame_style);
        buf.set_string(bar.x, bar.y + 2, edge("└", "┴", "┘"), frame_style);
    }
    for column in [0, last].into_iter().chain(dividers.iter().copied()) {
        buf.set_string(bar.x + column, middle, "│", frame_style);
    }

    for cell in &header.layout.cells {
        if cell.start >= last {
            break;
        }
        let width = cell.width.min(last - cell.start);
        let style = if header.active == Some(cell.tab) {
            let fill = if header.focused {
                colors.primary
            } else {
                colors.muted
            };
            Style::default()
                .bg(fill)
                .fg(colors.background)
                .add_modifier(Modifier::BOLD)
        } else if header.hovered == Some(cell.tab) {
            Style::default()
                .bg(colors.surface_hover)
                .fg(colors.secondary)
        } else {
            Style::default().fg(colors.muted)
        };
        let area = Rect::new(bar.x + cell.start, middle, width, 1);
        draw_centered(buf, area, cell.label, style);
    }

    if let Some((start, width)) = header.layout.info {
        let area = Rect::new(bar.x + start, middle, width.min(last - start), 1);
        draw_info(buf, area, header, theme);
    }
}

/// `text` centered in the one-row `area`, which is filled with `style`.
fn draw_centered(buf: &mut Buffer, area: Rect, text: &str, style: Style) {
    buf.set_style(area, style);
    let text = truncate_record_info(text, usize::from(area.width));
    let width = u16::try_from(display_width(&text)).unwrap_or(u16::MAX);
    let x = area.x + area.width.saturating_sub(width) / 2;
    buf.set_string(x, area.y, text, style);
}

/// The info cell: the title on the left and the info on the right when
/// there is a title, otherwise the info centered. The info shortens, then
/// drops, before the title is cut.
fn draw_info<T>(buf: &mut Buffer, area: Rect, header: &PanelHeader<'_, T>, theme: &Theme) {
    let colors = &theme.colors;
    let fill = Style::default().bg(colors.surface);
    let room = usize::from(area.width);
    let Some(title) = header.title else {
        buf.set_style(area, fill);
        if let Some(info) = &header.info {
            let text = [&info.text, &info.short]
                .into_iter()
                .find(|text| display_width(text) + 2 <= room)
                .unwrap_or(&info.short);
            draw_centered(buf, area, text, fill.fg(info.color));
        }
        return;
    };
    buf.set_style(area, fill);
    let title = truncate_record_info(title, room.saturating_sub(2));
    let title_style = fill
        .fg(if header.focused {
            colors.primary
        } else {
            colors.foreground
        })
        .add_modifier(Modifier::BOLD);
    buf.set_string(area.x + 1, area.y, &title, title_style);
    let left = display_width(&title) + 1;
    let Some(info) = &header.info else {
        return;
    };
    let fits = |text: &str| left + 1 + display_width(text) < room;
    let Some(text) = [&info.text, &info.short]
        .into_iter()
        .find(|text| fits(text))
    else {
        return;
    };
    let width = u16::try_from(display_width(text) + 1).unwrap_or(u16::MAX);
    buf.set_string(
        area.x + area.width - width,
        area.y,
        text,
        fill.fg(info.color),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{backend::TestBackend, Terminal};

    const TABS: [&str; 2] = ["CHAT", "TERMINAL"];

    fn draw<T: Copy + PartialEq>(width: u16, height: u16, header: &PanelHeader<'_, T>) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| {
                let (bar, _) = split(frame.area());
                render_panel_header(frame, bar, header, &Theme::cyber());
            })
            .unwrap();
        terminal.backend().buffer().clone()
    }

    fn row(buffer: &Buffer, y: u16) -> String {
        (0..buffer.area.width)
            .map(|x| buffer[(x, y)].symbol())
            .collect()
    }

    fn chat_header(
        width: u16,
        focused: bool,
        hovered: Option<usize>,
    ) -> PanelHeader<'static, usize> {
        PanelHeader {
            layout: TabLayout::new([0, 1], |i| TABS[i], |i| TABS[i], width),
            active: Some(0),
            hovered,
            title: None,
            info: Some(HeaderInfo::count(4, "AGENTS", Color::White)),
            focused,
        }
    }

    #[test]
    fn tabs_sit_between_dividers_that_join_the_frame() {
        let buffer = draw(40, 10, &chat_header(40, true, None));
        assert_eq!(row(&buffer, 0), "┌──────────┬──────────┬────────────────┐");
        assert_eq!(row(&buffer, 1), "│   CHAT   │ TERMINAL │    4 AGENTS    │");
        assert_eq!(row(&buffer, 2), "└──────────┴──────────┴────────────────┘");
    }

    #[test]
    fn the_active_tab_is_filled_with_inverted_text() {
        let colors = Theme::cyber().colors;
        let focused = draw(40, 10, &chat_header(40, true, Some(1)));
        let active = focused[(1, 1)].style();
        assert_eq!(active.bg, Some(colors.primary));
        assert_eq!(active.fg, Some(colors.background));
        let hovered = focused[(12, 1)].style();
        assert_eq!(hovered.bg, Some(colors.surface_hover));
        assert_eq!(hovered.fg, Some(colors.secondary));
        assert_eq!(focused[(25, 1)].style().bg, Some(colors.surface));

        let unfocused = draw(40, 10, &chat_header(40, false, None));
        assert_eq!(unfocused[(1, 1)].style().bg, Some(colors.muted));
        assert_eq!(unfocused[(0, 0)].style().fg, Some(colors.muted));
    }

    #[test]
    fn short_panels_get_the_tab_row_only() {
        let buffer = draw(40, 5, &chat_header(40, true, None));
        assert_eq!(row(&buffer, 0), "│   CHAT   │ TERMINAL │    4 AGENTS    │");
        assert_eq!(split(Rect::new(0, 0, 40, 5)).1.y, 1);
    }

    #[test]
    fn untabbed_bars_show_the_title_and_shorten_the_info() {
        let header = |width| PanelHeader::<()> {
            layout: TabLayout::untabbed(width),
            active: None,
            hovered: None,
            title: Some("KERNEL"),
            info: Some(HeaderInfo::count(128, "RECORDS", Color::Gray)),
            focused: false,
        };
        let wide = draw(30, 10, &header(30));
        assert_eq!(row(&wide, 1), "│ KERNEL         128 RECORDS │");
        let narrow = draw(16, 10, &header(16));
        assert_eq!(row(&narrow, 1), "│ KERNEL   128 │");
        let tiny = draw(10, 10, &header(10));
        assert_eq!(row(&tiny, 1), "│ KERNEL │");
    }
}
