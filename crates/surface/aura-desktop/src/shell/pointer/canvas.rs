//! The GRAPH tab's canvas under the pointer: zooming around it, panning
//! with the wheel and clicking its cards.

use super::super::Shell;
use super::{MOST_LINES, WHEEL_COLUMNS, WHEEL_ROWS, WHEEL_ZOOM, ZOOM_PIXELS};
use crate::geom::Point;
use ratatui::layout::Position;
use std::time::Instant;
use winit::event::MouseScrollDelta;

impl Shell {
    /// Two fingers pinched on a touchpad, by `delta`: the GRAPH tab's
    /// canvas zooms around the pointer.
    pub fn pinch(&mut self, delta: f32) {
        let Some(at) = self.pointer.on_grid else {
            return;
        };
        let started = Instant::now();
        if self.zoom_canvas_at(at, delta.exp()) {
            self.settle(started);
        }
    }

    /// The wheel with Ctrl held zooms the canvas around the pointer.
    /// Returns whether it was over the canvas.
    pub(super) fn zoom_canvas(&mut self, delta: MouseScrollDelta) -> bool {
        let Some(at) = self.pointer.on_grid else {
            return false;
        };
        #[allow(clippy::cast_possible_truncation)]
        let factor = match delta {
            MouseScrollDelta::LineDelta(_, y) => WHEEL_ZOOM.powf(y),
            MouseScrollDelta::PixelDelta(by) => (by.y as f32 / ZOOM_PIXELS).exp(),
        };
        self.zoom_canvas_at(at, factor)
    }

    /// Zoom the canvas by `factor`, keeping the point at `at` where it is.
    /// Returns whether `at` is on the canvas.
    fn zoom_canvas_at(&mut self, at: Point, factor: f32) -> bool {
        let Some(anchor) = self.canvas_offset(at) else {
            return false;
        };
        if factor.is_finite() && factor > 0.0 {
            self.tabs
                .front_mut()
                .tui
                .zoom_flow(factor.clamp(0.5, 2.0), Some(anchor));
            self.dirty = true;
        }
        true
    }

    /// The GRAPH tab's canvas point, in canvas cells, at `at` on the
    /// grid's part of the window, when the window draws the canvas there.
    pub(super) fn canvas_point(&self, at: Point) -> Option<(f32, f32)> {
        let (x, y) = self.canvas_offset(at)?;
        let flow = self.tabs.front().tui.flow();
        let (pan, zoom) = (flow.pan(), flow.zoom());
        Some((pan.0 + x / zoom, pan.1 + y / zoom))
    }

    /// How far `at` on the grid's part of the window is from the top left
    /// of the GRAPH tab's canvas, in grid cells, when the window draws the
    /// canvas there.
    fn canvas_offset(&self, at: Point) -> Option<(f32, f32)> {
        let tui = &self.tabs.front().tui;
        let area = tui.flow_canvas()?;
        let origin = self.metrics.origin(self.grid_area(), self.cols);
        let cell = self.metrics.cell;
        let (x, y) = (
            (at.x - origin.x) / cell.width,
            (at.y - origin.y) / cell.height,
        );
        let on = |at: f32| {
            (at >= 0.0 && at < f32::from(u16::MAX)).then(|| {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let whole = at.floor() as u16;
                whole
            })
        };
        let cell_at = Position::new(on(x)?, on(y)?);
        if !tui
            .flow_canvas_clip()
            .iter()
            .any(|rect| rect.contains(cell_at))
        {
            return None;
        }
        Some((x - f32::from(area.x), y - f32::from(area.y)))
    }

    /// A press and release on the canvas that did not drag: it works the
    /// card there, or focuses the chat panel on empty canvas.
    pub(super) fn click_canvas(&mut self, at: Point) {
        let Some((x, y)) = self.canvas_point(at) else {
            return;
        };
        let tui = &mut self.tabs.front_mut().tui;
        if !tui.click_flow_at(x, y) {
            tui.focus_pane(aura_ui_terminal::Pane::Chat);
        }
    }

    /// The wheel over the canvas pans it, by fractions of a cell on a
    /// touchpad; sideways with Shift held. Returns whether it was there.
    pub(super) fn wheel_canvas(&mut self, delta: MouseScrollDelta) -> bool {
        let Some(at) = self.pointer.on_grid else {
            return false;
        };
        if self.canvas_point(at).is_none() {
            return false;
        }
        let cell = self.metrics.cell;
        let zoom = self.tabs.front().tui.flow().zoom();
        #[allow(clippy::cast_possible_truncation)]
        let (dx, dy) = match delta {
            MouseScrollDelta::LineDelta(x, y) => (x * WHEEL_COLUMNS / zoom, y * WHEEL_ROWS / zoom),
            MouseScrollDelta::PixelDelta(by) => (
                by.x as f32 / (cell.width * zoom),
                by.y as f32 / (cell.height * zoom),
            ),
        };
        let (dx, dy) = if self.modifiers.shift_key() && dx == 0.0 {
            (dy * WHEEL_COLUMNS / WHEEL_ROWS, 0.0)
        } else {
            (dx, dy)
        };
        if dx.is_finite() && dy.is_finite() {
            let most = MOST_LINES;
            self.tabs
                .front_mut()
                .tui
                .pan_flow(-dx.clamp(-most, most), -dy.clamp(-most, most));
        }
        true
    }
}
