//! The panels' title bars, drawn over the framed, empty rows the terminal
//! UI keeps for them (`App::set_host_chrome`): boxes packed to the left,
//! each as wide as its label and always outlined, then the panel's info
//! at the right end. A panel without tabs shows its title in a filled
//! box; a panel with tabs fills the box of the one it shows. Fills take
//! the selection color while their panel has focus; otherwise the box is
//! only outlined. A bar over a view that closes, such as a file shown
//! over the chat, ends in a close box; a panel that expands to fill the
//! window has an expand box left of it. A panel whose lists can show the
//! whole project's items rather than the active agent's ends in a `*`
//! box, filled like a shown tab while it is on, left of its expand box.
//! A tab shown as its icon, HOST's computer, has a square box left of
//! that, lit as the other tabs are.
//!
//! The dividers between panels live here too: bands over the gap between
//! two panels and the border right of it, which drag to move the border.
//! The border left of the gap stays the panel's: the agents and chat
//! panels draw their scrollbars on it.

// Cell coordinates become pixel positions.
#![allow(clippy::cast_precision_loss)]

mod layout;
mod quads;

use crate::geom::{Color, Point, Rectangle};
use crate::grid::{Metrics, Palette};
use aura_ui_terminal::{Pane, PaneChrome, PaneSplits, PaneTab};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

/// Label characters of padding on each side of a label.
const SIDE: f32 = 1.0;

/// A border between panels that can be dragged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Divider {
    /// Between the agents column and the chat
    Agents,
    /// Between the chat and the right column
    Right,
    /// Between the record panel and the panel below it
    Records,
}

impl Divider {
    pub const fn sideways(self) -> bool {
        !matches!(self, Self::Records)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    Tab(PaneTab),
    /// The close box at the right end of a bar
    Close(Pane),
    /// The box that expands a panel to fill the window, or restores it
    Expand(Pane),
    /// The `*` box, which switches a panel's lists between the active
    /// agent's items and the whole project's
    Scope(Pane),
    /// The rest of a title bar
    Bar(Pane),
    Divider(Divider),
    /// The hamburger box under the window's close box, which hides or
    /// shows the right column
    SideToggle,
}

/// What the bars show, and where the grid sits.
#[derive(Debug, Clone, PartialEq)]
pub struct Model {
    pub panes: Vec<PaneChrome>,
    /// Whether an overlay covers any bar, which then leaves the dividers
    /// alone too
    pub covered: bool,
    /// When the header row keeps the hamburger button's cells blank for
    /// the front end to draw, and nothing covers them: the columns left
    /// and right of every panel, which the window's boxes line up with
    pub side_toggle: Option<(u16, u16)>,
    /// Top-left corner of the grid
    pub origin: Point,
    pub metrics: Metrics,
    pub palette: Palette,
}

impl Model {
    /// The bars in `panes` that `buffer` still shows empty: an overlay
    /// drawn over a bar keeps it. `side_toggle` is where the header keeps
    /// the hamburger button's cells, if it does.
    pub fn new(
        panes: &[PaneChrome],
        side_toggle: Option<Rect>,
        buffer: &Buffer,
        at: (Point, Metrics, Palette),
    ) -> Self {
        let (origin, metrics, palette) = at;
        let shown: Vec<PaneChrome> = panes
            .iter()
            .filter(|pane| is_empty(pane.bar, buffer))
            .cloned()
            .collect();
        let edges = panes
            .iter()
            .map(|pane| pane.area.x)
            .min()
            .zip(panes.iter().map(|pane| pane.area.right()).max());
        let side_toggle =
            edges.filter(|_| side_toggle.is_some_and(|cells| is_blank(cells, buffer)));
        Self {
            covered: shown.len() < panes.len(),
            panes: shown,
            side_toggle,
            origin,
            metrics,
            palette,
        }
    }
}

/// Whether every cell of `area` is blank and unpainted.
fn is_blank(area: Rect, buffer: &Buffer) -> bool {
    area.positions().all(|at| {
        buffer
            .cell(at)
            .is_some_and(|cell| cell.symbol() == " " && unpainted(cell))
    })
}

/// Whether a bar's rows hold only its frame, unpainted.
fn is_empty(bar: Rect, buffer: &Buffer) -> bool {
    let inside = bar.x + 1..bar.right().saturating_sub(1);
    (bar.y..bar.bottom()).all(|y| {
        inside.clone().all(|x| {
            buffer.cell((x, y)).is_some_and(|cell| {
                let symbol = cell.symbol();
                unpainted(cell) && (symbol == " " || (y == bar.y && symbol == "─"))
            })
        })
    })
}

/// Whether nothing painted `cell`'s background. The backdrop behind a
/// dialog paints every cell around it, though it leaves them blank, and
/// the chrome under it must not show.
fn unpainted(cell: &ratatui::buffer::Cell) -> bool {
    cell.bg == ratatui::style::Color::Reset
}

#[derive(Debug, Clone, PartialEq)]
enum Kind {
    Title,
    Tab(PaneTab),
}

#[derive(Debug, Clone, PartialEq)]
struct Boxed {
    rect: Rectangle,
    text: String,
    kind: Kind,
}

#[derive(Debug, Clone, PartialEq)]
struct Info {
    text: String,
    color: Color,
    /// Right end of the text
    end: Point,
}

#[derive(Debug, Clone, PartialEq)]
struct BarLayout {
    pane: Pane,
    /// Inside the frame
    rect: Rectangle,
    focused: bool,
    active: Option<PaneTab>,
    boxes: Vec<Boxed>,
    info: Option<Info>,
    /// The close box, for bars that close what they show
    close: Option<Rectangle>,
    /// The expand box, and whether the panel is expanded
    expand: Option<(Rectangle, bool)>,
    /// The `*` box, and whether it is on
    scope: Option<(Rectangle, bool)>,
    /// The box of the tab drawn as its icon, and the tab
    icon_tab: Option<(Rectangle, PaneTab)>,
}

#[derive(Debug, Clone, PartialEq)]
struct Band {
    divider: Divider,
    rect: Rectangle,
    /// Cells from the edge of the dragged panel to the band's middle,
    /// rightward
    lead: f32,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Layout {
    bars: Vec<BarLayout>,
    bands: Vec<Band>,
    /// Where the panels are, in cells: left, right, and the right
    /// column's top and bottom
    span: (u16, u16, u16, u16),
    /// The hamburger box, when the header has the button
    side_toggle: Option<Rectangle>,
}

/// The bars as they are shown, and what the pointer is over.
#[derive(Default)]
pub struct Chrome {
    model: Option<Model>,
    layout: Layout,
    hover: Option<Hit>,
}

impl Chrome {
    /// Show `model`, with the pointer at `pointer` (from the grid's
    /// corner of the window) when it is over the grid.
    pub fn update(&mut self, model: Model, pointer: Option<Point>) {
        if self.model.as_ref() != Some(&model) {
            self.layout = Layout::of(&model);
            self.model = Some(model);
        }
        self.point(pointer);
    }

