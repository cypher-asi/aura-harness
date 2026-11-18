//! The strip above the grid, which is also the window's title bar: a small
//! box per tab holding its number, the one in front filled, a `+` box
//! after the last, a wide box with the name of what the tab in front
//! shows, and the minimize, maximize and close boxes at the right end.
//!
//! One [`Layout`] places the boxes for both the painter ([`quads`]) and
//! the pointer, so a click lands on what is drawn.

// Label lengths become pixel widths.
#![allow(clippy::cast_precision_loss)]

pub(crate) mod quads;

use crate::geom::{Point, Rectangle, Size};
use crate::grid::{Metrics, Palette};
use crate::theme;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Label characters of padding on each side of a label.
const SIDE: f32 = 1.0;
/// Narrowest title box worth drawing, in label characters.
const MIN_TITLE: f32 = 8.0;

/// What the pointer is over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    Tab(usize),
    /// The `+` box.
    New,
    Minimize,
    Maximize,
    Close,
}

/// The window boxes, left to right.
const CONTROLS: [Hit; 3] = [Hit::Minimize, Hit::Maximize, Hit::Close];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// The tab's number.
    pub label: String,
    /// Whether an agent of the tab is working.
    pub busy: bool,
}

/// Everything the bar's look depends on, besides the pointer.
#[derive(Debug, Clone, PartialEq)]
pub struct Model {
    pub items: Vec<Item>,
    pub front: usize,
    /// What the tab in front shows, for the wide box.
    pub title: String,
    /// Left edge and width of the grid below, which the bar lines up with.
    pub x: f32,
    pub width: f32,
    pub metrics: Metrics,
    pub palette: Palette,
}

/// Height of the bar at this font size, in whole pixels.
pub fn height(metrics: &Metrics) -> f32 {
    // The same space above the boxes as around the grid.
    metrics.pad + box_height(metrics)
}

fn box_height(metrics: &Metrics) -> f32 {
    (metrics.cell.height * 1.25).round()
}

pub(crate) fn label_size(metrics: &Metrics) -> f32 {
    (metrics.font_size * 0.85).round()
}

/// Advance of one label character.
pub(crate) fn unit(metrics: &Metrics) -> f32 {
    theme::cell_size(label_size(metrics)).width
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Layout {
    tabs: Vec<Rectangle>,
    plus: Rectangle,
    /// The wide box, when there is room for it.
    title: Option<Title>,
    /// The window boxes, in the order of [`CONTROLS`].
    controls: [Rectangle; 3],
}

#[derive(Debug, Clone, PartialEq)]
struct Title {
    rect: Rectangle,
    /// What the tab in front shows, cut to fit
    text: String,
    /// Middle of the text: the middle of the bar, unless the text would
    /// then cross the box's left edge
    center: f32,
}

impl Layout {
    /// Boxes for `model`, left to right: the tabs, `+`, the title box
    /// taking what is left of the width, and the window boxes.
    pub fn of(model: &Model) -> Self {
        let unit = unit(&model.metrics);
        let height = box_height(&model.metrics);
        let gap = (unit * 0.4).round().max(2.0);
        let boxed = |x: f32, width: f32| {
            let (left, right) = (x.round(), (x + width).round());
            let top = model.metrics.pad;
            Rectangle::new(Point::new(left, top), Size::new(right - left, height))
        };

        let mut x = model.x;
        let mut tabs = Vec::with_capacity(model.items.len());
        for item in &model.items {
            let width = (item.label.width() as f32 + 2.0 * SIDE) * unit;
            tabs.push(boxed(x, width.max(height)));
            x = tabs.last().map_or(x, |tab| tab.x + tab.width) + gap;
        }
        let plus = boxed(x, height);
        x = plus.x + plus.width + gap;

        let mut controls = [Rectangle::default(); 3];
        let mut right = model.x + model.width;
        for control in controls.iter_mut().rev() {
            *control = boxed(right - height, height);
            right = control.x - gap;
        }

        let room = right - x;
        let title = (room >= MIN_TITLE * unit).then(|| {
            let rect = boxed(x, room);
            let text = fit(&model.title, cells_in(room - 2.0 * SIDE * unit, unit));
            let half = text.width() as f32 * unit / 2.0;
            let lowest = rect.x + SIDE * unit + half;
            let highest = (rect.x + rect.width - SIDE * unit - half).max(lowest);
            let center = (model.x + model.width / 2.0).clamp(lowest, highest).round();
            Title { rect, text, center }
        });
        Self {
            tabs,
            plus,
            title,
            controls,
        }
    }

    /// The window boxes win over tabs that run under them.
    pub fn hit(&self, at: Point) -> Option<Hit> {
        let control = self.controls.iter().position(|rect| rect.contains(at));
        let tab = || self.tabs.iter().position(|tab| tab.contains(at));
        control
            .map(|index| CONTROLS[index])
            .or_else(|| tab().map(Hit::Tab))
            .or_else(|| self.plus.contains(at).then_some(Hit::New))
    }

    /// The tab whose place a tab dragged to `x` takes: the box `x` is
    /// over, or the one at that end of the row past either end. A gap
    /// counts with the box before it.
    pub fn slot(&self, x: f32) -> Option<usize> {
        if self.tabs.is_empty() {
            return None;
        }
        let after = self.tabs.iter().take_while(|tab| tab.x <= x).count();
        Some(after.saturating_sub(1))
    }

    #[cfg(test)]
    fn rect(&self, hit: Hit) -> Option<Rectangle> {
        match hit {
            Hit::Tab(index) => self.tabs.get(index).copied(),
            Hit::New => Some(self.plus),
            Hit::Minimize => Some(self.controls[0]),
            Hit::Maximize => Some(self.controls[1]),
            Hit::Close => Some(self.controls[2]),
        }
    }
}

/// Whole label characters that fit in `space` pixels.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn cells_in(space: f32, unit: f32) -> usize {
    (space / unit + 0.01).floor().max(0.0) as usize
}

