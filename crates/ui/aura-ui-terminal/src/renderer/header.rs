//! A panel's header bar: its tabs, or a title and an info cell, inside a
//! bordered frame of their own, above the panel's body.
//!
//! ```text
//! ┌──────────┬──────────┬───────────┐
//! │   CHAT   │▓TERMINAL▓│  PLANNER  │
//! └──────────┴──────────┴───────────┘
//! ```

use super::text::display_width;
use crate::app::tab_bar::TabLayout;
use crate::app::{
    AgentsTab, ChatTab, Pane, PaneChrome, PaneTab, RecordsTab, SidePanelTab, SideScope,
};
use crate::components::truncate_record_info;
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

/// Narrowest bar with room for a close button.
const CLOSE_MIN_WIDTH: u16 = 8;

/// Narrowest bar with room for an expand button, beside tabs or a close
/// button.
const EXPAND_MIN_WIDTH: u16 = 20;

/// Columns an expand button takes from a tabbed bar: its cell and the
/// divider before it.
const EXPAND_COLUMNS: u16 = 4;

/// The expand button's glyph, and the one that restores the panel.
const EXPAND_GLYPH: &str = "⤢";
const RESTORE_GLYPH: &str = "⤡";

/// Narrowest bar with room for a scope button or an icon tab beside its
/// tabs.
const SCOPE_MIN_WIDTH: u16 = 20;

/// Columns a button's cell at the end of a tabbed bar from [`bar_end`]
/// adds to its label: a space on each side, or none when the tabs need
/// the room.
const BUTTON_PADDING: u16 = 2;

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

/// The cells of `bar`'s close button: `✕` with a space on each side,
/// against the right border on the row with the labels.
pub(super) fn close_cell(bar: Rect) -> Option<Rect> {
    if bar.width < CLOSE_MIN_WIDTH || bar.height == 0 {
        return None;
    }
    Some(Rect::new(bar.right() - 4, label_row(bar), 3, 1))
}

/// The cells of `bar`'s expand button. Beside a close button it is the
/// glyph and the space before it, left of the `✕`; otherwise a cell of
/// its own against the right border, after the tabs' last divider.
///
/// ```text
/// │ src/main.rs     100 LINES ⤢ ✕ │
/// │  CHAT  │ TERMINAL │ GRAPH │ ⤢ │
/// ```
pub(super) fn expand_cell(bar: Rect, beside_close: bool) -> Option<Rect> {
    if bar.width < EXPAND_MIN_WIDTH || bar.height == 0 {
        return None;
    }
    let row = label_row(bar);
    Some(if beside_close {
        Rect::new(bar.right() - 6, row, 2, 1)
    } else {
        Rect::new(bar.right() - 4, row, 3, 1)
    })
}

/// The width a tabbed bar leaves its tabs when it ends in an expand
/// button: the tab layout for this width puts the last divider before
/// the button's cell.
pub(super) fn tabs_beside_expand(bar: Rect) -> u16 {
    if expand_cell(bar, false).is_some() {
        bar.width - EXPAND_COLUMNS
    } else {
        bar.width
    }
}

/// Where the buttons at the right end of a tabbed bar fall, and the
/// columns they leave its tabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct BarEnd {
    /// Width to lay the tabs out in: their last divider is the one before
    /// the first button
    pub tabs: u16,
    pub icon: Option<Rect>,
    pub scope: Option<Rect>,
    pub expand: Option<Rect>,
}