    pub fn point(&mut self, pointer: Option<Point>) {
        self.hover = pointer.and_then(|at| self.layout.hit(at));
    }

    /// What the pointer was over when last told where it is.
    pub const fn hover(&self) -> Option<Hit> {
        self.hover
    }

    /// `splits` with `divider` moved to `at`, from the grid's corner of
    /// the window.
    pub fn drag(&self, divider: Divider, at: Point, mut splits: PaneSplits) -> PaneSplits {
        let Some(model) = &self.model else {
            return splits;
        };
        let cell = model.metrics.cell;
        let col = (at.x - model.origin.x) / cell.width;
        let row = (at.y - model.origin.y) / cell.height;
        let (left, right, top, bottom) = self.layout.span;
        let share = |part: f32, whole: u16| {
            Some(if whole == 0 {
                0.0
            } else {
                (part / f32::from(whole)).clamp(0.0, 1.0)
            })
        };
        let wide = right.saturating_sub(left);
        // The middle of the band stands for the edge of the dragged panel.
        let lead = self
            .layout
            .bands
            .iter()
            .find(|band| band.divider == divider)
            .map_or(0.0, |band| band.lead);
        let edge = col - lead;
        match divider {
            Divider::Agents => splits.agents = share(edge - f32::from(left), wide),
            Divider::Right => splits.right = share(f32::from(right) - edge, wide),
            Divider::Records => {
                splits.records = share(row - f32::from(top), bottom.saturating_sub(top));
            }
        }
        splits
    }

    /// The middle of the box of `tab`, its label's or its icon's, if it is
    /// drawn.
    #[cfg(test)]
    pub fn spot(&self, tab: PaneTab) -> Option<Point> {
        self.layout
            .bars
            .iter()
            .find_map(|bar| {
                let labelled = bar.boxes.iter().find(|boxed| boxed.kind == Kind::Tab(tab));
                labelled.map(|boxed| boxed.rect).or_else(|| {
                    let icon = bar.icon_tab.filter(|&(_, icon)| icon == tab);
                    icon.map(|(rect, _)| rect)
                })
            })
            .map(|rect| Point::new(rect.center_x(), rect.center_y()))
    }

    /// The close box of `pane`'s bar, if it is drawn.
    #[cfg(test)]
    pub fn close_box(&self, pane: Pane) -> Option<Rectangle> {
        let bar = self.layout.bars.iter().find(|bar| bar.pane == pane)?;
        bar.close
    }

    /// The expand box of `pane`'s bar, if it is drawn.
    #[cfg(test)]
    pub fn expand_box(&self, pane: Pane) -> Option<Rectangle> {
        let bar = self.layout.bars.iter().find(|bar| bar.pane == pane)?;
        bar.expand.map(|(rect, _)| rect)
    }

    /// The `*` box of `pane`'s bar, if it is drawn.
    #[cfg(test)]
    pub fn scope_box(&self, pane: Pane) -> Option<Rectangle> {
        let bar = self.layout.bars.iter().find(|bar| bar.pane == pane)?;
        bar.scope.map(|(rect, _)| rect)
    }

    /// The hamburger box under the window's close box, if it is drawn.
    #[cfg(test)]
    pub const fn side_toggle_box(&self) -> Option<Rectangle> {
        self.layout.side_toggle
    }

    /// The middle of the band of `divider`, if there is one.
    #[cfg(test)]
    pub fn band(&self, divider: Divider) -> Option<Point> {
        self.layout
            .bands
            .iter()
            .find(|band| band.divider == divider)
            .map(|band| Point::new(band.rect.center_x(), band.rect.center_y()))
    }
}

#[cfg(test)]
mod tests;