/// `label` cut to `cells` columns, ending in `..` when cut.
fn fit(label: &str, cells: usize) -> String {
    if label.width() <= cells {
        return label.to_string();
    }
    let keep = cells.saturating_sub(2);
    let mut out = String::new();
    let mut used = 0;
    for c in label.chars() {
        let width = c.width().unwrap_or(0);
        if used + width > keep {
            break;
        }
        used += width;
        out.push(c);
    }
    if cells >= 3 {
        out.push_str("..");
    }
    out
}

/// The bar as it is shown, and what the pointer is over.
#[derive(Default)]
pub struct Bar {
    model: Option<Model>,
    layout: Layout,
    hover: Option<Hit>,
}

impl Bar {
    /// Show `model`, with the pointer at `pointer` when it is over the bar.
    pub fn update(&mut self, model: Model, pointer: Option<Point>) {
        if self.model.as_ref() != Some(&model) {
            self.layout = Layout::of(&model);
            self.model = Some(model);
        }
        self.point(pointer);
    }

    /// The pointer is at `pointer`, when it is over the bar.
    pub fn point(&mut self, pointer: Option<Point>) {
        self.hover = pointer.and_then(|at| self.layout.hit(at));
    }

    /// What the pointer was over at the last [`Self::update`].
    pub const fn hover(&self) -> Option<Hit> {
        self.hover
    }

    /// See [`Layout::slot`].
    pub fn slot(&self, x: f32) -> Option<usize> {
        self.layout.slot(x)
    }

    /// The middle of the box `hit` names, if it is drawn.
    #[cfg(test)]
    pub fn spot(&self, hit: Hit) -> Option<Point> {
        let rect = self.layout.rect(hit)?;
        Some(Point::new(rect.center_x(), rect.center_y()))
    }

