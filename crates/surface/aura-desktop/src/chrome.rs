//! The panels' title bars, drawn over the framed, empty rows the terminal
//! UI keeps for them (`App::set_host_chrome`): boxes packed to the left,
//! each as wide as its label and always outlined, then the panel's info
//! at the right end. A panel without tabs shows its title in a filled
//! box; a panel with tabs fills the box of the one it shows. Fills take
//! the selection color while their panel has focus; otherwise the box is
//! only outlined. A bar over a view that closes, such as a file shown
//! over the chat, ends in a close box; a panel that expands to fill the
//! window has an expand box left of it.
//!
//! The dividers between panels live here too: bands along the borders
//! two panels share, which drag to move the border.

// Cell coordinates become pixel positions.
#![allow(clippy::cast_precision_loss)]

mod quads;

use crate::geom::{Color, Point, Rectangle, Size};
use crate::grid::{self, Metrics, Palette};
use crate::tab_bar::unit;
use aura_ui_terminal::{Pane, PaneChrome, PaneSplits, PaneTab};
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
    /// The rest of a title bar
    Bar(Pane),
    Divider(Divider),
}

/// What the bars show, and where the grid sits.
#[derive(Debug, Clone, PartialEq)]
pub struct Model {
    pub panes: Vec<PaneChrome>,
    /// Whether an overlay covers any bar, which then leaves the dividers
    /// alone too
    pub covered: bool,
    /// Top-left corner of the grid
    pub origin: Point,
    pub metrics: Metrics,
    pub palette: Palette,
}

impl Model {
    /// The bars in `panes` that `buffer` still shows empty: an overlay
    /// drawn over a bar keeps it.
    pub fn new(panes: &[PaneChrome], buffer: &Buffer, at: (Point, Metrics, Palette)) -> Self {
        let (origin, metrics, palette) = at;
        let shown: Vec<PaneChrome> = panes
            .iter()
            .filter(|pane| is_empty(pane.bar, buffer))
            .cloned()
            .collect();
        Self {
            covered: shown.len() < panes.len(),
            panes: shown,
            origin,
            metrics,
            palette,
        }
    }
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
}

#[derive(Debug, Clone, PartialEq)]
struct Band {
    divider: Divider,
    rect: Rectangle,
    /// Blank cells between the borders on either side
    gap: u16,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Layout {
    bars: Vec<BarLayout>,
    bands: Vec<Band>,
    /// Where the panels are, in cells: left, right, and the right
    /// column's top and bottom
    span: (u16, u16, u16, u16),
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
        // From the border on one side of the gap to the border on the
        // other, beside the chat's rows.
        let upright = |divider, left: Rect, right: Rect, rows: Rect| {
            let gap = right.x.saturating_sub(left.right());
            Band {
                divider,
                rect: Rectangle::new(
                    at(left.right().saturating_sub(1), rows.y),
                    Size::new(
                        f32::from(gap + 2) * cell.width,
                        f32::from(rows.height) * cell.height,
                    ),
                ),
                gap,
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
                        gap: 0,
                    });
                }
            }
        }
        Self { bars, bands, span }
    }

    /// Dividers win over the bars they cross.
    pub fn hit(&self, at: Point) -> Option<Hit> {
        if let Some(band) = self.bands.iter().find(|band| band.rect.contains(at)) {
            return Some(Hit::Divider(band.divider));
        }
        let bar = self.bars.iter().find(|bar| bar.rect.contains(at))?;
        if bar.close.is_some_and(|close| close.contains(at)) {
            return Some(Hit::Close(bar.pane));
        }
        if bar.expand.is_some_and(|(expand, _)| expand.contains(at)) {
            return Some(Hit::Expand(bar.pane));
        }
        let tab = bar.boxes.iter().find_map(|boxed| match boxed.kind {
            Kind::Tab(tab) if boxed.rect.contains(at) => Some(tab),
            _ => None,
        });
        Some(tab.map_or(Hit::Bar(bar.pane), Hit::Tab))
    }
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
        // The middle of the gap stands for the edge of the dragged panel.
        let half_gap = self
            .layout
            .bands
            .iter()
            .find(|band| band.divider == divider)
            .map_or(0.0, |band| f32::from(band.gap) / 2.0);
        match divider {
            Divider::Agents => splits.agents = share(col - half_gap - f32::from(left), wide),
            Divider::Right => splits.right = share(f32::from(right) - col - half_gap, wide),
            Divider::Records => {
                splits.records = share(row - f32::from(top), bottom.saturating_sub(top));
            }
        }
        splits
    }

    /// The middle of the box of `tab`, if it is drawn.
    #[cfg(test)]
    pub fn spot(&self, tab: PaneTab) -> Option<Point> {
        self.layout
            .bars
            .iter()
            .flat_map(|bar| &bar.boxes)
            .find_map(|boxed| {
                (boxed.kind == Kind::Tab(tab))
                    .then(|| Point::new(boxed.rect.center_x(), boxed.rect.center_y()))
            })
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
    use aura_ui_terminal::{ChatTab, SidePanelTab};
    use ratatui::style::Color as TermColor;

    fn pane(which: Pane, area: Rect, tabs: Vec<PaneTab>) -> PaneChrome {
        PaneChrome {
            pane: which,
            area,
            bar: Rect::new(area.x, area.y, area.width, 2),
            active: tabs.first().copied(),
            tabs,
            title: None,
            info: None,
            info_short: None,
            info_color: TermColor::Reset,
            closable: false,
            expand: None,
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
    fn dragging_a_divider_moves_its_share() {
        let mut chrome = Chrome::default();
        let model = four_panes();
        let cell = model.metrics.cell;
        let (x, y) = (model.origin.x, model.origin.y);
        chrome.update(model, None);
        let splits = PaneSplits::default();

        let to = |col: f32, row: f32| Point::new(x + col * cell.width, y + row * cell.height);
        let moved = chrome.drag(Divider::Agents, to(14.0, 5.0), splits);
        assert!((moved.agents.expect("set") - 0.1).abs() < 1e-4, "{moved:?}");
        assert_eq!(moved.right, None);

        let moved = chrome.drag(Divider::Right, to(126.0, 5.0), splits);
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
        let model = Model::new(&panes, &buffer, at);
        assert_eq!(model.panes.len(), 4);
        assert!(!model.covered);
        assert_eq!(Layout::of(&model).bands.len(), 3);

        buffer.set_string(50, 1, "Approve?", ratatui::style::Style::default());
        let model = Model::new(&panes, &buffer, at);
        let left: Vec<Pane> = model.panes.iter().map(|p| p.pane).collect();
        assert_eq!(left, [Pane::Agents, Pane::Records, Pane::Side]);
        assert!(model.covered);
        assert!(Layout::of(&model).bands.is_empty());
    }
}
