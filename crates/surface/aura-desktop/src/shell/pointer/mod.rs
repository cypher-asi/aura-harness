//! The pointer: over the grid it is the terminal UI's mouse, cell by
//! cell; on the panels' title bars and the borders between them it picks
//! tabs, focuses panels and drags; on the tab bar it works the tabs and
//! the window, which has no frame from the system: empty bar space moves
//! it and its edges resize it. On the GRAPH tab's canvas, which the
//! window draws, it drags the view by the pixel, clicks cards, and with
//! Ctrl held the wheel zooms around it, as a pinch does.

mod canvas;
mod press;
#[cfg(test)]
mod tests;

use super::{glide, Effect, Shell};
use crate::chrome::{self, Divider};
use crate::geom::{Point, Size};
use crate::tab_bar::{self, Hit};
use crossterm::event::{MouseButton as Button, MouseEvent, MouseEventKind};
use ratatui::layout::Position;
use std::time::{Duration, Instant};
use winit::event::{MouseButton, MouseScrollDelta};
use winit::window::{CursorIcon, ResizeDirection};

/// Width of the band along the window's sides that resizes it.
const EDGE: f32 = 6.0;
/// How far along a side from a corner the band resizes both ways.
const CORNER: f32 = 16.0;
/// Two presses on empty tab bar space this close together maximize.
const DOUBLE_CLICK: Duration = Duration::from_millis(400);
/// Most lines one turn of the wheel scrolls, whatever the device says.
pub(super) const MOST_LINES: f32 = 200.0;
/// Pixels a press on the canvas moves before it drags the view rather
/// than clicks, on a plain display.
const DRAG_SLOP: f32 = 3.0;
/// Canvas rows and columns a line of the wheel pans.
const WHEEL_ROWS: f32 = 3.0;
const WHEEL_COLUMNS: f32 = 8.0;
/// What a line of the wheel zooms the canvas by, with Ctrl held.
const WHEEL_ZOOM: f32 = 1.1;
/// Pixels of a touchpad's scroll, with Ctrl held, that zoom the canvas
/// by e.
const ZOOM_PIXELS: f32 = 240.0;

/// A press on the GRAPH tab's canvas, in the grid's part of the window.
#[derive(Debug, Clone, Copy)]
struct CanvasPress {
    from: Point,
    /// Where the view was last dragged to
    last: Point,
    /// It went far enough to drag
    moved: bool,
}

/// Where the pointer is and what it is doing.
#[derive(Default)]
pub struct Pointer {
    /// The grid cell under it
    pub(super) cell: Option<Position>,
    /// Where it is on the grid's part of the window, while over it
    pub(super) on_grid: Option<Point>,
    /// Where it is while over the tab bar
    pub(super) on_bar: Option<Point>,
    /// The divider being dragged
    dragging: Option<Divider>,
    /// The tab being dragged along the tab bar, by id: moves its menus
    /// ask for may close tabs mid-drag and shift the rest
    pub(super) tab_drag: Option<u64>,
    /// The window side or corner under it
    edge: Option<ResizeDirection>,
    /// When empty tab bar space was last pressed, for a double click
    last_bar_press: Option<Instant>,
    left_down: bool,
    /// Scroll distance not yet turned into whole lines
    scroll: f32,
    canvas: Option<CanvasPress>,
}

impl Shell {
    /// The pointer moved to `at`, in the window.
    pub fn pointer_moved(&mut self, at: Point) {
        let started = Instant::now();
        let hovers = (self.bar.hover(), self.chrome.hover());
        if let Some(id) = self.pointer.tab_drag {
            self.drag_tab_to(id, at);
            return;
        }
        let pointer = &mut self.pointer;
        pointer.edge = (!pointer.left_down)
            .then(|| edge(self.window, at, self.scale))
            .flatten();
        let bar = tab_bar::height(&self.metrics);
        // A drag that began in the grid keeps selecting there.
        if at.y < bar && !pointer.left_down && pointer.dragging.is_none() {
            self.point_bar(at, hovers);
            return;
        }
        pointer.on_bar = None;
        let in_grid = Point::new(at.x, at.y - bar);
        pointer.on_grid = Some(in_grid);
        self.bar.point(None);
        if let Some(divider) = pointer.dragging {
            self.drag_divider(divider, in_grid);
            return;
        }
        if self.pointer.canvas.is_some() {
            self.drag_canvas(in_grid);
            return;
        }
        self.point_grid(in_grid, hovers, started);
    }

