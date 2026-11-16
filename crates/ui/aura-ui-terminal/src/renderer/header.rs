//! A panel's header bar: its tabs, or a title and an info cell, inside a
//! bordered frame of their own, above the panel's body.
//!
//! ```text
//! ┌──────────┬──────────┬───────────┐
//! │   CHAT   │▓TERMINAL▓│  PLANNER  │
//! └──────────┴──────────┴───────────┘
//! ```

use super::panels::truncate_record_info;
use super::text::display_width;
use crate::app::tab_bar::TabLayout;
use crate::app::{AgentsTab, ChatTab, Pane, PaneChrome, PaneTab, SidePanelTab};
use crate::{App, Theme};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    Frame,
};

/// Rows of a framed header bar.
const FRAMED_HEIGHT: u16 = 3;

/// Rows of a bar the front end draws (see [`crate::App::set_host_chrome`]): the
/// frame's top and an empty row. The body's top border closes it.
const HOSTED_HEIGHT: u16 = 2;

/// Shortest panel given a framed bar; shorter ones get its middle row.
const FRAMED_MIN_PANEL_HEIGHT: u16 = 6;

/// Split a panel's area into its header bar and the body below it.
/// `hosted` bars are the front end's to draw.
pub(super) fn split(area: Rect, hosted: bool) -> (Rect, Rect) {
    let height = match (area.height >= FRAMED_MIN_PANEL_HEIGHT, hosted) {
        (true, true) => HOSTED_HEIGHT,
        (true, false) => FRAMED_HEIGHT,
        (false, _) => area.height.min(1),
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
    /// Whether the front end draws the bar: the grid keeps only its frame
    pub hosted: bool,
}

/// A tab of a header bar, as the front end names it.
pub(super) trait HostTab: Copy + PartialEq {
    fn pane_tab(self) -> Option<PaneTab>;
}

impl HostTab for () {
    fn pane_tab(self) -> Option<PaneTab> {
        None
    }
}

impl HostTab for AgentsTab {
    fn pane_tab(self) -> Option<PaneTab> {
        Some(PaneTab::Agents(self))
    }
}

impl HostTab for ChatTab {
    fn pane_tab(self) -> Option<PaneTab> {
        Some(PaneTab::Chat(self))
    }
}

impl HostTab for SidePanelTab {
    fn pane_tab(self) -> Option<PaneTab> {
        Some(PaneTab::Side(self))
    }
}

impl<T: HostTab> PanelHeader<'_, T> {
    /// What the front end draws for this bar of `pane`, which covers `area`.
    pub(super) fn chrome(&self, pane: Pane, area: Rect, bar: Rect) -> PaneChrome {
        PaneChrome {
            pane,
            area,
            bar,
            tabs: self
                .layout
                .cells
                .iter()
                .filter_map(|cell| cell.tab.pane_tab())
                .collect(),
            active: self.active.and_then(HostTab::pane_tab),
            title: self.title.map(str::to_string),
            info: self.info.as_ref().map(|info| info.text.clone()),
            info_short: self.info.as_ref().map(|info| info.short.clone()),
            info_color: self.info.as_ref().map_or(Color::Reset, |info| info.color),
            focused: self.focused,
        }
    }
}

