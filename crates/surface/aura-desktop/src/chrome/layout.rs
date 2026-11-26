//! Where the bars' boxes and the dividers' bands sit, and what a point
//! is over.

use super::{Band, BarLayout, Boxed, Divider, Hit, Info, Kind, Layout, Model, SIDE};
use crate::geom::{Point, Rectangle, Size};
use crate::grid;
use crate::tab_bar::{self, unit};
use aura_ui_terminal::{Pane, PaneChrome, PaneTab, SideScope};
use ratatui::layout::Rect;
use unicode_width::UnicodeWidthStr;

/// The area of `which`, if the model has that panel.
fn find(model: &Model, which: Pane) -> Option<Rect> {
    model
        .panes
        .iter()
        .find(|pane| pane.pane == which)
        .map(|pane| pane.area)
}

/// The record panel, or the side panel when there is none.
fn column(model: &Model) -> Option<Rect> {
    find(model, Pane::Records).or_else(|| find(model, Pane::Side))
}

/// Where the panels are, in cells: left, right, and the right column's
/// top and bottom.
fn span(model: &Model) -> (u16, u16, u16, u16) {
    let left = model.panes.iter().map(|p| p.area.x).min().unwrap_or(0);
    let right = model
        .panes
        .iter()
        .map(|p| p.area.right())
        .max()
        .unwrap_or(0);
    let column = column(model);
    (
        left,
        right,
        column.map_or(0, |c| c.y),
        find(model, Pane::Side).or(column).map_or(0, Rect::bottom),
    )
}

/// The pixel position of cell `(col, row)`.
fn at(model: &Model, col: u16, row: u16) -> Point {
    let cell = model.metrics.cell;
    Point::new(
        model.origin.x + f32::from(col) * cell.width,
        model.origin.y + f32::from(row) * cell.height,
    )
}

/// The gap and the border on its right, beside the chat's rows. The
/// border on its left is left alone: the panel there draws its
/// scrollbar on it.
fn upright(model: &Model, divider: Divider, left: Rect, right: Rect, rows: Rect) -> Band {
    let cell = model.metrics.cell;
    let gap = right.x.saturating_sub(left.right());
    let middle = f32::from(left.right()) + f32::from(gap + 1) / 2.0;
    let edge = match divider {
        Divider::Agents => left.right(),
        Divider::Right | Divider::Records => right.x,
    };
    Band {
        divider,
        rect: Rectangle::new(
            at(model, left.right(), rows.y),
            Size::new(
                f32::from(gap + 1) * cell.width,
                f32::from(rows.height) * cell.height,
            ),
        ),
        lead: middle - f32::from(edge),
    }
}

/// The dividers' bands, unless an overlay covers a bar.
fn bands(model: &Model) -> Vec<Band> {
    let mut bands = Vec::new();
    if model.covered {
        return bands;
    }
    let cell = model.metrics.cell;
    if let (Some(agents), Some(chat)) = (find(model, Pane::Agents), find(model, Pane::Chat)) {
        bands.push(upright(model, Divider::Agents, agents, chat, chat));
    }
    if let (Some(chat), Some(column)) = (find(model, Pane::Chat), column(model)) {
        bands.push(upright(model, Divider::Right, chat, column, chat));
    }
    if let (Some(records), Some(side)) = (find(model, Pane::Records), find(model, Pane::Side)) {
        if records.x == side.x && side.y == records.bottom() {
            bands.push(Band {
                divider: Divider::Records,
                rect: Rectangle::new(
                    at(model, side.x, side.y - 1),
                    Size::new(f32::from(side.width) * cell.width, 2.0 * cell.height),
                ),
                lead: 0.0,
            });
        }
    }
    bands
}

impl Layout {
    pub fn of(model: &Model) -> Self {
        let bars = model
            .panes
            .iter()
            .filter(|pane| pane.bar.height >= 2)
            .map(|pane| bar_layout(model, pane))
            .collect();
        let side_toggle = model
            .side_toggle
            .map(|(left, right)| side_toggle_box(model, left, right));
        Self {
            bars,
            bands: bands(model),
            span: span(model),
            side_toggle,
        }
    }