/// The buttons at the right end of a tabbed bar that asks for an icon
/// tab, a scope button, an expand button or any of them, each a cell
/// after a divider of its own: the expand button against the border, the
/// scope button left of it and the icon left of that.
///
/// ```text
/// │ SBX │REPO │ PRC │TRACE│TASKS│ 🖥  │ * │ ⤢ │
/// │SBX │REPO│PRC │TRC │TSK │🖥 │*│⤢│
/// ```
///
/// The cells are padded unless the tabs, laid out by `layout` for the
/// width left to them, would not fit beside them; then they shrink to
/// their labels, and on a bar too narrow even for that the expand button
/// goes. A bar too narrow for the others alone keeps them padded and clips
/// its tabs; under [`SCOPE_MIN_WIDTH`] it has no buttons.
pub(super) fn bar_end<T: Copy + PartialEq>(
    bar: Rect,
    (icon, scope, expand): (Option<&str>, bool, bool),
    layout: impl Fn(u16) -> TabLayout<T>,
) -> BarEnd {
    let room = bar.width >= SCOPE_MIN_WIDTH && bar.height > 0;
    let icon = icon
        .filter(|_| room)
        .map(|icon| u16::try_from(display_width(icon)).unwrap_or(u16::MAX));
    let scope = scope && room;
    let expand = expand && bar.width >= EXPAND_MIN_WIDTH && bar.height > 0;
    // Padded buttons without the expand button leave the tabs about the
    // room of tight ones with it, so the expand button goes only once
    // those fail too.
    [(expand, BUTTON_PADDING), (expand, 0), (false, 0)]
        .into_iter()
        .map(|(expand, padding)| lay_out_end(bar, (icon, scope, expand), padding))
        .find(|end| layout(end.tabs).fits(end.tabs))
        .unwrap_or_else(|| lay_out_end(bar, (icon, scope, false), BUTTON_PADDING))
}

/// [`BarEnd`] for the buttons asked for, the icon by its width, each
/// `padding` columns wider than its label.
fn lay_out_end(
    bar: Rect,
    (icon, scope, expand): (Option<u16>, bool, bool),
    padding: u16,
) -> BarEnd {
    let row = label_row(bar);
    // The column right of the next button's cell, from the right border.
    let mut right = bar.right().saturating_sub(1);
    let mut place = |label: Option<u16>| {
        label.map(|label| {
            let cell = label + padding;
            let x = right.saturating_sub(cell);
            right = x.saturating_sub(1);
            Rect::new(x, row, cell, 1)
        })
    };
    let expand = place(expand.then_some(1));
    let scope = place(scope.then_some(1));
    let icon = place(icon);
    BarEnd {
        tabs: (right + 1).saturating_sub(bar.x).min(bar.width),
        icon,
        scope,
        expand,
    }
}

/// The row of `bar` its labels are drawn on.
const fn label_row(bar: Rect) -> u16 {
    if bar.height >= FRAMED_HEIGHT {
        bar.y + 1
    } else {
        bar.y
    }
}

/// A bar's expand button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ExpandButton {
    /// Whether the panel fills the screen, or the column, so the button
    /// restores it
    pub expanded: bool,
    pub hovered: bool,
    /// Where it is drawn, from [`expand_cell`] or [`bar_end`]; `None` on
    /// a bar too narrow for it
    pub cell: Option<Rect>,
}

/// A bar's scope button, [`SideScope::BUTTON`], lit while the panel's
/// lists show the whole project's items rather than the active agent's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ScopeButton {
    pub on: bool,
    pub hovered: bool,
    /// Where it is drawn, from [`bar_end`]; `None` on a bar too narrow
    /// for it
    pub cell: Option<Rect>,
}

/// A tab a bar draws as its icon, after its other tabs: lit as they are
/// when shown or under the pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct IconTab<T> {
    pub tab: T,
    /// Two columns, the second blank (see [`SidePanelTab::icon`])
    pub icon: &'static str,
    /// Where it is drawn, from [`bar_end`]; `None` on a bar too narrow
    /// for it
    pub cell: Option<Rect>,
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
    /// A close button at the right end, `Some(true)` while the pointer is
    /// over it
    pub close: Option<bool>,
    /// An expand button at the right end, left of any close button. A
    /// tabbed bar lays out its tabs in [`tabs_beside_expand`] columns, or
    /// [`bar_end`]'s beside a scope button.
    pub expand: Option<ExpandButton>,
    /// A scope button at the right end of a tabbed bar, left of any
    /// expand button; the bar lays out its tabs in [`bar_end`]'s columns.
    pub scope: Option<ScopeButton>,
    /// A tab drawn as its icon, left of any scope button
    pub icon_tab: Option<IconTab<T>>,
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