    /// The pointer moved to `at` while dragging the tab `id` along the
    /// tab bar.
    fn drag_tab_to(&mut self, id: u64, at: Point) {
        self.pointer.on_bar = Some(at);
        self.bar.point(Some(at));
        // A tab gone mid-drag drags nothing until the button is let go.
        if let Some(from) = self.tabs.index_of(id) {
            if let Some(to) = self.bar.slot(at.x).filter(|to| *to != from) {
                self.tabs.move_tab(from, to);
            }
        }
        self.dirty = true;
    }

    /// The pointer moved to `at` over the tab bar; `hovers` is what was
    /// lit before.
    fn point_bar(&mut self, at: Point, hovers: (Option<Hit>, Option<chrome::Hit>)) {
        let pointer = &mut self.pointer;
        pointer.on_bar = Some(at);
        pointer.cell = None;
        pointer.on_grid = None;
        self.bar.point(Some(at));
        self.chrome.point(None);
        self.dirty |= hovers != (self.bar.hover(), self.chrome.hover());
        self.dirty |= self.tabs.front_mut().tui.hover_flow_at(None);
    }

    /// The pointer moved to `in_grid` while dragging `divider`.
    fn drag_divider(&mut self, divider: Divider, in_grid: Point) {
        let tui = &mut self.tabs.front_mut().tui;
        let splits = self.chrome.drag(divider, in_grid, tui.pane_splits());
        tui.set_pane_splits(splits);
        self.dirty = true;
    }

    /// The pointer moved to `in_grid` while pressed on the canvas: past
    /// the slop it drags the view by the pixel.
    fn drag_canvas(&mut self, in_grid: Point) {
        let Some(press) = &mut self.pointer.canvas else {
            return;
        };
        let slop = DRAG_SLOP * self.scale;
        let gone = (in_grid.x - press.from.x).hypot(in_grid.y - press.from.y);
        press.moved |= gone > slop;
        if press.moved {
            let cell = self.metrics.cell;
            let (dx, dy) = (in_grid.x - press.last.x, in_grid.y - press.last.y);
            press.last = in_grid;
            let tui = &mut self.tabs.front_mut().tui;
            let zoom = tui.flow().zoom();
            tui.pan_flow(-dx / (cell.width * zoom), -dy / (cell.height * zoom));
            self.dirty = true;
        }
    }

    /// The pointer moved to `in_grid` over the grid, dragging nothing:
    /// it lights what is under it and tells the terminal UI of a new cell.
    fn point_grid(
        &mut self,
        in_grid: Point,
        hovers: (Option<Hit>, Option<chrome::Hit>),
        started: Instant,
    ) {
        if !self.pointer.left_down {
            self.chrome.point(Some(in_grid));
        }
        self.dirty |= hovers != (self.bar.hover(), self.chrome.hover());
        let over = self.canvas_point(in_grid);
        self.dirty |= self.tabs.front_mut().tui.hover_flow_at(over);
        let cell = self.cell_at(in_grid);
        if self.pointer.cell == Some(cell) {
            return;
        }
        self.pointer.cell = Some(cell);
        let changed = self.send_mouse(if self.pointer.left_down {
            MouseEventKind::Drag(Button::Left)
        } else {
            MouseEventKind::Moved
        });
        if changed {
            self.settle(started);
        }
    }

    /// A file or folder was dropped on the window. The system sends no
    /// pointer position during a drag from outside on every platform, so
    /// a folder is taken wherever it lands.
    pub fn path_dropped(&mut self, path: &std::path::Path) {
        self.tabs.front_mut().tui.drop_path(path);
        self.dirty = true;
    }

