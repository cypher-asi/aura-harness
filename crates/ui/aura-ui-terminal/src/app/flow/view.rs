//! Where the GRAPH tab's view is on the canvas: following the selection,
//! panning, zooming, the map and the cells a front end draws in.

use super::{cells, FlowState, FlowView, MinimapView, Zoom, MAX_ZOOM, MIN_ZOOM, ROOM};
use ratatui::buffer::{Buffer, Cell};
use ratatui::layout::Rect;
use ratatui::style::Color;

impl FlowState {
    /// Remember where the canvas was drawn, and whether the front end
    /// draws it.
    pub fn set_view(&mut self, view: Option<FlowView>, hosted: bool) {
        self.view = view;
        self.hosted = hosted && view.is_some();
        self.clip.clear();
    }

    /// Once the frame is drawn: which cells of a hosted canvas are still
    /// marked skipped, nothing having been drawn over them. The marks
    /// come off, as ratatui sends no skipped cell to the backend and the
    /// front end's grid would keep what was there before.
    pub(crate) fn take_canvas_marks(&mut self, buffer: &mut Buffer) {
        self.clip.clear();
        let Some(area) = self.hosted_area() else {
            return;
        };
        // Drawn over without being cleared first, a cell keeps its mark.
        let marked = |cell: &mut Cell| {
            let blank = cell.skip
                && cell.symbol() == " "
                && cell.fg == Color::Reset
                && cell.bg == Color::Reset
                && cell.modifier.is_empty();
            cell.set_skip(false);
            blank
        };
        let mut open: Vec<Rect> = Vec::new();
        for y in area.top()..area.bottom() {
            let mut runs: Vec<(u16, u16)> = Vec::new();
            for x in area.left()..area.right() {
                if !marked(&mut buffer[(x, y)]) {
                    continue;
                }
                match runs.last_mut() {
                    Some((start, width)) if *start + *width == x => *width += 1,
                    _ => runs.push((x, 1)),
                }
            }
            // A run under the same run of the row above makes it taller.
            let (kept, closed): (Vec<Rect>, Vec<Rect>) = open
                .into_iter()
                .partition(|rect| rect.bottom() == y && runs.contains(&(rect.x, rect.width)));
            self.clip.extend(closed);
            open = runs
                .into_iter()
                .map(|(x, width)| {
                    kept.iter()
                        .find(|rect| rect.x == x && rect.width == width)
                        .map_or(Rect::new(x, y, width, 1), |rect| Rect {
                            height: rect.height + 1,
                            ..*rect
                        })
                })
                .collect();
        }
        self.clip.extend(open);
        self.clip.sort_by_key(|rect| (rect.y, rect.x));
    }

    /// The screen cells the front end is to draw the canvas in, when it
    /// draws it.
    #[must_use]
    pub fn hosted_area(&self) -> Option<Rect> {
        self.view.filter(|_| self.hosted).map(|view| view.area)
    }

    /// Keep the selection inside a view `width` × `height` screen cells,
    /// and that view on the canvas, unless the view was panned off it:
    /// then it stays wherever it was taken. Returns the canvas cell at the
    /// view's top left.
    pub fn sync_view(&mut self, width: f32, height: f32) -> (f32, f32) {
        let (width, height) = (width / self.zoom(), height / self.zoom());
        if self.detached {
            return self.pan;
        }
        if let Some((card, row)) = self.selected_place() {
            let rect = self.layout.card_rect(card);
            let (left, right) = (cells(rect.x) - ROOM, cells(rect.right()) + ROOM);
            if right - left > width || left < self.pan.0 {
                self.pan.0 = left;
            } else if right > self.pan.0 + width {
                self.pan.0 = right - width;
            }
            // The agent's name is on the row above the card.
            let (top, bottom) = (cells(rect.y) - 1.0, cells(rect.bottom()));
            if bottom - top <= height {
                if top < self.pan.1 {
                    self.pan.1 = top;
                } else if bottom > self.pan.1 + height {
                    self.pan.1 = bottom - height;
                }
            } else {
                let line = cells(rect.y + self.layout.cards[card].line_of(row));
                if line < self.pan.1 || line + 1.0 > self.pan.1 + height {
                    self.pan.1 = (line + 1.0 - height / 2.0).max(top);
                }
            }
        }
        self.clamp_pan(width, height);
        self.pan
    }

    /// Keep a view `width` × `height` canvas cells on the canvas.
    pub(super) fn clamp_pan(&mut self, width: f32, height: f32) {
        let most_x = (cells(self.layout.width) - width).max(0.0);
        let most_y = (cells(self.layout.height) - height).max(0.0);
        self.pan.0 = self.pan.0.clamp(0.0, most_x);
        self.pan.1 = self.pan.1.clamp(0.0, most_y);
    }

    /// Move the view so that canvas point `(x, y)` is in its middle, as
    /// far as the canvas goes; it leaves the selection where it is.
    pub(super) fn center_on(&mut self, (x, y): (f32, f32)) {
        let Some(area) = self.view.map(|view| view.area) else {
            return;
        };
        let width = f32::from(area.width) / self.zoom();
        let height = f32::from(area.height) / self.zoom();
        self.pan = (x - width / 2.0, y - height / 2.0);
        self.clamp_pan(width, height);
        self.detached = true;
        self.move_view();
    }

    /// Whether the map of the whole canvas is to be drawn: unless `m` hid
    /// it, when the canvas is twice as wide or as tall as a view `width`
    /// × `height` screen cells, or more.
    #[must_use]
    pub fn shows_minimap(&self, width: u16, height: u16) -> bool {
        let zoom = self.zoom();
        !self.minimap_hidden
            && (cells(self.layout.width) >= 2.0 * f32::from(width) / zoom
                || cells(self.layout.height) >= 2.0 * f32::from(height) / zoom)
    }

    /// Where the map of the whole canvas was drawn in the last frame.
    #[must_use]
    pub const fn minimap(&self) -> Option<MinimapView> {
        self.minimap
    }

    /// Where the map was drawn this frame, if it was.
    pub(crate) fn set_minimap(&mut self, map: Option<MinimapView>) {
        self.minimap = map;
        if map.is_none() {
            self.minimap_drag = false;
        }
    }

    /// Move the view the mouse maps through to where the view now is.
    pub(super) fn move_view(&mut self) {
        if let Some(view) = &mut self.view {
            view.pan = self.pan;
        }
    }

    /// Move the view `dx` × `dy` canvas cells over the canvas, or off it
    /// any distance; it leaves the selection where it is.
    pub fn pan_by(&mut self, dx: f32, dy: f32) {
        let pan = (self.pan.0 + dx, self.pan.1 + dy);
        if !(pan.0.is_finite() && pan.1.is_finite()) {
            return;
        }
        self.pan = pan;
        self.detached = true;
        self.move_view();
    }

    /// Zoom by `factor`, keeping the canvas point at `anchor` where it is
    /// on screen: `anchor` in screen cells from the view's top left, or
    /// its middle for `None`. It leaves the selection where it is.
    pub fn zoom_at(&mut self, factor: f32, anchor: Option<(f32, f32)>) {
        let before = self.zoom();
        let after = (before * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        if !after.is_finite() || after == before {
            return;
        }
        let (ax, ay) = anchor
            .or_else(|| {
                let area = self.view?.area;
                Some((f32::from(area.width) / 2.0, f32::from(area.height) / 2.0))
            })
            .unwrap_or((0.0, 0.0));
        self.pan.0 += ax / before - ax / after;
        self.pan.1 += ay / before - ay / after;
        self.zoom = Zoom(after);
        self.detached = true;
        self.move_view();
    }
}