impl HostTab for RecordsTab {
    fn pane_tab(self) -> Option<PaneTab> {
        Some(PaneTab::Records(self))
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
            icon_tab: self.icon_tab.and_then(|icon| icon.tab.pane_tab()),
            active: self.active.and_then(HostTab::pane_tab),
            title: self.title.map(str::to_string),
            info: self.info.as_ref().map(|info| info.text.clone()),
            info_short: self.info.as_ref().map(|info| info.short.clone()),
            info_color: self.info.as_ref().map_or(Color::Reset, |info| info.color),
            closable: self.close.is_some(),
            expand: self.expand.map(|button| button.expanded),
            scope: self.scope.map(|button| button.on),
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
    let frame_style = Style::default().fg(super::chrome::border_color(header.focused, theme));
    let buf = frame.buffer_mut();
    if header.hosted {
        draw_hosted_frame(buf, bar, frame_style);
        return;
    }
    let framed = bar.height >= FRAMED_HEIGHT;
    let middle = if framed { bar.y + 1 } else { bar.y };

    let last = bar.width - 1;
    let close = header.close.zip(close_cell(bar));
    let scope = header.scope.and_then(|button| Some((button, button.cell?)));
    let expand = header
        .expand
        .and_then(|button| Some((button, button.cell?)));
    let icon_tab = header.icon_tab.and_then(|icon| Some((icon, icon.cell?)));
    // A tabbed bar's buttons each sit after a divider of their own, and
    // its tabs stop at the first of those.
    let button_dividers: Vec<u16> = if header.layout.cells.is_empty() {
        Vec::new()
    } else {
        [
            icon_tab.map(|(_, cell)| cell),
            scope.map(|(_, cell)| cell),
            expand.map(|(_, cell)| cell),
        ]
        .into_iter()
        .flatten()
        .map(|cell| cell.x.saturating_sub(bar.x).saturating_sub(1))
        .filter(|&column| column > 0 && column < last)
        .collect()
    };
    let limit = button_dividers.iter().copied().min().unwrap_or(last);
    let dividers: Vec<u16> = header
        .layout
        .cells
        .iter()
        .map(|cell| cell.start + cell.width)
        .filter(|&column| column < limit)
        .chain(button_dividers.iter().copied())
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
        if cell.start >= limit {
            break;
        }
        let width = cell.width.min(limit - cell.start);
        let style = tab_style(
            header.active == Some(cell.tab),
            header.hovered == Some(cell.tab),
            header.focused,
            theme,
        );
        let area = Rect::new(bar.x + cell.start, middle, width, 1);
        draw_centered(buf, area, cell.label, style);
    }
    if let Some((icon, cell)) = icon_tab {
        let style = tab_style(
            header.active == Some(icon.tab),
            header.hovered == Some(icon.tab),
            header.focused,
            theme,
        );
        buf.set_style(cell, style);
        // Both of the icon's columns centered, the blank one included.
        let width = u16::try_from(display_width(icon.icon)).unwrap_or(u16::MAX);
        let x = cell.x + cell.width.saturating_sub(width) / 2;
        buf.set_string(x, cell.y, icon.icon, style);
    }
    if let Some((button, cell)) = scope {
        let style = tab_style(button.on, button.hovered, header.focused, theme);
        draw_centered(buf, cell, SideScope::BUTTON, style);
    }

    if let Some((start, width)) = header.layout.info {
        let buttons = u16::from(close.is_some()) + u16::from(expand.is_some());
        let width = width.min(last - start).saturating_sub(2 * buttons);
        let area = Rect::new(bar.x + start, middle, width, 1);
        draw_info(buf, area, header, theme);
    }
    let button_style = |hovered| {
        if hovered {
            Style::default()
                .bg(colors.surface_hover)
                .fg(colors.foreground)
        } else {
            Style::default().fg(colors.muted)
        }
    };
    if let Some((hovered, cell)) = close {
        draw_centered(buf, cell, "✕", button_style(hovered));
    }
    if let Some((button, cell)) = expand {
        let style = button_style(button.hovered);
        let glyph = if button.expanded {
            RESTORE_GLYPH
        } else {
            EXPAND_GLYPH
        };
        buf.set_style(cell, style);
        // After the space of a padded cell; a tight cell is the glyph.
        buf.set_string(cell.x + u16::from(cell.width > 1), cell.y, glyph, style);
    }
}

