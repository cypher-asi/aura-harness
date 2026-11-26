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

mod quads;

use crate::geom::{Color, Point, Rectangle, Size};
use crate::grid::{self, Metrics, Palette};
use crate::tab_bar::{self, unit};
use aura_ui_terminal::{Pane, PaneChrome, PaneSplits, PaneTab, SideScope};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use unicode_width::UnicodeWidthStr;

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

/// Whether every cell of `area` is blank.
fn is_blank(area: Rect, buffer: &Buffer) -> bool {
    area.positions()
        .all(|at| buffer.cell(at).is_some_and(|cell| cell.symbol() == " "))
}

/// Whether a bar's rows hold only its frame.
fn is_empty(bar: Rect, buffer: &Buffer) -> bool {
    let inside = bar.x + 1..bar.right().saturating_sub(1);
    (bar.y..bar.bottom()).all(|y| {
        inside.clone().all(|x| {
            buffer.cell((x, y)).is_some_and(|cell| {
                let symbol = cell.symbol();
                symbol == " " || (y == bar.y && symbol == "─")
            })
        })
    })
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

impl Layout {
    pub fn of(model: &Model) -> Self {
        let bars = model
            .panes
            .iter()
            .filter(|pane| pane.bar.height >= 2)
            .map(|pane| bar_layout(model, pane))
            .collect();
        let find = |which: Pane| {
            model
                .panes
                .iter()
                .find(|pane| pane.pane == which)
                .map(|pane| pane.area)
        };
        let left = model.panes.iter().map(|p| p.area.x).min().unwrap_or(0);
        let right = model
            .panes
            .iter()
            .map(|p| p.area.right())
            .max()
            .unwrap_or(0);
        let column = find(Pane::Records).or_else(|| find(Pane::Side));
        let span = (
            left,
            right,
            column.map_or(0, |c| c.y),
            find(Pane::Side).or(column).map_or(0, Rect::bottom),
        );

        let cell = model.metrics.cell;
        let at = |col: u16, row: u16| {
            Point::new(
                model.origin.x + f32::from(col) * cell.width,
                model.origin.y + f32::from(row) * cell.height,
            )
        };
        // The gap and the border on its right, beside the chat's rows.
        // The border on its left is left alone: the panel there draws its
        // scrollbar on it.
        let upright = |divider, left: Rect, right: Rect, rows: Rect| {
            let gap = right.x.saturating_sub(left.right());
            let middle = f32::from(left.right()) + f32::from(gap + 1) / 2.0;
            let edge = match divider {
                Divider::Agents => left.right(),
                Divider::Right | Divider::Records => right.x,
            };
            Band {
                divider,
                rect: Rectangle::new(
                    at(left.right(), rows.y),
                    Size::new(
                        f32::from(gap + 1) * cell.width,
                        f32::from(rows.height) * cell.height,
                    ),
                ),
                lead: middle - f32::from(edge),
            }
        };
        let mut bands = Vec::new();
        if !model.covered {
            if let (Some(agents), Some(chat)) = (find(Pane::Agents), find(Pane::Chat)) {
                bands.push(upright(Divider::Agents, agents, chat, chat));
            }
            if let (Some(chat), Some(column)) = (find(Pane::Chat), column) {
                bands.push(upright(Divider::Right, chat, column, chat));
            }
            if let (Some(records), Some(side)) = (find(Pane::Records), find(Pane::Side)) {
                if records.x == side.x && side.y == records.bottom() {
                    bands.push(Band {
                        divider: Divider::Records,
                        rect: Rectangle::new(
                            at(side.x, side.y - 1),
                            Size::new(f32::from(side.width) * cell.width, 2.0 * cell.height),
                        ),
                        lead: 0.0,
                    });
                }
            }
        }
        let side_toggle = model
            .side_toggle
            .map(|(left, right)| side_toggle_box(model, left, right));
        Self {
            bars,
            bands,
            span,
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

/// The boxes of one bar, packed from the left; short labels when the
/// long ones don't fit, and boxes that still don't fit left out.
fn bar_layout(model: &Model, pane: &PaneChrome) -> BarLayout {
    let (metrics, cell) = (&model.metrics, model.metrics.cell);
    let bar = pane.bar;
    let rect = Rectangle::new(
        Point::new(
            model.origin.x + (f32::from(bar.x) + 0.5) * cell.width,
            model.origin.y + (f32::from(bar.y) + 0.5) * cell.height,
        ),
        Size::new(
            f32::from(bar.width.saturating_sub(1)) * cell.width,
            f32::from(bar.height) * cell.height,
        ),
    );
    let unit = unit(metrics);
    let height = (cell.height * 1.25).round().min(rect.height - 4.0);
    let margin = ((rect.height - height) / 2.0).floor();
    let top = (rect.center_y() - height / 2.0).round();
    let gap = (unit * 0.4).round().max(2.0);
    let (start, mut end) = (
        (rect.x + margin).round(),
        (rect.x + rect.width - margin).round(),
    );
    let mut square = |wanted: bool| {
        (wanted && end - start >= 4.0 * height).then(|| {
            let square = Rectangle::new(Point::new(end - height, top), Size::new(height, height));
            end = square.x - gap;
            square
        })
    };
    let close = square(pane.closable);
    let expand = pane
        .expand
        .and_then(|expanded| Some((square(true)?, expanded)));
    let width = |text: &str| ((text.width() as f32 + 2.0 * SIDE) * unit).round();
    let scope = pane.scope.and_then(|on| {
        // A one-glyph label still gets a box at least as wide as tall.
        let w = width(SideScope::BUTTON).max(height);
        (end - start >= 3.0 * w).then(|| {
            let rect = Rectangle::new(Point::new(end - w, top), Size::new(w, height));
            end = rect.x - gap;
            (rect, on)
        })
    });
    let icon_tab = pane.icon_tab.and_then(|tab| {
        (end - start >= 4.0 * height).then(|| {
            let rect = Rectangle::new(Point::new(end - height, top), Size::new(height, height));
            end = rect.x - gap;
            (rect, tab)
        })
    });

    let items = |short: bool| -> Vec<(String, Kind)> {
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
    };
    let room = |items: &[(String, Kind)]| {
        items.iter().map(|(text, _)| width(text) + gap).sum::<f32>() - gap <= end - start
    };
    let long = items(false);
    let items = if room(&long) { long } else { items(true) };

    let mut boxes = Vec::with_capacity(items.len());
    let mut x = start;
    for (text, kind) in items {
        let w = width(&text);
        if x + w > end {
            break;
        }
        boxes.push(Boxed {
            rect: Rectangle::new(Point::new(x, top), Size::new(w, height)),
            text,
            kind,
        });
        x += w + gap;
    }

    let after = if boxes.is_empty() { start } else { x + gap };
    let info = [&pane.info, &pane.info_short]
        .into_iter()
        .flatten()
        .find(|text| end - text.width() as f32 * unit >= after)
        .map(|text| Info {
            text: text.clone(),
            color: grid::color(pane.info_color, model.palette.muted),
            end: Point::new(end - (unit * SIDE).round(), rect.center_y()),
        });

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
mod tests {
    use super::*;
    use aura_ui_terminal::{ChatTab, RecordsTab, SidePanelTab};
    use ratatui::style::Color as TermColor;

    fn pane(which: Pane, area: Rect, tabs: Vec<PaneTab>) -> PaneChrome {
        PaneChrome {
            pane: which,
            area,
            bar: Rect::new(area.x, area.y, area.width, 2),
            active: tabs.first().copied(),
            tabs,
            icon_tab: None,
            title: None,
            info: None,
            info_short: None,
            info_color: TermColor::Reset,
            closable: false,
            expand: None,
            scope: None,
            focused: false,
        }
    }

    fn chat_tabs() -> Vec<PaneTab> {
        vec![
            PaneTab::Chat(ChatTab::Chat),
            PaneTab::Chat(ChatTab::Terminal),
        ]
    }

    fn model(panes: Vec<PaneChrome>) -> Model {
        Model {
            panes,
            covered: false,
            side_toggle: None,
            origin: Point::new(6.0, 6.0),
            metrics: Metrics::new(14.0),
            palette: Palette::from_theme(&aura_ui_terminal::Theme::by_name("cyber")),
        }
    }

    fn four_panes() -> Model {
        let mut records = pane(Pane::Records, Rect::new(100, 0, 40, 20), Vec::new());
        records.title = Some("KERNEL".into());
        records.info = Some("12 records".into());
        model(vec![
            pane(
                Pane::Agents,
                Rect::new(0, 0, 30, 40),
                vec![PaneTab::Side(SidePanelTab::Trace)],
            ),
            pane(Pane::Chat, Rect::new(30, 0, 70, 40), chat_tabs()),
            records,
            pane(
                Pane::Side,
                Rect::new(100, 20, 40, 20),
                vec![PaneTab::Side(SidePanelTab::Trace)],
            ),
        ])
    }

    #[test]
    fn tabs_are_packed_left_and_as_wide_as_their_labels() {
        let layout = Layout::of(&four_panes());
        let chat = &layout.bars[1];
        assert_eq!(chat.pane, Pane::Chat);
        let [first, second] = [&chat.boxes[0], &chat.boxes[1]];
        assert!(first.rect.x > chat.rect.x && first.rect.x < chat.rect.x + 12.0);
        assert!(second.rect.x > first.rect.x + first.rect.width);
        assert!(second.rect.x < first.rect.x + first.rect.width + 6.0);
        let widths = [first.rect.width, second.rect.width];
        assert!(widths[1] > widths[0], "TERMINAL is longer than CHAT");
        assert!(first.rect.y > chat.rect.y);
        assert!(first.rect.y + first.rect.height < chat.rect.y + chat.rect.height);
    }

    #[test]
    fn untabbed_panels_show_a_title_box_and_their_info() {
        let layout = Layout::of(&four_panes());
        let records = &layout.bars[2];
        assert_eq!(records.boxes.len(), 1);
        assert_eq!(records.boxes[0].text, "KERNEL");
        assert_eq!(records.boxes[0].kind, Kind::Title);
        let info = records.info.as_ref().expect("room for the info");
        assert_eq!(info.text, "12 records");
        assert!(info.end.x <= records.rect.x + records.rect.width);
    }

    #[test]
    fn the_kernel_bar_has_its_tabs_and_keeps_its_count() {
        let tabs = vec![
            PaneTab::Records(RecordsTab::Kernel),
            PaneTab::Records(RecordsTab::Members),
            PaneTab::Records(RecordsTab::Policies),
            PaneTab::Records(RecordsTab::Inbox),
        ];
        let mut records = pane(Pane::Records, Rect::new(100, 0, 50, 20), tabs);
        records.active = Some(PaneTab::Records(RecordsTab::Members));
        records.info = Some("3 AGENTS".into());
        let layout = Layout::of(&model(vec![records]));
        let bar = &layout.bars[0];
        let kinds: Vec<Kind> = bar.boxes.iter().map(|boxed| boxed.kind.clone()).collect();
        assert_eq!(
            kinds,
            [
                Kind::Tab(PaneTab::Records(RecordsTab::Kernel)),
                Kind::Tab(PaneTab::Records(RecordsTab::Members)),
                Kind::Tab(PaneTab::Records(RecordsTab::Policies)),
                Kind::Tab(PaneTab::Records(RecordsTab::Inbox)),
            ]
        );
        assert_eq!(bar.boxes[1].text, "MEMBERS");
        assert_eq!(bar.boxes[2].text, "POLICIES");
        assert_eq!(bar.boxes[3].text, "INBOX");
        let info = bar.info.as_ref().expect("room for the count");
        assert_eq!(info.text, "3 AGENTS");
        assert!(info.end.x > bar.boxes[3].rect.x + bar.boxes[3].rect.width);
    }

    #[test]
    fn narrow_bars_fall_back_to_short_labels_and_drop_what_still_does_not_fit() {
        let mut narrow = pane(Pane::Chat, Rect::new(0, 0, 14, 20), chat_tabs());
        narrow.info = Some("a long info text".into());
        narrow.info_short = Some("info".into());
        let layout = Layout::of(&model(vec![narrow]));
        let bar = &layout.bars[0];
        let labels: Vec<&str> = bar.boxes.iter().map(|b| b.text.as_str()).collect();
        assert_eq!(
            labels,
            [ChatTab::Chat.short_label(), ChatTab::Terminal.short_label()]
        );
        assert!(bar.info.is_none(), "{:?}", bar.info);
    }

    #[test]
    fn the_pointer_hits_tabs_bars_and_dividers() {
        let mut chrome = Chrome::default();
        chrome.update(four_panes(), None);
        let terminal = PaneTab::Chat(ChatTab::Terminal);
        let at = chrome.spot(terminal).expect("drawn");
        assert_eq!(chrome.layout.hit(at), Some(Hit::Tab(terminal)));
        let bar = &chrome.layout.bars[1].rect;
        let empty = Point::new(bar.x + bar.width - 20.0, bar.center_y());
        assert_eq!(chrome.layout.hit(empty), Some(Hit::Bar(Pane::Chat)));
        for divider in [Divider::Agents, Divider::Right, Divider::Records] {
            let at = chrome.band(divider).expect("between two panels");
            assert_eq!(chrome.layout.hit(at), Some(Hit::Divider(divider)));
        }
        // The borders the agents and chat panels draw their scrollbars on
        // are theirs, not the dividers'.
        let cell = chrome.model.as_ref().expect("shown").metrics.cell;
        let on_cell =
            |col: f32, row: f32| Point::new(6.0 + col * cell.width, 6.0 + row * cell.height);
        assert_eq!(
            chrome.layout.hit(on_cell(99.5, 10.5)),
            None,
            "chat's border"
        );
        assert_eq!(
            chrome.layout.hit(on_cell(29.5, 10.5)),
            None,
            "agents' border"
        );
        let right = chrome.layout.hit(on_cell(100.5, 10.5));
        assert_eq!(right, Some(Hit::Divider(Divider::Right)));
        chrome.point(Some(at));
        assert_eq!(chrome.hover(), Some(Hit::Tab(terminal)));
    }

    #[test]
    fn closable_bars_end_in_a_close_box_left_of_the_info() {
        let mut model = four_panes();
        let chat = &mut model.panes[1];
        chat.tabs.clear();
        chat.title = Some("RECORD #60".into());
        chat.info = Some("POLICY".into());
        chat.closable = true;
        let layout = Layout::of(&model);
        let bar = &layout.bars[1];
        let close = bar.close.expect("room for the close box");
        assert!(close.x + close.width <= bar.rect.x + bar.rect.width);
        let info = bar.info.as_ref().expect("room for the info");
        assert!(info.end.x < close.x);
        let middle = Point::new(close.center_x(), close.center_y());
        assert_eq!(layout.hit(middle), Some(Hit::Close(Pane::Chat)));
        assert!(Layout::of(&four_panes()).bars[1].close.is_none());
    }

    #[test]
    fn expandable_bars_end_in_an_expand_box_left_of_any_close_box() {
        let mut model = four_panes();
        model.panes[1].expand = Some(false);
        let layout = Layout::of(&model);
        let bar = &layout.bars[1];
        let (expand, expanded) = bar.expand.expect("room for the expand box");
        assert!(!expanded);
        assert!(expand.x + expand.width <= bar.rect.x + bar.rect.width);
        let last = bar.boxes.last().expect("the tabs are drawn");
        assert!(last.rect.x + last.rect.width < expand.x);
        let middle = Point::new(expand.center_x(), expand.center_y());
        assert_eq!(layout.hit(middle), Some(Hit::Expand(Pane::Chat)));
        assert!(Layout::of(&four_panes()).bars[1].expand.is_none());

        let chat = &mut model.panes[1];
        chat.tabs.clear();
        chat.title = Some("RECORD #60".into());
        chat.closable = true;
        chat.expand = Some(true);
        let layout = Layout::of(&model);
        let bar = &layout.bars[1];
        let close = bar.close.expect("room for the close box");
        let (expand, expanded) = bar.expand.expect("room for the expand box");
        assert!(expanded);
        assert!(expand.x + expand.width < close.x);
        let middle = Point::new(expand.center_x(), expand.center_y());
        assert_eq!(layout.hit(middle), Some(Hit::Expand(Pane::Chat)));
    }

    #[test]
    fn scoped_bars_end_in_a_star_box_after_their_tabs() {
        let mut model = four_panes();
        model.panes[3].scope = Some(true);
        let layout = Layout::of(&model);
        let bar = &layout.bars[3];
        assert_eq!(bar.pane, Pane::Side);
        let (scope, on) = bar.scope.expect("room for the * box");
        assert!(on);
        assert!(scope.width >= scope.height, "at least square: {scope:?}");
        assert!(scope.x + scope.width <= bar.rect.x + bar.rect.width);
        let last = bar.boxes.last().expect("the tabs are drawn");
        assert!(last.rect.x + last.rect.width < scope.x);
        let middle = Point::new(scope.center_x(), scope.center_y());
        assert_eq!(layout.hit(middle), Some(Hit::Scope(Pane::Side)));
        assert!(Layout::of(&four_panes()).bars[3].scope.is_none());
    }

    #[test]
    fn the_side_bar_ends_in_a_star_box_then_an_expand_box() {
        let mut model = four_panes();
        model.panes[3].scope = Some(false);
        model.panes[3].expand = Some(false);
        let layout = Layout::of(&model);
        let bar = &layout.bars[3];
        let (scope, on) = bar.scope.expect("room for the * box");
        let (expand, expanded) = bar.expand.expect("room for the expand box");
        assert!(!on && !expanded);
        assert!(scope.x + scope.width < expand.x, "{scope:?} {expand:?}");
        assert!(expand.x + expand.width <= bar.rect.x + bar.rect.width);
        let last = bar.boxes.last().expect("the tabs are drawn");
        assert!(last.rect.x + last.rect.width < scope.x);
        let hit = |rect: Rectangle| layout.hit(Point::new(rect.center_x(), rect.center_y()));
        assert_eq!(hit(scope), Some(Hit::Scope(Pane::Side)));
        assert_eq!(hit(expand), Some(Hit::Expand(Pane::Side)));
    }

    #[test]
    fn hosts_icon_is_a_square_tab_box_left_of_the_star_box() {
        let host = PaneTab::Side(SidePanelTab::Host);
        let mut model = four_panes();
        model.panes[3].scope = Some(false);
        model.panes[3].expand = Some(false);
        model.panes[3].icon_tab = Some(host);
        let layout = Layout::of(&model);
        let bar = &layout.bars[3];
        let (icon, tab) = bar.icon_tab.expect("room for the icon box");
        assert_eq!(tab, host);
        assert!(
            (icon.width - icon.height).abs() < f32::EPSILON,
            "square: {icon:?}"
        );
        let (scope, _) = bar.scope.expect("room for the * box");
        assert!(icon.x + icon.width < scope.x, "{icon:?} {scope:?}");
        let last = bar.boxes.last().expect("the tabs are drawn");
        assert!(last.rect.x + last.rect.width < icon.x);
        assert!(
            bar.boxes.iter().all(|boxed| boxed.kind != Kind::Tab(host)),
            "it has no label's box"
        );
        let middle = Point::new(icon.center_x(), icon.center_y());
        assert_eq!(layout.hit(middle), Some(Hit::Tab(host)));

        let mut chrome = Chrome::default();
        chrome.update(model, None);
        assert_eq!(chrome.spot(host), Some(middle));
    }

    #[test]
    fn dragging_a_divider_moves_its_share() {
        let mut chrome = Chrome::default();
        let model = four_panes();
        let cell = model.metrics.cell;
        let (x, y) = (model.origin.x, model.origin.y);
        chrome.update(model, None);
        let splits = PaneSplits::default();

        let to = |col: f32, row: f32| Point::new(x + col * cell.width, y + row * cell.height);
        // The bands' middles are on the borders beside the chat, half a
        // cell past the edges they move.
        let moved = chrome.drag(Divider::Agents, to(14.5, 5.0), splits);
        assert!((moved.agents.expect("set") - 0.1).abs() < 1e-4, "{moved:?}");
        assert_eq!(moved.right, None);

        let moved = chrome.drag(Divider::Right, to(126.5, 5.0), splits);
        assert!((moved.right.expect("set") - 0.1).abs() < 1e-4, "{moved:?}");

        let moved = chrome.drag(Divider::Records, to(110.0, 10.0), splits);
        assert!(
            (moved.records.expect("set") - 0.25).abs() < 1e-4,
            "{moved:?}"
        );

        let past = chrome.drag(Divider::Agents, to(-50.0, 5.0), splits);
        assert_eq!(past.agents, Some(0.0));
    }

    #[test]
    fn covered_bars_are_left_to_what_covers_them() {
        let Model { panes, palette, .. } = four_panes();
        let at = (Point::default(), Metrics::new(14.0), palette);
        let mut buffer = Buffer::empty(Rect::new(0, 0, 140, 40));
        let model = Model::new(&panes, None, &buffer, at);
        assert_eq!(model.panes.len(), 4);
        assert!(!model.covered);
        assert_eq!(Layout::of(&model).bands.len(), 3);

        buffer.set_string(50, 1, "Approve?", ratatui::style::Style::default());
        let model = Model::new(&panes, None, &buffer, at);
        let left: Vec<Pane> = model.panes.iter().map(|p| p.pane).collect();
        assert_eq!(left, [Pane::Agents, Pane::Records, Pane::Side]);
        assert!(model.covered);
        assert!(Layout::of(&model).bands.is_empty());
    }
}