    /// Dividers win over the bars they cross.
    pub fn hit(&self, at: Point) -> Option<Hit> {
        if let Some(band) = self.bands.iter().find(|band| band.rect.contains(at)) {
            return Some(Hit::Divider(band.divider));
        }
        if self.side_toggle.is_some_and(|rect| rect.contains(at)) {
            return Some(Hit::SideToggle);
        }
        let bar = self.bars.iter().find(|bar| bar.rect.contains(at))?;
        if bar.close.is_some_and(|close| close.contains(at)) {
            return Some(Hit::Close(bar.pane));
        }
        if bar.expand.is_some_and(|(expand, _)| expand.contains(at)) {
            return Some(Hit::Expand(bar.pane));
        }
        if bar.scope.is_some_and(|(scope, _)| scope.contains(at)) {
            return Some(Hit::Scope(bar.pane));
        }
        if let Some((_, tab)) = bar.icon_tab.filter(|(icon, _)| icon.contains(at)) {
            return Some(Hit::Tab(tab));
        }
        let tab = bar.boxes.iter().find_map(|boxed| match boxed.kind {
            Kind::Tab(tab) if boxed.rect.contains(at) => Some(tab),
            _ => None,
        });
        Some(tab.map_or(Hit::Bar(bar.pane), Hit::Tab))
    }
}

/// The hamburger box: as big as the window's close box and right under
/// it, its right edge on the close box's (see the tab bar's model in
/// `shell.rs`, which lines its boxes up with the panels' outer borders
/// from column `left` to `right`), and centered on the header row, in
/// the half cell above it and the half below that the border of the
/// panels' tops leaves.
fn side_toggle_box(model: &Model, left: u16, right: u16) -> Rectangle {
    let cell = model.metrics.cell;
    let side = tab_bar::box_height(&model.metrics);
    let bar_x = (model.origin.x + (f32::from(left) + 0.5) * cell.width).round();
    let end =
        (bar_x + f32::from(right.saturating_sub(left).saturating_sub(1)) * cell.width).round();
    let x = (end - side).round();
    let top = (model.origin.y + (cell.height - side) / 2.0).round();
    Rectangle::new(Point::new(x, top), Size::new(end - x, side))
}

/// The sizes one bar's boxes are laid out with.
struct Row {
    unit: f32,
    height: f32,
    top: f32,
    gap: f32,
    start: f32,
    /// Where the room for boxes ends; moves left as boxes take the
    /// right end
    end: f32,
}

impl Row {
    /// How wide the box of `text` is.
    fn width(&self, text: &str) -> f32 {
        ((text.width() as f32 + 2.0 * SIDE) * self.unit).round()
    }

    /// A square box at the right end, when `wanted` and there is room.
    fn square(&mut self, wanted: bool) -> Option<Rectangle> {
        (wanted && self.end - self.start >= 4.0 * self.height).then(|| {
            let square = Rectangle::new(
                Point::new(self.end - self.height, self.top),
                Size::new(self.height, self.height),
            );
            self.end = square.x - self.gap;
            square
        })
    }

    /// The `*` box at the right end, when there is room.
    fn scope(&mut self, on: bool) -> Option<(Rectangle, bool)> {
        // A one-glyph label still gets a box at least as wide as tall.
        let w = self.width(SideScope::BUTTON).max(self.height);
        (self.end - self.start >= 3.0 * w).then(|| {
            let rect = Rectangle::new(
                Point::new(self.end - w, self.top),
                Size::new(w, self.height),
            );
            self.end = rect.x - self.gap;
            (rect, on)
        })
    }

    /// The square box of `tab`, drawn as its icon, at the right end,
    /// when there is room.
    fn icon_tab(&mut self, tab: PaneTab) -> Option<(Rectangle, PaneTab)> {
        (self.end - self.start >= 4.0 * self.height).then(|| {
            let rect = Rectangle::new(
                Point::new(self.end - self.height, self.top),
                Size::new(self.height, self.height),
            );
            self.end = rect.x - self.gap;
            (rect, tab)
        })
    }
}

