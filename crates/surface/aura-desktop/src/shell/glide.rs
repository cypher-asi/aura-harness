//! The chat scrolls by pixels: a touchpad's movement as it comes, a
//! wheel's notches eased out over a few frames. The terminal UI scrolls
//! by whole rows, and between them the chat's body is drawn up to a row
//! higher than its cells, so a frame that only slides it is painted from
//! the rows already rendered.

use super::pointer::MOST_LINES;
use super::Shell;
use crate::geom::Point;
use crate::paint::Slide;
use aura_ui_terminal::App;
use ratatui::layout::Position;
use std::sync::LazyLock;
use std::time::{Duration, Instant};
use winit::event::MouseScrollDelta;

/// Rows a notch of the wheel scrolls the chat.
pub(super) const NOTCH_ROWS: f32 = 16.0;
/// Notches this soon after the one before are a spin of the wheel, and
/// each goes further than the last.
const SPIN: Duration = Duration::from_millis(150);
/// How much further each notch of a spin goes.
const SPIN_GAIN: f32 = 1.5;
/// The furthest a notch of a spin goes, in notches.
const MOST_GAIN: f32 = 8.0;
/// Pixels the chat moves per pixel of a touchpad's movement.
pub(super) const TOUCHPAD_GAIN: f32 = 6.0;
/// How quickly an eased scroll gets there: what is left of it shrinks
/// by a factor of e every this long.
const EASE: Duration = Duration::from_millis(30);
/// The step of the first eased frame, which has no frame before it to go
/// by.
const FIRST_STEP: Duration = Duration::from_millis(16);
/// The longest step one frame takes, however late it comes.
const LONGEST_STEP: Duration = Duration::from_millis(100);
/// Pixels left that the next step covers all of.
const CLOSE: f32 = 0.5;

/// `AURA_DESKTOP_SCROLL_SPEED`: how far the wheel and the touchpad
/// scroll, times what they do by default.
pub(super) fn speed() -> f32 {
    static SPEED: LazyLock<f32> = LazyLock::new(|| {
        std::env::var("AURA_DESKTOP_SCROLL_SPEED")
            .ok()
            .and_then(|speed| speed.trim().parse::<f32>().ok())
            .filter(|speed| speed.is_finite() && *speed > 0.0)
            .map_or(1.0, |speed| speed.clamp(0.1, 10.0))
    });
    *SPEED
}

/// Where the chat is between rows, and where an eased scroll is going.
#[derive(Debug, Default)]
pub struct Glide {
    /// Pixels the chat's body is drawn higher than its cells: less than
    /// a row
    by: f32,
    /// Pixels still to scroll as notches ease out, up when positive
    pending: f32,
    /// When the last eased step was taken
    stepped: Option<Instant>,
    /// When the wheel last turned over the chat
    notched: Option<Instant>,
    /// How many notches the last one went, in a spin
    gain: f32,
    /// Whether there is a frame to draw for the chat sliding, or easing
    pub(super) moved: bool,
}

impl Glide {
    /// The chat's body of `tui` as the painter slides it.
    #[allow(clippy::cast_possible_truncation)]
    pub(super) fn slide<'a>(&self, tui: &'a App) -> Option<Slide<'a>> {
        let view = tui.chat_scroll()?;
        let (next, next_prose) = tui.chat_row_below();
        Some(Slide {
            body: view.body,
            by: self.by.round() as i32,
            next,
            next_prose,
        })
    }

    pub(super) fn stop(&mut self) {
        self.moved |= self.by != 0.0;
        self.by = 0.0;
        self.pending = 0.0;
        self.stepped = None;
        self.notched = None;
    }
}

