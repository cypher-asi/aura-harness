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

/// Narrowest bar with room for a scope button beside its tabs.
const SCOPE_MIN_WIDTH: u16 = 20;

/// Columns of a button's cell at the end of a tabbed bar from
/// [`bar_end`]: its one-column label with a space on each side, or the
/// label alone when the tabs need the room.
const PADDED_BUTTON: u16 = 3;
const TIGHT_BUTTON: u16 = 1;

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
    pub scope: Option<Rect>,
    pub expand: Option<Rect>,
}

/// The buttons at the right end of a tabbed bar that asks for a scope
/// button, an expand button or both, each a cell after a divider of its
/// own: the expand button against the border, the scope button left of
/// it.
///
/// ```text
/// │ SBX │ PRC │TRACE│TASKS│ * │ ⤢ │
/// │SBX │PRC │TRC │TSK│*│⤢│
/// ```
///
/// The cells are padded unless the tabs, laid out by `layout` for the
/// width left to them, would not fit beside them; then they shrink to
/// their labels, and on a bar too narrow even for that the expand button
/// goes. A bar too narrow for the scope button alone keeps it padded and
/// clips its tabs; under [`SCOPE_MIN_WIDTH`] it has no buttons.
pub(super) fn bar_end<T: Copy + PartialEq>(
    bar: Rect,
    (scope, expand): (bool, bool),
    layout: impl Fn(u16) -> TabLayout<T>,
) -> BarEnd {
    let scope = scope && bar.width >= SCOPE_MIN_WIDTH && bar.height > 0;
    let expand = expand && bar.width >= EXPAND_MIN_WIDTH && bar.height > 0;
    // A padded scope button alone leaves the tabs the room of two tight
    // buttons, so the expand button goes only once those fail too.
    [
        (expand, PADDED_BUTTON),
        (expand, TIGHT_BUTTON),
        (false, TIGHT_BUTTON),
    ]
    .into_iter()
    .map(|(expand, cell)| lay_out_end(bar, (scope, expand), cell))
    .find(|end| layout(end.tabs).fits(end.tabs))
    .unwrap_or_else(|| lay_out_end(bar, (scope, false), PADDED_BUTTON))
}

/// [`BarEnd`] for the buttons asked for, in cells `cell` columns wide.
fn lay_out_end(bar: Rect, (scope, expand): (bool, bool), cell: u16) -> BarEnd {
    let row = label_row(bar);
    // The column right of the next button's cell, from the right border.
    let mut right = bar.right().saturating_sub(1);
    let mut place = |wanted: bool| {
        wanted.then(|| {
            let x = right.saturating_sub(cell);
            right = x.saturating_sub(1);
            Rect::new(x, row, cell, 1)
        })
    };
    let expand = place(expand);
    let scope = place(scope);
    BarEnd {
        tabs: (right + 1).saturating_sub(bar.x).min(bar.width),
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
    // A tabbed bar's buttons each sit after a divider of their own, and
    // its tabs stop at the first of those.
    let button_dividers: Vec<u16> = if header.layout.cells.is_empty() {
        Vec::new()
    } else {
        [scope.map(|(_, cell)| cell), expand.map(|(_, cell)| cell)]
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
fn tab_style(active: bool, hovered: bool, focused: bool, theme: &Theme) -> Style {
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
mod tests;
