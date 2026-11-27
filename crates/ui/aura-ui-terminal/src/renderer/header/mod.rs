//! A panel's header bar: its tabs, or a title and an info cell, inside a
//! bordered frame of their own, above the panel's body.
//!
//! ```text
//! ┌──────────┬──────────┬───────────┐
//! │   CHAT   │▓TERMINAL▓│  PLANNER  │
//! └──────────┴──────────┴───────────┘
//! ```

mod draw;

use crate::app::tab_bar::TabLayout;
#[cfg(doc)]
use crate::app::SideScope;
use crate::app::{AgentsTab, ChatTab, Pane, PaneChrome, PaneTab, RecordsTab, SidePanelTab};
use crate::renderer::text::display_width;
use crate::{App, Theme};
pub(super) use draw::{render_icon_tooltip, render_panel_header, tab_style};
use ratatui::{layout::Rect, style::Color, Frame};

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
    /// Two columns: HOST's computer and a blank (see
    /// [`SidePanelTab::icon`]), or BENCH's wrench (see [`AgentsTab::icon`])
    pub icon: &'static str,
    /// The tab's name, shown under its icon while the pointer is over it
    /// (see [`render_icon_tooltip`])
    pub tooltip: Option<&'static str>,
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

#[cfg(test)]
mod tests;