impl Shell {
    /// Scroll the chat by pixels for `delta` when the pointer is over its
    /// body; false leaves `delta` to scroll whole rows.
    pub(super) fn glide_wheel(&mut self, delta: MouseScrollDelta, now: Instant) -> bool {
        let Some(view) = self.tabs.front().tui.chat_scroll() else {
            self.glide.stop();
            return false;
        };
        if !self.pointer.cell.is_some_and(|at| view.body.contains(at)) {
            return false;
        }
        let row = self.metrics.cell.height;
        let most = MOST_LINES * row;
        match delta {
            MouseScrollDelta::LineDelta(_, notches) if notches.is_finite() && notches != 0.0 => {
                let glide = &mut self.glide;
                let spinning = glide
                    .notched
                    .is_some_and(|at| now.saturating_duration_since(at) < SPIN);
                // Turning the wheel back stops the scroll that is easing out.
                if glide.pending != 0.0 && glide.pending.signum() != notches.signum() {
                    glide.pending = 0.0;
                    glide.gain = 1.0;
                } else {
                    glide.gain = if spinning {
                        (glide.gain + SPIN_GAIN).min(MOST_GAIN)
                    } else {
                        1.0
                    };
                }
                glide.notched = Some(now);
                let px = notches * NOTCH_ROWS * row * glide.gain * speed();
                glide.pending = (glide.pending + px).clamp(-most, most);
                glide.moved = true;
            }
            #[allow(clippy::cast_possible_truncation)]
            MouseScrollDelta::PixelDelta(by) if by.y.is_finite() => {
                let px = by.y as f32 * TOUCHPAD_GAIN * speed();
                self.slide_chat(px.clamp(-most, most));
            }
            _ => {}
        }
        true
    }

    /// Take the eased scroll's step for the frame at `now`.
    pub(super) fn ease(&mut self, now: Instant) {
        let pending = self.glide.pending;
        if pending == 0.0 {
            self.glide.stepped = None;
            return;
        }
        let dt = self
            .glide
            .stepped
            .map_or(FIRST_STEP, |at| now.saturating_duration_since(at))
            .min(LONGEST_STEP);
        let mut step = pending * (1.0 - (-dt.as_secs_f32() / EASE.as_secs_f32()).exp());
        if (pending - step).abs() < CLOSE {
            step = pending;
        }
        self.glide.pending -= step;
        self.glide.stepped = Some(now);
        self.slide_chat(step);
    }

    /// Slide the chat `px` pixels up, or down when negative, scrolling the
    /// terminal UI by the whole rows that makes. It stops at either end;
    /// at the bottom it is on its rows, so new output keeps it there.
    fn slide_chat(&mut self, px: f32) {
        let row = self.metrics.cell.height;
        let tui = &mut self.tabs.front_mut().tui;
        let Some(view) = tui.chat_scroll() else {
            self.glide.stop();
            return;
        };
        let total = view.above + view.below;
        let mut below = tui.scroll_offset().min(total);
        let mut above = total - below;
        let mut by = self.glide.by - px;
        let mut rows = 0_isize;
        while by < 0.0 {
            if above == 0 {
                by = 0.0;
                self.glide.pending = self.glide.pending.min(0.0);
                break;
            }
            (above, below, rows, by) = (above - 1, below + 1, rows + 1, by + row);
        }
        while by >= row && below > 0 {
            (above, below, rows, by) = (above + 1, below - 1, rows - 1, by - row);
        }
        if below == 0 {
            by = 0.0;
            self.glide.pending = self.glide.pending.max(0.0);
        }
        if rows != 0 {
            tui.scroll_chat_rows(rows);
            self.dirty = true;
        }
        if by != self.glide.by {
            self.glide.by = by;
            self.glide.moved = true;
        }
    }

    /// Fit the slide to the chat as just rendered: none while something
    /// covers the chat, and none at the bottom.
    pub(super) fn fit_glide(&mut self) {
        match self.tabs.front().tui.chat_scroll() {
            None => self.glide.stop(),
            Some(view) if view.below == 0 => {
                self.glide.by = 0.0;
                self.glide.pending = self.glide.pending.max(0.0);
            }
            Some(_) => {}
        }
    }

    /// Whether an eased scroll has frames to go.
    pub(super) fn easing(&self) -> bool {
        self.glide.pending != 0.0
    }

    /// The grid cell at `in_grid` on the grid's part of the window: in
    /// the chat's body, the cell drawn there as it slides.
    pub(super) fn cell_at(&self, in_grid: Point) -> Position {
        let area = self.grid_area();
        let cell = self.metrics.cell_at(area, self.cols, self.rows, in_grid);
        let origin = self.metrics.origin(area, self.cols);
        let bands = self.tabs.front().tui.list_bands();
        let cell = crate::paint::cell_in_bands(bands, self.metrics, cell, in_grid.y - origin.y);
        let by = self.glide.by.round();
        let Some(view) = self.tabs.front().tui.chat_scroll().filter(|_| by > 0.0) else {
            return cell;
        };
        if !view.body.contains(cell) {
            return cell;
        }
        let lower = Point::new(in_grid.x, in_grid.y + by);
        let slid = self.metrics.cell_at(area, self.cols, self.rows, lower);
        Position::new(cell.x, slid.y.min(view.body.bottom().saturating_sub(1)))
    }
}