/// A tab cell's style: the shown tab is bold, filled with the selection
/// color while its panel has focus; a tab under the pointer is shaded.
pub(super) fn tab_style(active: bool, hovered: bool, focused: bool, theme: &Theme) -> Style {
    let colors = &theme.colors;
    if active {
        let style = Style::default()
            .fg(colors.foreground)
            .add_modifier(Modifier::BOLD);
        if focused {
            style.bg(colors.selection).fg(colors.on_selection)
        } else {
            style
        }
    } else if hovered {
        Style::default()
            .bg(colors.surface_hover)
            .fg(colors.secondary)
    } else {
        Style::default().fg(colors.muted)
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
/// there is a title, otherwise the info centered, or on the right beside
/// tabs. The info shortens, then drops, before the title is cut.
fn draw_info<T>(buf: &mut Buffer, area: Rect, header: &PanelHeader<'_, T>, theme: &Theme) {
    let colors = &theme.colors;
    let room = usize::from(area.width);
    let Some(title) = header.title else {
        if let Some(info) = &header.info {
            let style = Style::default().fg(info.color);
            if header.layout.cells.is_empty() {
                let text = [&info.text, &info.short]
                    .into_iter()
                    .find(|text| display_width(text) + 2 <= room)
                    .unwrap_or(&info.short);
                draw_centered(buf, area, text, style);
            } else if let Some(text) = [&info.text, &info.short]
                .into_iter()
                .find(|text| display_width(text) + 2 <= room)
            {
                // Where an untabbed bar has it, a space in from the border.
                let width = u16::try_from(display_width(text) + 1).unwrap_or(u16::MAX);
                buf.set_string(area.x + area.width - width, area.y, text, style);
            }
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
            close: None,
            expand: None,
            scope: None,
            icon_tab: None,
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
        assert_eq!(active.bg, Some(colors.selection));
        assert_eq!(active.fg, Some(colors.on_selection));
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
        assert_eq!(unfocused[(0, 0)].style().fg, Some(colors.faint));
        assert_eq!(focused[(0, 0)].style().fg, Some(colors.success));
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
            close: None,
            expand: None,
            scope: None,
            icon_tab: None,
            focused: true,
            hosted: true,
        };
        let chrome = tabs.chrome(Pane::Chat, area, bar);
        assert_eq!(
            chrome.tabs,
            [
                PaneTab::Chat(ChatTab::Chat),
                PaneTab::Chat(ChatTab::Terminal),
                PaneTab::Chat(ChatTab::Graph),
                PaneTab::Chat(ChatTab::Time),
                PaneTab::Chat(ChatTab::Log)
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
            close: None,
            expand: None,
            scope: None,
            icon_tab: None,
            focused: false,
            hosted: true,
        };
        let chrome = titled.chrome(Pane::Records, area, bar);
        assert!(!chrome.closable);
        assert!(
            PanelHeader {
                close: Some(false),
                ..titled
            }
            .chrome(Pane::Chat, area, bar)
            .closable
        );
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
            close: None,
            expand: None,
            scope: None,
            icon_tab: None,
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

    #[test]
    fn tabs_with_an_info_cell_keep_the_count_on_the_right() {
        const LABELS: [&str; 2] = ["KERNEL", "MEMBERS"];
        let header = |width| PanelHeader {
            layout: TabLayout::with_info([0, 1], |i| LABELS[i], |i| &LABELS[i][..3], width),
            active: Some(0),
            hovered: None,
            title: None,
            info: Some(HeaderInfo::count(128, "RECORDS", Color::Gray)),
            close: None,
            expand: None,
            scope: None,
            icon_tab: None,
            focused: false,
            hosted: false,
        };
        let wide = draw(40, 10, &header(40));
        assert_eq!(row(&wide, 0), "┌─────────┬─────────┬──────────────────┐");
        assert_eq!(row(&wide, 1), "│ KERNEL  │ MEMBERS │      128 RECORDS │");
        assert_eq!(row(&wide, 2), "└─────────┴─────────┴──────────────────┘");
        let narrow = draw(28, 10, &header(28));
        assert_eq!(row(&narrow, 1), "│ KERNEL  │ MEMBERS │  128 │");
        let tight = draw(23, 10, &header(23));
        assert_eq!(
            row(&tight, 1),
            "│  KERNEL  │ MEMBERS  │",
            "no room left for it"
        );
    }

    #[test]
    fn closable_bars_end_in_a_close_button_lit_under_the_pointer() {
        let header = |hovered| PanelHeader::<()> {
            layout: TabLayout::untabbed(30),
            active: None,
            hovered: None,
            title: Some("RECORD #60"),
            info: Some(HeaderInfo::count(128, "LINES", Color::Gray)),
            close: Some(hovered),
            expand: None,
            scope: None,
            icon_tab: None,
            focused: true,
            hosted: false,
        };
        let idle = draw(30, 10, &header(false));
        assert_eq!(row(&idle, 1), "│ RECORD #60     128 LINES ✕ │");
        assert_eq!(
            close_cell(Rect::new(0, 0, 30, 3)),
            Some(Rect::new(26, 1, 3, 1))
        );
        let colors = Theme::cyber().colors;
        assert_eq!(idle[(27, 1)].style().fg, Some(colors.muted));
        let lit = draw(30, 10, &header(true));
        assert_eq!(lit[(27, 1)].style().bg, Some(colors.surface_hover));
    }

    #[test]
    fn tabbed_bars_end_in_an_expand_button_of_their_own() {
        let bar = Rect::new(0, 0, 40, 3);
        let tabs = tabs_beside_expand(bar);
        assert_eq!(tabs, 36);
        let header = |expanded, hovered| PanelHeader {
            expand: Some(ExpandButton {
                expanded,
                hovered,
                cell: expand_cell(bar, false),
            }),
            ..chat_header(tabs, true, None)
        };
        let idle = draw(40, 10, &header(false, false));
        assert_eq!(row(&idle, 0), "┌─────────────────┬────────────────┬───┐");
        assert_eq!(row(&idle, 1), "│      CHAT       │    TERMINAL    │ ⤢ │");
        assert_eq!(row(&idle, 2), "└─────────────────┴────────────────┴───┘");
        assert_eq!(expand_cell(bar, false), Some(Rect::new(36, 1, 3, 1)));
        let colors = Theme::cyber().colors;
        assert_eq!(idle[(37, 1)].style().fg, Some(colors.muted));

        let lit = draw(40, 10, &header(true, true));
        assert_eq!(lit[(37, 1)].symbol(), "⤡", "expanded, it restores");
        assert_eq!(lit[(36, 1)].style().bg, Some(colors.surface_hover));

        assert_eq!(tabs_beside_expand(Rect::new(0, 0, 16, 3)), 16);
        assert_eq!(expand_cell(Rect::new(0, 0, 16, 3), false), None);
    }

    /// Where the side panel's bar `width` columns wide puts its buttons,
    /// HOST's icon among them.
    fn side_end(width: u16, (scope, expand): (bool, bool)) -> BarEnd {
        let icon = SidePanelTab::Host.icon();
        bar_end(Rect::new(0, 0, width, 3), (icon, scope, expand), |width| {
            SidePanelTab::layout(width, false)
        })
    }

    /// The side panel's header bar `width` columns wide, ending in HOST's
    /// icon and the buttons asked for.
    fn side_header(
        width: u16,
        buttons: (bool, bool),
        (on, active): (bool, SidePanelTab),
    ) -> PanelHeader<'static, SidePanelTab> {
        let end = side_end(width, buttons);
        PanelHeader {
            layout: SidePanelTab::layout(end.tabs, false),
            active: Some(active),
            hovered: None,
            title: None,
            info: None,
            close: None,
            expand: buttons.1.then_some(ExpandButton {
                expanded: false,
                hovered: false,
                cell: end.expand,
            }),
            scope: buttons.0.then_some(ScopeButton {
                on,
                hovered: false,
                cell: end.scope,
            }),
            icon_tab: Some(IconTab {
                tab: SidePanelTab::Host,
                icon: SidePanelTab::Host.icon().unwrap(),
                cell: end.icon,
            }),
            focused: false,
            hosted: false,
        }
    }

    /// [`side_header`] drawn on a screen ten rows tall.
    fn draw_side(width: u16, buttons: (bool, bool), state: (bool, SidePanelTab)) -> Buffer {
        draw(width, 10, &side_header(width, buttons, state))
    }

    #[test]
    fn scoped_bars_end_in_a_star_button_lit_while_on() {
        let end = side_end(35, (true, false));
        assert_eq!(end.tabs, 26);
        assert_eq!(end.icon, Some(Rect::new(26, 1, 4, 1)));
        assert_eq!(end.scope, Some(Rect::new(31, 1, 3, 1)));
        assert_eq!(end.expand, None);
        let off = draw_side(35, (true, false), (false, SidePanelTab::Sandbox));
        assert_eq!(row(&off, 0), "┌────┬────┬────┬────┬────┬────┬───┐");
        assert_eq!(row(&off, 1), "│SBX │REPO│PRC │TRC │TSK │ 🖥  │ * │");
        assert_eq!(row(&off, 2), "└────┴────┴────┴────┴────┴────┴───┘");
        let colors = Theme::cyber().colors;
        assert_eq!(off[(32, 1)].style().fg, Some(colors.muted));

        let on = draw_side(35, (true, false), (true, SidePanelTab::Sandbox));
        assert_eq!(on[(32, 1)].style().fg, Some(colors.foreground));
        assert!(on[(32, 1)].style().add_modifier.contains(Modifier::BOLD));

        let narrow = side_end(16, (true, false));
        assert_eq!((narrow.tabs, narrow.icon, narrow.scope), (16, None, None));
    }

    #[test]
    fn hosts_icon_is_a_tab_lit_while_shown_and_centered() {
        let colors = Theme::cyber().colors;
        let off = draw_side(35, (true, false), (false, SidePanelTab::Sandbox));
        assert_eq!(off[(27, 1)].symbol(), "🖥");
        assert_eq!(off[(27, 1)].style().fg, Some(colors.muted));
        let shown = draw_side(35, (true, false), (false, SidePanelTab::Host));
        for x in 26..30 {
            let fg = shown[(x, 1)].style().fg;
            assert_eq!(fg, Some(colors.foreground), "column {x}");
        }
        assert!(shown[(27, 1)].style().add_modifier.contains(Modifier::BOLD));
        assert_eq!(
            shown[(6, 1)].style().fg,
            Some(colors.muted),
            "the REPO label is not lit"
        );
    }

    #[test]
    fn the_expand_button_sits_right_of_the_star_button() {
        let end = side_end(44, (true, true));
        assert_eq!(end.tabs, 31);
        assert_eq!(end.icon, Some(Rect::new(31, 1, 4, 1)));
        assert_eq!(end.scope, Some(Rect::new(36, 1, 3, 1)));
        assert_eq!(end.expand, Some(Rect::new(40, 1, 3, 1)));
        let buffer = draw_side(44, (true, true), (false, SidePanelTab::Sandbox));
        assert_eq!(
            row(&buffer, 0),
            "┌─────┬─────┬─────┬─────┬─────┬────┬───┬───┐"
        );
        assert_eq!(
            row(&buffer, 1),
            "│ SBX │REPO │ PRC │TRACE│TASKS│ 🖥  │ * │ ⤢ │"
        );
        assert_eq!(
            row(&buffer, 2),
            "└─────┴─────┴─────┴─────┴─────┴────┴───┴───┘"
        );
    }

    #[test]
    fn narrow_bars_tighten_their_buttons_then_drop_the_expand_button() {
        // Five short labels and their dividers need 21 columns: 34 leaves
        // them that beside HOST's icon and two buttons, padded, 33 does
        // not. Tight ones leave them room down to 28; at 27 the expand
        // button goes, and under 20 there are no buttons.
        let end = side_end(34, (true, true));
        assert_eq!(end.tabs, 21);
        assert_eq!(end.icon, Some(Rect::new(21, 1, 4, 1)));
        assert_eq!(end.scope, Some(Rect::new(26, 1, 3, 1)));
        assert_eq!(end.expand, Some(Rect::new(30, 1, 3, 1)));
        let header = |width| side_header(width, (true, true), (false, SidePanelTab::Sandbox));
        let buffer = draw(34, 10, &header(34));
        assert_eq!(row(&buffer, 1), "│SBX│RPO│PRC│TRC│TSK│ 🖥  │ * │ ⤢ │");

        let end = side_end(33, (true, true));
        assert_eq!(end.tabs, 26);
        assert_eq!(end.icon, Some(Rect::new(26, 1, 2, 1)));
        assert_eq!(end.scope, Some(Rect::new(29, 1, 1, 1)));
        assert_eq!(end.expand, Some(Rect::new(31, 1, 1, 1)));
        let buffer = draw(33, 10, &header(33));
        assert_eq!(row(&buffer, 0), "┌────┬────┬────┬────┬────┬──┬─┬─┐");
        assert_eq!(row(&buffer, 1), "│SBX │REPO│PRC │TRC │TSK │🖥 │*│⤢│");
        assert_eq!(row(&buffer, 2), "└────┴────┴────┴────┴────┴──┴─┴─┘");

        let end = side_end(28, (true, true));
        assert_eq!(end.tabs, 21);
        assert_eq!(end.expand, Some(Rect::new(26, 1, 1, 1)));

        let end = side_end(27, (true, true));
        assert_eq!(end.expand, None, "the expand button goes first");
        assert_eq!((end.tabs, end.scope), (22, Some(Rect::new(25, 1, 1, 1))));
        assert_eq!(end.icon, Some(Rect::new(22, 1, 2, 1)));

        let bare = side_end(19, (true, true));
        assert_eq!(
            (bare.tabs, bare.icon, bare.scope, bare.expand),
            (19, None, None, None)
        );
    }

    #[test]
    fn tabs_too_many_for_a_tight_button_keep_a_padded_one() {
        // Five short labels and their dividers need 21 columns, more than
        // a 21-column bar leaves beside even one tight button.
        const SHORT: [&str; 5] = ["ONE", "TWO", "SIX", "TEN", "ART"];
        let five = |width| TabLayout::new([0, 1, 2, 3, 4], |i| SHORT[i], |i| SHORT[i], width);
        let clipped = bar_end(Rect::new(0, 0, 21, 3), (None, true, true), five);
        assert_eq!(clipped.expand, None);
        assert_eq!(
            (clipped.tabs, clipped.scope),
            (17, Some(Rect::new(17, 1, 3, 1))),
            "tabs that fit beside nothing keep a padded button"
        );
    }

    #[test]
    fn the_expand_button_sits_left_of_a_close_button() {
        let header = PanelHeader::<()> {
            layout: TabLayout::untabbed(30),
            active: None,
            hovered: None,
            title: Some("RECORD #60"),
            info: Some(HeaderInfo::count(128, "LINES", Color::Gray)),
            close: Some(false),
            expand: Some(ExpandButton {
                expanded: false,
                hovered: false,
                cell: expand_cell(Rect::new(0, 0, 30, 3), true),
            }),
            scope: None,
            icon_tab: None,
            focused: true,
            hosted: false,
        };
        let buffer = draw(30, 10, &header);
        assert_eq!(row(&buffer, 1), "│ RECORD #60   128 LINES ⤢ ✕ │");
        assert_eq!(
            expand_cell(Rect::new(0, 0, 30, 3), true),
            Some(Rect::new(24, 1, 2, 1))
        );
    }
}