/// Draw the header of `pane`, which covers `area`, into `bar`; a hosted
/// one is also handed to the front end to draw over its frame.
pub(super) fn show<T: HostTab>(
    frame: &mut Frame,
    app: &mut App,
    (pane, area, bar): (Pane, Rect, Rect),
    header: &PanelHeader<'_, T>,
    theme: &Theme,
) {
    render_panel_header(frame, bar, header, theme);
    if header.hosted {
        app.push_pane_chrome(header.chrome(pane, area, bar));
    }
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
    if header.hosted {
        draw_hosted_frame(buf, bar, frame_style);
        return;
    }
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
            let mut style = Style::default()
                .fg(colors.foreground)
                .add_modifier(Modifier::BOLD);
            if header.focused {
                style = style.bg(colors.primary).fg(colors.background);
            }
            style
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

/// The frame of a bar the front end draws: its top and sides around empty
/// rows, open at the bottom for the body's top border to close.
fn draw_hosted_frame(buf: &mut Buffer, bar: Rect, style: Style) {
    let inner = usize::from(bar.width.saturating_sub(2));
    let mut rows = bar.rows();
    if bar.height > 1 {
        if let Some(top) = rows.next() {
            buf.set_string(top.x, top.y, format!("┌{}┐", "─".repeat(inner)), style);
        }
    }
    for row in rows {
        buf.set_string(row.x, row.y, format!("│{}│", " ".repeat(inner)), style);
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
    let room = usize::from(area.width);
    let Some(title) = header.title else {
        if let Some(info) = &header.info {
            let text = [&info.text, &info.short]
                .into_iter()
                .find(|text| display_width(text) + 2 <= room)
                .unwrap_or(&info.short);
            draw_centered(buf, area, text, Style::default().fg(info.color));
        }
        return;
    };
    let title = truncate_record_info(title, room.saturating_sub(2));
    let title_style = Style::default()
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
        Style::default().fg(info.color),
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
                let (bar, _) = split(frame.area(), header.hosted);
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
            info: None,
            focused,
            hosted: false,
        }
    }

    #[test]
    fn tabs_sit_between_dividers_that_join_the_frame() {
        let buffer = draw(40, 10, &chat_header(40, true, None));
        assert_eq!(row(&buffer, 0), "┌───────────────────┬──────────────────┐");
        assert_eq!(row(&buffer, 1), "│       CHAT        │     TERMINAL     │");
        assert_eq!(row(&buffer, 2), "└───────────────────┴──────────────────┘");
    }

    #[test]
    fn the_active_tab_is_filled_with_inverted_text() {
        let colors = Theme::cyber().colors;
        let focused = draw(40, 10, &chat_header(40, true, Some(1)));
        let active = focused[(1, 1)].style();
        assert_eq!(active.bg, Some(colors.primary));
        assert_eq!(active.fg, Some(colors.background));
        let hovered = focused[(25, 1)].style();
        assert_eq!(hovered.bg, Some(colors.surface_hover));
        assert_eq!(hovered.fg, Some(colors.secondary));

        let unfocused = draw(40, 10, &chat_header(40, false, None));
        let active = unfocused[(1, 1)].style();
        assert_eq!(active.bg, Some(Color::Reset));
        assert_eq!(active.fg, Some(colors.foreground));
        assert!(active.add_modifier.contains(Modifier::BOLD));
        let idle = unfocused[(25, 1)].style();
        assert_eq!(idle.bg, Some(Color::Reset));
        assert_eq!(idle.fg, Some(colors.muted));
        assert_eq!(unfocused[(0, 0)].style().fg, Some(colors.muted));
    }

    #[test]
    fn short_panels_get_the_tab_row_only() {
        let buffer = draw(40, 5, &chat_header(40, true, None));
        assert_eq!(row(&buffer, 0), "│       CHAT        │     TERMINAL     │");
        assert_eq!(split(Rect::new(0, 0, 40, 5), false).1.y, 1);
    }

    #[test]
    fn hosted_bars_keep_only_their_frame_for_the_front_end() {
        let header = PanelHeader {
            hosted: true,
            ..chat_header(20, true, None)
        };
        let buffer = draw(20, 10, &header);
        assert_eq!(row(&buffer, 0), "┌──────────────────┐");
        assert_eq!(row(&buffer, 1), "│                  │");
        assert_eq!(split(Rect::new(0, 0, 20, 10), true).1.y, 2);
        assert_eq!(split(Rect::new(0, 0, 20, 5), true).1.y, 1);
    }

    #[test]
    fn hosted_bars_tell_the_front_end_what_they_show() {
        let area = Rect::new(4, 1, 30, 12);
        let bar = Rect::new(4, 1, 30, 2);
        let tabs = PanelHeader {
            layout: ChatTab::layout(30),
            active: Some(ChatTab::Terminal),
            hovered: None,
            title: None,
            info: None,
            focused: true,
            hosted: true,
        };
        let chrome = tabs.chrome(Pane::Chat, area, bar);
        assert_eq!(
            chrome.tabs,
            [
                PaneTab::Chat(ChatTab::Chat),
                PaneTab::Chat(ChatTab::Terminal)
            ]
        );
        assert_eq!(chrome.active, Some(PaneTab::Chat(ChatTab::Terminal)));
        assert_eq!((chrome.area, chrome.bar), (area, bar));
        assert!(chrome.focused);

        let titled = PanelHeader::<()> {
            layout: TabLayout::untabbed(30),
            active: None,
            hovered: None,
            title: Some("KERNEL"),
            info: Some(HeaderInfo::count(128, "RECORDS", Color::Gray)),
            focused: false,
            hosted: true,
        };
        let chrome = titled.chrome(Pane::Records, area, bar);
        assert!(chrome.tabs.is_empty());
        assert_eq!(chrome.title.as_deref(), Some("KERNEL"));
        assert_eq!(chrome.info.as_deref(), Some("128 RECORDS"));
        assert_eq!(chrome.info_short.as_deref(), Some("128"));
        assert_eq!(chrome.info_color, Color::Gray);
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
            hosted: false,
        };
        let wide = draw(30, 10, &header(30));
        assert_eq!(row(&wide, 1), "│ KERNEL         128 RECORDS │");
        assert_eq!(wide[(2, 1)].style().bg, Some(Color::Reset));
        assert_eq!(wide[(10, 1)].style().bg, Some(Color::Reset));
        let narrow = draw(16, 10, &header(16));
        assert_eq!(row(&narrow, 1), "│ KERNEL   128 │");
        let tiny = draw(10, 10, &header(10));
        assert_eq!(row(&tiny, 1), "│ KERNEL │");
    }
}