    /// The text in the wide box, as drawn.
    #[cfg(test)]
    pub fn title(&self) -> Option<&str> {
        self.layout.title.as_ref().map(|title| title.text.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(tabs: usize, title: &str, width: f32) -> Model {
        Model {
            items: (1..=tabs)
                .map(|number| Item {
                    label: number.to_string(),
                    busy: false,
                })
                .collect(),
            front: 0,
            title: title.to_string(),
            x: 9.0,
            width,
            metrics: Metrics::new(14.0),
            palette: Palette::from_theme(&aura_ui_terminal::Theme::by_name("cyber")),
        }
    }

    fn middle(rect: Rectangle) -> Point {
        Point::new(rect.center_x(), rect.center_y())
    }

    #[test]
    fn boxes_follow_each_other_and_the_title_takes_the_rest() {
        let model = model(3, "AURA-HARNESS", 1300.0);
        let layout = Layout::of(&model);
        assert_eq!(layout.tabs.len(), 3);
        assert!((layout.tabs[0].x - 9.0).abs() < f32::EPSILON);
        for pair in layout.tabs.windows(2) {
            assert!(pair[1].x > pair[0].x + pair[0].width);
            assert!((pair[1].width - pair[0].width).abs() < f32::EPSILON);
        }
        let last = layout.tabs[2];
        assert!(layout.plus.x > last.x + last.width);
        let title = layout.title.clone().expect("room for a title");
        assert!(title.rect.x > layout.plus.x + layout.plus.width);
        assert!(title.rect.x + title.rect.width < layout.controls[0].x);
        for pair in layout.controls.windows(2) {
            assert!(pair[1].x > pair[0].x + pair[0].width);
        }
        let close = layout.controls[2];
        assert!((close.x + close.width - (model.x + model.width)).abs() <= 1.0);
        assert_eq!(title.text, "AURA-HARNESS");
        let bottom = layout.plus.y + layout.plus.height;
        assert!((bottom - height(&model.metrics)).abs() < f32::EPSILON);
    }

    #[test]
    fn the_pointer_hits_what_is_drawn() {
        let layout = Layout::of(&model(2, "AURA-HARNESS", 1300.0));
        assert_eq!(layout.hit(middle(layout.tabs[0])), Some(Hit::Tab(0)));
        assert_eq!(layout.hit(middle(layout.tabs[1])), Some(Hit::Tab(1)));
        assert_eq!(layout.hit(middle(layout.plus)), Some(Hit::New));
        let between = Point::new(
            layout.tabs[0].x + layout.tabs[0].width + 1.0,
            layout.plus.center_y(),
        );
        assert_eq!(layout.hit(between), None);
        assert_eq!(layout.hit(Point::new(10.0, 2.0)), None, "above the boxes");
        let title = layout.title.clone().expect("room for a title").rect;
        assert_eq!(layout.hit(middle(title)), None, "the title is no button");
        assert_eq!(layout.hit(middle(layout.controls[0])), Some(Hit::Minimize));
        assert_eq!(layout.hit(middle(layout.controls[1])), Some(Hit::Maximize));
        assert_eq!(layout.hit(middle(layout.controls[2])), Some(Hit::Close));
    }

    #[test]
    fn a_dragged_tab_takes_the_place_of_the_box_under_it() {
        let layout = Layout::of(&model(3, "AURA-HARNESS", 1300.0));
        for (index, tab) in layout.tabs.iter().enumerate() {
            assert_eq!(layout.slot(tab.center_x()), Some(index));
        }
        assert_eq!(layout.slot(0.0), Some(0), "left of the row");
        assert_eq!(layout.slot(1200.0), Some(2), "right of the row");
        let gap = layout.tabs[0].x + layout.tabs[0].width + 0.5;
        assert_eq!(layout.slot(gap), Some(0));
        assert_eq!(Layout::default().slot(10.0), None);
    }

    #[test]
    fn the_window_boxes_stay_reachable_when_tabs_crowd_the_bar() {
        let layout = Layout::of(&model(20, "AURA-HARNESS", 300.0));
        assert_eq!(layout.title, None);
        assert_eq!(layout.hit(middle(layout.controls[2])), Some(Hit::Close));
    }

    #[test]
    fn the_title_is_cut_to_its_box_and_left_out_when_there_is_no_room() {
        let layout = Layout::of(&model(2, "A RATHER LONG PROJECT NAME / AGENT", 300.0));
        let text = layout.title.expect("room for a short title").text;
        assert!(text.ends_with(".."), "{text:?}");
        assert!(text.starts_with("A RATHER"), "{text:?}");

        let layout = Layout::of(&model(9, "AURA-HARNESS", 260.0));
        assert_eq!(layout.title, None);
        assert_eq!(layout.tabs.len(), 9);
    }

    #[test]
    fn the_title_sits_in_the_middle_of_the_bar_when_it_fits() {
        let wide = model(3, "ZODE-WEBSITE / AGENT", 1300.0);
        let title = Layout::of(&wide).title.expect("room for a title");
        let middle = wide.x + wide.width / 2.0;
        assert!((title.center - middle).abs() <= 0.5, "{title:?}");

        let narrow = model(3, "A RATHER LONG PROJECT NAME / AGENT", 300.0);
        let title = Layout::of(&narrow).title.expect("room for a title");
        let half = title.text.width() as f32 * unit(&narrow.metrics) / 2.0;
        assert!(
            title.center - half >= title.rect.x,
            "kept inside its box: {title:?}"
        );
    }

    #[test]
    fn wider_numbers_get_wider_boxes() {
        let layout = Layout::of(&model(100, "", 4000.0));
        assert!(layout.tabs[99].width > layout.tabs[0].width);
    }

    #[test]
    fn labels_are_cut_by_columns() {
        assert_eq!(fit("AGENT", 5), "AGENT");
        assert_eq!(fit("AGENTS", 5), "AGE..");
        assert_eq!(fit("日本語の名前", 6), "日本..");
        assert_eq!(fit("AGENT", 2), "");
    }

    #[test]
    fn the_bar_notices_what_the_pointer_is_over() {
        let mut bar = Bar::default();
        let model = model(2, "AURA-HARNESS", 1300.0);
        bar.update(model.clone(), None);
        assert_eq!(bar.hover(), None);
        assert_eq!(bar.title(), Some("AURA-HARNESS"));
        let at = bar.spot(Hit::Tab(1)).expect("drawn");
        bar.update(model, Some(at));
        assert_eq!(bar.hover(), Some(Hit::Tab(1)));
    }
}