    /// The pointer left the window.
    pub fn pointer_left(&mut self) {
        let hovers = (self.bar.hover(), self.chrome.hover());
        self.pointer.cell = None;
        self.pointer.on_bar = None;
        self.pointer.on_grid = None;
        self.pointer.edge = None;
        self.bar.point(None);
        self.chrome.point(None);
        self.dirty |= hovers != (self.bar.hover(), self.chrome.hover());
    }

    /// A mouse button went down (`pressed`) or came up.
    pub fn button(&mut self, button: MouseButton, pressed: bool, now: Instant) {
        let started = Instant::now();
        let kind = if pressed {
            self.button_down(button, now, started)
        } else {
            self.button_up(button, started)
        };
        let Some(kind) = kind else {
            return;
        };
        self.send_mouse(kind);
        self.settle(started);
    }

    /// `button` went down: what the terminal UI is to hear of it, if
    /// the window did not take it.
    fn button_down(
        &mut self,
        button: MouseButton,
        now: Instant,
        started: Instant,
    ) -> Option<MouseEventKind> {
        if let (Some(edge), MouseButton::Left) = (self.pointer.edge, button) {
            self.effects.push(Effect::Resize(edge));
            return None;
        }
        if self.pointer.on_bar.is_some() {
            self.bar_press(button, now);
            self.settle(started);
            return None;
        }
        if let Some(hit) = self.chrome.hover() {
            self.chrome_press(hit, button);
            self.settle(started);
            return None;
        }
        if let Some(at) = self.pointer.on_grid.filter(|_| button == MouseButton::Left) {
            if self.canvas_point(at).is_some() {
                self.pointer.canvas = Some(CanvasPress {
                    from: at,
                    last: at,
                    moved: false,
                });
                return None;
            }
        }
        let button = mouse_button(button)?;
        // The chat may have slid under a pointer that kept still.
        if let Some(at) = self.pointer.on_grid {
            self.pointer.cell = Some(self.cell_at(at));
        }
        self.pointer.left_down |= button == Button::Left;
        Some(MouseEventKind::Down(button))
    }

    /// `button` came up: what the terminal UI is to hear of it, if it
    /// did not end a drag of the window's.
    fn button_up(&mut self, button: MouseButton, started: Instant) -> Option<MouseEventKind> {
        if button == MouseButton::Left && self.pointer.tab_drag.take().is_some() {
            self.dirty = true;
            return None;
        }
        if button == MouseButton::Left && self.pointer.dragging.take().is_some() {
            self.chrome.point(self.pointer.on_grid);
            self.dirty = true;
            return None;
        }
        if button == MouseButton::Left {
            if let Some(press) = self.pointer.canvas.take() {
                if !press.moved {
                    self.click_canvas(press.from);
                }
                self.settle(started);
                return None;
            }
        }
        let button = mouse_button(button)?;
        if button == Button::Left {
            self.pointer.left_down = false;
        }
        Some(MouseEventKind::Up(button))
    }

    /// The wheel turned, or two fingers moved on a touchpad.
    pub fn wheel(&mut self, delta: MouseScrollDelta, now: Instant) {
        if self.pointer.on_bar.is_some() || self.glide_wheel(delta, now) {
            return;
        }
        let started = Instant::now();
        if self.modifiers.control_key() && self.zoom_canvas(delta) {
            self.settle(started);
            return;
        }
        if self.wheel_canvas(delta) {
            self.settle(started);
            return;
        }
        #[allow(clippy::cast_possible_truncation)]
        let lines = match delta {
            MouseScrollDelta::LineDelta(_, y) => y,
            MouseScrollDelta::PixelDelta(by) => {
                by.y as f32 * glide::TOUCHPAD_GAIN / self.metrics.cell.height
            }
        } * glide::speed();
        if !lines.is_finite() {
            return;
        }
        self.pointer.scroll += lines.clamp(-MOST_LINES, MOST_LINES);
        let mut scrolled = false;
        while self.pointer.scroll.abs() >= 1.0 {
            let kind = if self.pointer.scroll > 0.0 {
                self.pointer.scroll -= 1.0;
                MouseEventKind::ScrollUp
            } else {
                self.pointer.scroll += 1.0;
                MouseEventKind::ScrollDown
            };
            self.send_mouse(kind);
            scrolled = true;
        }
        if scrolled {
            self.settle(started);
        }
    }