/// The inside of a bar's frame, in pixels.
fn bar_rect(model: &Model, bar: Rect) -> Rectangle {
    let cell = model.metrics.cell;
    Rectangle::new(
        Point::new(
            model.origin.x + (f32::from(bar.x) + 0.5) * cell.width,
            model.origin.y + (f32::from(bar.y) + 0.5) * cell.height,
        ),
        Size::new(
            f32::from(bar.width.saturating_sub(1)) * cell.width,
            f32::from(bar.height) * cell.height,
        ),
    )
}

/// The sizes of the boxes in `rect`.
fn row(model: &Model, rect: Rectangle) -> Row {
    let cell = model.metrics.cell;
    let unit = unit(&model.metrics);
    let height = (cell.height * 1.25).round().min(rect.height - 4.0);
    let margin = ((rect.height - height) / 2.0).floor();
    Row {
        unit,
        height,
        top: (rect.center_y() - height / 2.0).round(),
        gap: (unit * 0.4).round().max(2.0),
        start: (rect.x + margin).round(),
        end: (rect.x + rect.width - margin).round(),
    }
}

/// The title and tab labels, short or long.
fn items(pane: &PaneChrome, short: bool) -> Vec<(String, Kind)> {
    let title = pane.title.iter().map(|t| (t.clone(), Kind::Title));
    let tabs = pane.tabs.iter().map(|&tab| {
        let label = if short {
            tab.short_label()
        } else {
            tab.label()
        };
        (label.to_string(), Kind::Tab(tab))
    });
    title.chain(tabs).collect()
}

/// The label boxes packed from the left, and where the next would
/// start: short labels when the long ones don't fit, and boxes that
/// still don't fit left out.
fn label_boxes(row: &Row, pane: &PaneChrome) -> (Vec<Boxed>, f32) {
    let room = |items: &[(String, Kind)]| {
        items
            .iter()
            .map(|(text, _)| row.width(text) + row.gap)
            .sum::<f32>()
            - row.gap
            <= row.end - row.start
    };
    let long = items(pane, false);
    let items = if room(&long) { long } else { items(pane, true) };

    let mut boxes = Vec::with_capacity(items.len());
    let mut x = row.start;
    for (text, kind) in items {
        let w = row.width(&text);
        if x + w > row.end {
            break;
        }
        boxes.push(Boxed {
            rect: Rectangle::new(Point::new(x, row.top), Size::new(w, row.height)),
            text,
            kind,
        });
        x += w + row.gap;
    }
    (boxes, x)
}

/// The panel's info, long or short, if one fits right of `after`.
fn info(model: &Model, pane: &PaneChrome, row: &Row, rect: Rectangle, after: f32) -> Option<Info> {
    let (unit, end) = (row.unit, row.end);
    [&pane.info, &pane.info_short]
        .into_iter()
        .flatten()
        .find(|text| end - text.width() as f32 * unit >= after)
        .map(|text| Info {
            text: text.clone(),
            color: grid::color(pane.info_color, model.palette.muted),
            end: Point::new(end - (unit * SIDE).round(), rect.center_y()),
        })
}

/// The boxes of one bar, packed from the left; short labels when the
/// long ones don't fit, and boxes that still don't fit left out.
pub(super) fn bar_layout(model: &Model, pane: &PaneChrome) -> BarLayout {
    let rect = bar_rect(model, pane.bar);
    let mut row = row(model, rect);
    let close = row.square(pane.closable);
    let expand = pane
        .expand
        .and_then(|expanded| Some((row.square(true)?, expanded)));
    let scope = pane.scope.and_then(|on| row.scope(on));
    let icon_tab = pane.icon_tab.and_then(|tab| row.icon_tab(tab));
    let (boxes, x) = label_boxes(&row, pane);
    let after = if boxes.is_empty() {
        row.start
    } else {
        x + row.gap
    };
    let info = info(model, pane, &row, rect, after);
    BarLayout {
        pane: pane.pane,
        rect,
        focused: pane.focused,
        active: pane.active,
        boxes,
        info,
        close,
        expand,
        scope,
        icon_tab,
    }
}