    /// The pointer's shape where it is: arrows over what resizes or
    /// drags, a hand over a panel's tab.
    pub fn cursor_icon(&self) -> CursorIcon {
        let hover = self.chrome.hover();
        let divider = self.pointer.dragging.or(match hover {
            Some(chrome::Hit::Divider(divider)) => Some(divider),
            _ => None,
        });
        let panning = self.pointer.canvas.is_some_and(|press| press.moved)
            || self.tabs.front().tui.project_dragging();
        if self.pointer.tab_drag.is_some() || panning {
            CursorIcon::Grabbing
        } else if let Some(edge) = self.pointer.edge {
            resize_cursor(edge)
        } else if let Some(divider) = divider {
            if divider.sideways() {
                CursorIcon::EwResize
            } else {
                CursorIcon::NsResize
            }
        } else if matches!(
            hover,
            Some(
                chrome::Hit::Tab(_)
                    | chrome::Hit::Close(_)
                    | chrome::Hit::Expand(_)
                    | chrome::Hit::Scope(_)
                    | chrome::Hit::SideToggle
            )
        ) {
            CursorIcon::Pointer
        } else {
            CursorIcon::Default
        }
    }
    /// Hand the terminal UI a mouse event at the pointer's cell, and say
    /// whether the screen may look different for it.
    fn send_mouse(&mut self, kind: MouseEventKind) -> bool {
        let Some(at) = self.pointer.cell else {
            return false;
        };
        let modifiers = super::keys::modifiers(self.modifiers);
        let changed = self.tabs.front_mut().tui.handle_mouse(MouseEvent {
            kind,
            column: at.x,
            row: at.y,
            modifiers,
        });
        self.tabs.adopt_project_order();
        changed
    }
}

/// The side or corner of a `window`-sized window that `at` resizes, on
/// a display of `scale` pixels to a plain one's.
fn edge(window: Size, at: Point, scale: f32) -> Option<ResizeDirection> {
    use ResizeDirection::{East, North, NorthEast, NorthWest, South, SouthEast, SouthWest, West};
    let within = |band: f32| {
        let band = band * scale;
        (
            at.x < band,
            at.x >= window.width - band,
            at.y < band,
            at.y >= window.height - band,
        )
    };
    let (west, east, north, south) = within(EDGE);
    let (near_west, near_east, near_north, near_south) = within(CORNER);
    let direction = match () {
        () if (north && near_west) || (west && near_north) => NorthWest,
        () if (north && near_east) || (east && near_north) => NorthEast,
        () if (south && near_west) || (west && near_south) => SouthWest,
        () if (south && near_east) || (east && near_south) => SouthEast,
        () if north => North,
        () if south => South,
        () if west => West,
        () if east => East,
        () => return None,
    };
    Some(direction)
}

const fn resize_cursor(edge: ResizeDirection) -> CursorIcon {
    use ResizeDirection::{East, North, NorthEast, NorthWest, South, SouthEast, SouthWest, West};
    match edge {
        North | South => CursorIcon::NsResize,
        East | West => CursorIcon::EwResize,
        NorthEast | SouthWest => CursorIcon::NeswResize,
        NorthWest | SouthEast => CursorIcon::NwseResize,
    }
}

const fn mouse_button(button: MouseButton) -> Option<Button> {
    match button {
        MouseButton::Left => Some(Button::Left),
        MouseButton::Right => Some(Button::Right),
        MouseButton::Middle => Some(Button::Middle),
        _ => None,
    }
}
