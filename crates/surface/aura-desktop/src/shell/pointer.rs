//! The pointer: over the grid it is the terminal UI's mouse, cell by
//! cell; on the panels' title bars and the borders between them it picks
//! tabs, focuses panels and drags; on the tab bar it works the tabs and
//! the window, which has no frame from the system: empty bar space moves
//! it and its edges resize it. On the GRAPH tab's canvas, which the
//! window draws, it drags the view by the pixel, clicks cards, and with
//! Ctrl held the wheel zooms around it, as a pinch does.

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
    /// Where the tab being dragged along the tab bar is now
    pub(super) tab_drag: Option<usize>,
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
        if let Some(from) = self.pointer.tab_drag {
            self.pointer.on_bar = Some(at);
            self.bar.point(Some(at));
            if let Some(to) = self.bar.slot(at.x).filter(|to| *to != from) {
                self.tabs.move_tab(from, to);
                self.pointer.tab_drag = Some(to);
            }
            self.dirty = true;
            return;
        }
        let pointer = &mut self.pointer;
        pointer.edge = (!pointer.left_down)
            .then(|| edge(self.window, at, self.scale))
            .flatten();
        let bar = tab_bar::height(&self.metrics);
        // A drag that began in the grid keeps selecting there.
        if at.y < bar && !pointer.left_down && pointer.dragging.is_none() {
            pointer.on_bar = Some(at);
            pointer.cell = None;
            pointer.on_grid = None;
            self.bar.point(Some(at));
            self.chrome.point(None);
            self.dirty |= hovers != (self.bar.hover(), self.chrome.hover());
            self.dirty |= self.tabs.front_mut().tui.hover_flow_at(None);
            return;
        }
        pointer.on_bar = None;
        let in_grid = Point::new(at.x, at.y - bar);
        pointer.on_grid = Some(in_grid);
        self.bar.point(None);
        if let Some(divider) = pointer.dragging {
            let tui = &mut self.tabs.front_mut().tui;
            let splits = self.chrome.drag(divider, in_grid, tui.pane_splits());
            tui.set_pane_splits(splits);
            self.dirty = true;
            return;
        }
        if let Some(press) = &mut pointer.canvas {
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
            return;
        }
        if !pointer.left_down {
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
            if let (Some(edge), MouseButton::Left) = (self.pointer.edge, button) {
                self.effects.push(Effect::Resize(edge));
                return;
            }
            if self.pointer.on_bar.is_some() {
                self.bar_press(button, now);
                self.settle(started);
                return;
            }
            if let Some(hit) = self.chrome.hover() {
                self.chrome_press(hit, button);
                self.settle(started);
                return;
            }
            if let Some(at) = self.pointer.on_grid.filter(|_| button == MouseButton::Left) {
                if self.canvas_point(at).is_some() {
                    self.pointer.canvas = Some(CanvasPress {
                        from: at,
                        last: at,
                        moved: false,
                    });
                    return;
                }
            }
            let Some(button) = mouse_button(button) else {
                return;
            };
            // The chat may have slid under a pointer that kept still.
            if let Some(at) = self.pointer.on_grid {
                self.pointer.cell = Some(self.cell_at(at));
            }
            self.pointer.left_down |= button == Button::Left;
            MouseEventKind::Down(button)
        } else {
            if button == MouseButton::Left && self.pointer.tab_drag.take().is_some() {
                self.dirty = true;
                return;
            }
            if button == MouseButton::Left && self.pointer.dragging.take().is_some() {
                self.chrome.point(self.pointer.on_grid);
                self.dirty = true;
                return;
            }
            if button == MouseButton::Left {
                if let Some(press) = self.pointer.canvas.take() {
                    if !press.moved {
                        self.click_canvas(press.from);
                    }
                    self.settle(started);
                    return;
                }
            }
            let Some(button) = mouse_button(button) else {
                return;
            };
            if button == Button::Left {
                self.pointer.left_down = false;
            }
            MouseEventKind::Up(button)
        };
        self.send_mouse(kind);
        self.settle(started);
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
            )
        ) {
            CursorIcon::Pointer
        } else {
            CursorIcon::Default
        }
    }

    /// A button went down on the tab bar: left picks a tab and starts
    /// dragging it along the bar, opens a tab or works a window box,
    /// middle closes the tab under the pointer, and left on empty space
    /// moves the window, or maximizes it when pressed twice.
    fn bar_press(&mut self, button: MouseButton, now: Instant) {
        match (self.bar.hover(), button) {
            (Some(Hit::Tab(index)), MouseButton::Left) => {
                self.tabs.focus(index);
                self.pointer.tab_drag = Some(index);
            }
            (Some(Hit::Tab(index)), MouseButton::Middle) => {
                self.tabs.close(index);
            }
            (Some(Hit::New), MouseButton::Left) => self.tabs.new_tab(),
            (Some(Hit::Minimize), MouseButton::Left) => self.effects.push(Effect::Minimize),
            (Some(Hit::Maximize), MouseButton::Left) => self.effects.push(Effect::ToggleMaximize),
            (Some(Hit::Close), MouseButton::Left) => self.effects.push(Effect::Exit),
            (None, MouseButton::Left) => {
                let double = self
                    .pointer
                    .last_bar_press
                    .is_some_and(|at| now.saturating_duration_since(at) < DOUBLE_CLICK);
                self.pointer.last_bar_press = (!double).then_some(now);
                self.effects.push(if double {
                    Effect::ToggleMaximize
                } else {
                    Effect::Drag
                });
            }
            _ => {}
        }
    }

    /// A button went down on a panel's title bar or a divider: left picks
    /// the tab, closes what covers the panel, expands or restores it,
    /// switches its lists between the agent's and the project's, focuses
    /// the panel or starts dragging the divider.
    fn chrome_press(&mut self, hit: chrome::Hit, button: MouseButton) {
        if button != MouseButton::Left {
            return;
        }
        let tui = &mut self.tabs.front_mut().tui;
        match hit {
            chrome::Hit::Tab(tab) => tui.select_pane_tab(tab),
            chrome::Hit::Close(pane) => tui.close_pane_view(pane),
            chrome::Hit::Expand(pane) => tui.expand_pane(pane),
            chrome::Hit::Scope(pane) => tui.toggle_pane_scope(pane),
            chrome::Hit::Bar(pane) => tui.focus_pane(pane),
            chrome::Hit::Divider(divider) => self.pointer.dragging = Some(divider),
        }
    }

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
    fn zoom_canvas(&mut self, delta: MouseScrollDelta) -> bool {
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
    fn canvas_point(&self, at: Point) -> Option<(f32, f32)> {
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
    fn click_canvas(&mut self, at: Point) {
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
    fn wheel_canvas(&mut self, delta: MouseScrollDelta) -> bool {
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

#[cfg(test)]
mod tests {
    use super::super::tests::{stage, Stage};
    use super::*;
    use crate::shell::keys::Press;
    use crate::shell::WINDOW;
    use aura_ui::events::UiCommand;
    use aura_ui_terminal::FLOW_MIN_ZOOM;
    use aura_ui_terminal::{ChatTab, Pane, PaneChrome, PaneTab};
    use std::time::Duration;
    use winit::dpi::PhysicalPosition;
    use winit::keyboard::{ModifiersState, NamedKey};

    impl Stage {
        /// Draw a frame, so what is pointed at is where it was drawn.
        fn draw(&mut self) {
            self.shell.frame(self.now);
        }

        /// Press and let go of `button` on a box of the tab bar.
        pub fn click(&mut self, hit: Hit, button: MouseButton) {
            self.draw();
            let at = self.shell.bar.spot(hit).expect("the box is drawn");
            self.shell.pointer_moved(at);
            self.shell.button(button, true, self.now);
            self.shell.button(button, false, self.now);
            self.draw();
        }

        /// Press the left button on a box of the tab bar and let go over
        /// another.
        pub fn drag_tab(&mut self, from: Hit, to: Hit) {
            self.draw();
            let from = self.shell.bar.spot(from).expect("the box is drawn");
            let to = self.shell.bar.spot(to).expect("the box is drawn");
            self.shell.pointer_moved(from);
            self.shell.button(MouseButton::Left, true, self.now);
            self.shell.pointer_moved(to);
            self.shell.button(MouseButton::Left, false, self.now);
            self.draw();
        }

        /// Press the left button at `from` on the grid's part of the
        /// window and let go at `to`.
        pub fn drag(&mut self, from: Point, to: Point) {
            self.draw();
            let bar = tab_bar::height(&self.shell.metrics);
            let on_window = |at: Point| Point::new(at.x, at.y + bar);
            self.shell.pointer_moved(on_window(from));
            self.shell.button(MouseButton::Left, true, self.now);
            self.shell.pointer_moved(on_window(to));
            self.shell.button(MouseButton::Left, false, self.now);
            self.draw();
        }

        fn chat(&self) -> PaneChrome {
            let tui = &self.shell.tabs.front().tui;
            let pane = tui.pane_chrome().iter().find(|c| c.pane == Pane::Chat);
            pane.expect("the chat has a bar").clone()
        }
    }

    #[test]
    fn the_window_edges_resize_it() {
        use ResizeDirection::{East, North, NorthWest, SouthEast, SouthWest};
        let size = Size::new(800.0, 600.0);
        let at = |x, y| edge(size, Point::new(x, y), 1.0);
        assert_eq!(at(400.0, 2.0), Some(North));
        assert_eq!(at(798.0, 300.0), Some(East));
        assert_eq!(at(2.0, 2.0), Some(NorthWest));
        assert_eq!(at(10.0, 2.0), Some(NorthWest), "near the corner");
        assert_eq!(at(2.0, 590.0), Some(SouthWest));
        assert_eq!(at(799.0, 599.0), Some(SouthEast));
        assert_eq!(at(400.0, 300.0), None);
        assert_eq!(at(400.0, 10.0), None, "the tab bar below the band");
        // Twice the pixels, twice the band.
        assert_eq!(edge(size, Point::new(400.0, 10.0), 2.0), Some(North));
    }

    #[test]
    fn the_edges_resize_the_window_and_show_it() {
        let mut stage = stage();
        stage.draw();
        stage
            .shell
            .pointer_moved(Point::new(WINDOW.width - 2.0, 400.0));
        assert_eq!(stage.shell.cursor_icon(), CursorIcon::EwResize);
        stage.shell.button(MouseButton::Left, true, stage.now);
        assert_eq!(
            stage.shell.take_effects(),
            [Effect::Resize(ResizeDirection::East)]
        );
        stage
            .shell
            .pointer_moved(Point::new(3.0, WINDOW.height - 3.0));
        assert_eq!(stage.shell.cursor_icon(), CursorIcon::NeswResize);
        stage.shell.pointer_moved(Point::new(400.0, 400.0));
        assert_eq!(stage.shell.cursor_icon(), CursorIcon::Default);
        stage.shell.pointer_left();
        assert_eq!(stage.shell.cursor_icon(), CursorIcon::Default);
    }

    #[test]
    fn the_tab_bar_moves_and_maximizes_the_window() {
        let mut stage = stage();
        stage.draw();
        // Between the title and the window boxes.
        let boxes = stage.shell.bar.spot(Hit::Minimize).expect("the boxes");
        let empty = Point::new(boxes.x - 60.0, boxes.y);
        stage.shell.pointer_moved(empty);
        assert_eq!(stage.shell.bar.hover(), None, "nothing is there");
        stage.shell.button(MouseButton::Left, true, stage.now);
        assert_eq!(stage.shell.take_effects(), [Effect::Drag]);

        stage.now += Duration::from_millis(200);
        stage.shell.button(MouseButton::Left, true, stage.now);
        assert_eq!(stage.shell.take_effects(), [Effect::ToggleMaximize]);
        // The third press of three is the first of the next pair.
        stage.now += Duration::from_millis(200);
        stage.shell.button(MouseButton::Left, true, stage.now);
        assert_eq!(stage.shell.take_effects(), [Effect::Drag]);
        stage.now += Duration::from_secs(1);
        stage.shell.button(MouseButton::Left, true, stage.now);
        assert_eq!(stage.shell.take_effects(), [Effect::Drag], "too late");

        stage.click(Hit::Minimize, MouseButton::Left);
        assert_eq!(stage.shell.take_effects(), [Effect::Minimize]);
        stage.click(Hit::Maximize, MouseButton::Left);
        assert_eq!(stage.shell.take_effects(), [Effect::ToggleMaximize]);
        stage.click(Hit::Close, MouseButton::Left);
        assert_eq!(stage.shell.take_effects(), [Effect::Exit]);
    }

    #[test]
    fn hovering_a_box_lights_it_and_asks_for_a_frame() {
        let mut stage = stage();
        let plain = stage.pixels();
        let at = stage.shell.bar.spot(Hit::Close).expect("the box");
        stage.shell.pointer_moved(at);
        assert_eq!(stage.shell.bar.hover(), Some(Hit::Close));
        assert!(stage.shell.dirty());
        let lit = stage.pixels();
        assert!(plain != lit);

        stage.shell.pointer_moved(Point::new(at.x + 1.0, at.y));
        assert!(!stage.shell.dirty(), "still on it: nothing new to draw");
        stage.shell.pointer_left();
        assert!(stage.shell.dirty());
        assert!(plain == stage.pixels());
    }

    #[test]
    fn tabs_open_switch_and_close_from_the_bar() {
        let mut stage = stage();
        stage.seed(false);
        stage.draw();
        let _ = stage.asked();

        stage.click(Hit::New, MouseButton::Left);
        assert!(stage.asked().is_empty(), "nothing until a folder is picked");
        stage.pick_folder();
        assert_eq!(stage.shell.tabs.len(), 1, "the tab opens with its project");
        stage.created("a3", "agent03");
        assert_eq!(stage.shell.tabs.len(), 2);
        assert_eq!(stage.shell.tabs.front_index(), 1);
        let _ = stage.asked();

        stage.click(Hit::Tab(0), MouseButton::Left);
        assert_eq!(stage.asked(), [r#"FocusAgent("a1")"#]);
        assert_eq!(stage.shell.tabs.front_index(), 0);

        stage.click(Hit::Tab(1), MouseButton::Middle);
        assert_eq!(stage.shell.tabs.len(), 1);
        assert!(stage.shell.take_effects().is_empty());
    }

    #[test]
    fn panel_tabs_and_dividers_belong_to_the_window() {
        let mut stage = stage();
        stage.seed(false);
        stage.draw();
        assert!(
            !stage.shell.text().contains("TERMINAL"),
            "tab labels are drawn, not cells"
        );

        let terminal = PaneTab::Chat(ChatTab::Terminal);
        let tab = stage.shell.chrome.spot(terminal).expect("the tab is drawn");
        stage.shell.pointer_moved(Point::new(
            tab.x,
            tab.y + tab_bar::height(&stage.shell.metrics),
        ));
        assert_eq!(stage.shell.cursor_icon(), CursorIcon::Pointer);
        stage.drag(tab, tab);
        assert_eq!(stage.shell.tabs.front().tui.chat_tab(), ChatTab::Terminal);
        assert!(stage.chat().focused);
        assert_eq!(stage.chat().active, Some(terminal));

        let before = stage.chat().area;
        let band = stage
            .shell
            .chrome
            .band(Divider::Agents)
            .expect("agents beside the chat");
        let cell = stage.shell.metrics.cell.width;
        stage.shell.pointer_moved(Point::new(
            band.x,
            band.y + tab_bar::height(&stage.shell.metrics),
        ));
        assert_eq!(stage.shell.cursor_icon(), CursorIcon::EwResize);
        stage.drag(band, Point::new(band.x + 10.0 * cell, band.y));
        let after = stage.chat().area;
        assert_eq!(after.right(), before.right());
        assert_eq!(after.x, before.x + 10, "{before:?} → {after:?}");
    }

    #[test]
    fn the_chats_scrollbar_scrolls_it_and_the_divider_still_moves_beside_it() {
        use aura_ui::events::{MessageData, MessageRole};

        let mut stage = stage();
        stage.seed(false);
        stage.shell.commands((0..80).map(|i| {
            UiCommand::ShowMessage(MessageData::new(
                MessageRole::Assistant,
                format!("reply {i}"),
            ))
        }));
        stage.draw();
        let chat = stage.chat().area;
        let (cell, area) = (stage.shell.metrics.cell, stage.shell.grid_area());
        let origin = stage.shell.metrics.origin(area, stage.shell.cols);
        let at = |col: u16, row: u16| {
            Point::new(
                origin.x + (f32::from(col) + 0.5) * cell.width,
                origin.y + (f32::from(row) + 0.5) * cell.height,
            )
        };
        let border = chat.right() - 1;
        let middle = at(border, chat.y + chat.height / 2);
        let bar = tab_bar::height(&stage.shell.metrics);
        stage
            .shell
            .pointer_moved(Point::new(middle.x, middle.y + bar));
        assert_eq!(stage.shell.chrome.hover(), None, "no divider on the border");
        assert_eq!(stage.shell.cursor_icon(), CursorIcon::Default);

        assert_eq!(stage.shell.tabs.front().tui.scroll_offset(), 0);
        stage.drag(middle, at(border, chat.y));
        let tui = &stage.shell.tabs.front().tui;
        assert!(tui.scroll_offset() > 0, "the chat scrolled up");
        assert!(tui.selection().is_none(), "nothing is selected");
        assert_eq!(stage.chat().area, chat, "the divider stayed");
        assert!(stage.shell.take_effects().is_empty(), "nothing copied");

        // The blank columns left of the border take hold of it too, two
        // of them, so it is wider to hit than the line it draws.
        let beside = border - 2;
        let from = at(beside, chat.y + chat.height / 2);
        stage.shell.pointer_moved(Point::new(from.x, from.y + bar));
        assert_eq!(stage.shell.chrome.hover(), None);
        stage.drag(from, at(beside, chat.bottom() - 1));
        let tui = &stage.shell.tabs.front().tui;
        assert_eq!(tui.scroll_offset(), 0, "back at the bottom");
        assert!(tui.selection().is_none(), "nothing is selected");
        assert!(stage.shell.take_effects().is_empty(), "nothing copied");

        let band = stage
            .shell
            .chrome
            .band(Divider::Right)
            .expect("beside the chat");
        stage.drag(band, Point::new(band.x - 10.0 * cell.width, band.y));
        let after = stage.chat().area;
        assert_eq!(after.x, chat.x);
        assert_eq!(after.right(), chat.right() - 10, "{chat:?} → {after:?}");
    }

    /// A record shown over the chat.
    fn record_over_the_chat() -> Stage {
        use aura_ui::events::{RecordStatus, RecordSummary};
        use crossterm::event::{KeyCode, KeyEvent};

        let mut stage = stage();
        stage.seed(false);
        stage.shell.commands([UiCommand::NewRecord(RecordSummary {
            seq: 60,
            timestamp: "06:16:19".to_string(),
            full_hash: "355d52f6".repeat(8),
            hash_suffix: String::new(),
            tx_kind: "Policy".to_string(),
            sender: "KERNEL".to_string(),
            message: "approved".to_string(),
            action_count: 0,
            effect_status: "-".to_string(),
            status: RecordStatus::Ok,
            info: String::new(),
            what: String::new(),
            group: String::new(),
            detail: String::new(),
            error_details: String::new(),
            tx_id: "a71e37b3".repeat(8),
            agent_id: String::new(),
            ts_ms: 0,
            policy: None,
        })]);
        stage.draw();
        assert!(!stage.chat().closable, "the chat's own tabs don't close");
        let tui = &mut stage.shell.tabs.front_mut().tui;
        tui.focus_pane(Pane::Records);
        tui.handle_key(KeyEvent::from(KeyCode::Enter));
        stage.draw();
        stage
    }

    #[test]
    fn the_close_box_closes_what_covers_the_chat() {
        let mut stage = record_over_the_chat();
        assert!(stage.chat().closable, "the record over the chat closes");

        let close = stage
            .shell
            .chrome
            .close_box(Pane::Chat)
            .expect("the close box is drawn");
        let close = Point::new(close.center_x(), close.center_y());
        stage.shell.pointer_moved(Point::new(
            close.x,
            close.y + tab_bar::height(&stage.shell.metrics),
        ));
        assert_eq!(stage.shell.cursor_icon(), CursorIcon::Pointer);
        stage.drag(close, close);
        assert!(stage.shell.tabs.front().tui.detail_view().is_none());
        assert!(!stage.chat().closable);
        assert!(stage.chat().focused);
    }

    #[test]
    fn the_expand_box_fills_the_window_with_the_chat_and_restores_it() {
        let mut stage = stage();
        stage.seed(false);
        stage.draw();
        assert_eq!(stage.chat().expand, Some(false));
        let panes = stage.shell.tabs.front().tui.pane_chrome().len();
        assert!(panes > 1, "the other panels are beside the chat");

        let press = |stage: &mut Stage| {
            let expand = stage
                .shell
                .chrome
                .expand_box(Pane::Chat)
                .expect("the expand box is drawn");
            let middle = Point::new(expand.center_x(), expand.center_y());
            stage.shell.pointer_moved(Point::new(
                middle.x,
                middle.y + tab_bar::height(&stage.shell.metrics),
            ));
            assert_eq!(stage.shell.cursor_icon(), CursorIcon::Pointer);
            stage.drag(middle, middle);
            stage.draw();
        };
        press(&mut stage);
        let tui = &stage.shell.tabs.front().tui;
        assert!(tui.chat_expanded());
        assert_eq!(tui.pane_chrome().len(), 1, "only the chat is left");
        assert_eq!(stage.chat().expand, Some(true));

        press(&mut stage);
        let tui = &stage.shell.tabs.front().tui;
        assert!(!tui.chat_expanded());
        assert_eq!(tui.pane_chrome().len(), panes);
    }

    #[test]
    fn the_all_box_switches_the_side_panels_lists_to_the_project() {
        use aura_ui_terminal::SideScope;
        let mut stage = stage();
        stage.seed(false);
        stage.draw();
        let side = |stage: &Stage| {
            let tui = &stage.shell.tabs.front().tui;
            let pane = tui.pane_chrome().iter().find(|c| c.pane == Pane::Side);
            pane.expect("the side panel has a bar").clone()
        };
        assert_eq!(side(&stage).scope, Some(false));

        let all = stage
            .shell
            .chrome
            .scope_box(Pane::Side)
            .expect("the ALL box is drawn");
        let middle = Point::new(all.center_x(), all.center_y());
        stage.shell.pointer_moved(Point::new(
            middle.x,
            middle.y + tab_bar::height(&stage.shell.metrics),
        ));
        assert_eq!(stage.shell.cursor_icon(), CursorIcon::Pointer);
        stage.drag(middle, middle);
        let tui = &stage.shell.tabs.front().tui;
        assert_eq!(tui.side_scope(), SideScope::Project);
        assert_eq!(side(&stage).scope, Some(true));
        assert!(side(&stage).focused);
    }

    #[test]
    fn the_cross_sits_in_the_middle_of_the_close_box() {
        let mut stage = record_over_the_chat();
        // Odd and even boxes, thin and thick strokes.
        for _ in 0..12 {
            cross_is_centered(&mut stage);
            stage.key(ModifiersState::CONTROL, Press::typing("="));
            stage.draw();
        }
    }

    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    fn cross_is_centered(stage: &mut Stage) {
        let mut close = stage.shell.chrome.close_box(Pane::Chat).expect("drawn");
        close.y += tab_bar::height(&stage.shell.metrics);
        let pixels = stage.pixels();
        let width = WINDOW.width as usize;
        let at = |x: usize, y: usize| {
            let i = (y * width + x) * 4;
            pixels[i..i + 3].iter().map(|&c| f32::from(c)).sum::<f32>()
        };
        // Inside the outline, which is a few pixels thick at most.
        let inset = 4.0;
        let xs = (close.x + inset) as usize..(close.x + close.width - inset) as usize;
        let ys = (close.y + inset) as usize..(close.y + close.height - inset) as usize;
        let ground = at(xs.start, ys.start);
        let (mut sum, mut sx, mut sy) = (0.0, 0.0, 0.0);
        for y in ys.clone() {
            for x in xs.clone() {
                let ink = (at(x, y) - ground).abs();
                sum += ink;
                sx += ink * (x as f32 + 0.5);
                sy += ink * (y as f32 + 0.5);
            }
        }
        assert!(sum > 0.0, "the cross is drawn");
        let (cx, cy) = (sx / sum, sy / sum);
        let size = stage.shell.font_size;
        assert!(
            (cx - close.center_x()).abs() < 0.25,
            "{cx} in {close:?} at {size}"
        );
        assert!(
            (cy - close.center_y()).abs() < 0.25,
            "{cy} in {close:?} at {size}"
        );
    }

    #[test]
    fn the_wheel_scrolls_the_chat_by_pixels() {
        let mut stage = stage();
        stage.seed(false);
        stage.shell.commands([UiCommand::StartStreaming]);
        let lines: String = (0..200).map(|n| format!("line {n}\n\n")).collect();
        stage.shell.commands([UiCommand::AppendText(lines)]);
        stage.draw();
        assert!(stage.shell.text().contains("line 199"));
        let (width, height) = (stage.shell.window.width, stage.shell.window.height);
        stage
            .shell
            .pointer_moved(Point::new(width / 2.0, height / 2.0));
        stage.draw();

        // Moving over plain text changes nothing on screen.
        let cell = stage.shell.metrics.cell;
        stage.shell.pointer_moved(Point::new(
            width / 2.0 + 2.0 * cell.width,
            height / 2.0 + cell.height,
        ));
        assert!(
            !stage.shell.dirty(),
            "no frame for a move that shows nothing"
        );

        // A touchpad: a third of a row up from the bottom is the row above
        // drawn two thirds of a row higher.
        let offset = |stage: &Stage| stage.shell.tabs.front().tui.scroll_offset();
        let third = f64::from(cell.height / glide::TOUCHPAD_GAIN) / 3.0;
        let nudge = MouseScrollDelta::PixelDelta(PhysicalPosition::new(0.0, third));
        stage.shell.wheel(nudge, stage.now);
        assert!(stage.shell.dirty());
        assert_eq!(offset(&stage), 1);
        stage.draw();

        // Another third slides the same rows.
        let rows = stage.shell.text();
        stage.shell.wheel(nudge, stage.now);
        assert!(stage.shell.dirty(), "the body slides");
        assert_eq!(offset(&stage), 1);
        let slid = stage.pixels();
        assert_eq!(stage.shell.painter.built(), 0, "painted from kept rows");
        assert_eq!(stage.shell.text(), rows);
        assert!(slid == crate::shell::tests::afresh(&stage.shell));

        // A notch eases out over frames.
        let settle = |stage: &mut Stage| {
            let mut frames = 0;
            while stage.shell.dirty() && frames < 200 {
                stage.now += Duration::from_millis(16);
                stage.draw();
                frames += 1;
            }
            frames
        };
        let notch = |notches| MouseScrollDelta::LineDelta(0.0, notches);
        let before = offset(&stage);
        stage.now += Duration::from_secs(1);
        stage.shell.wheel(notch(4.0), stage.now);
        let frames = settle(&mut stage);
        assert!((4..100).contains(&frames), "{frames} frames");
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let rows = (4.0 * glide::NOTCH_ROWS) as usize;
        let moved = offset(&stage) - before;
        assert!((rows - 1..=rows + 1).contains(&moved), "{moved} rows");
        assert!(!stage.shell.text().contains("line 199"), "scrolled up");
        assert!(stage.pixels() == crate::shell::tests::afresh(&stage.shell));

        // Notches of a spin go further each time.
        let before = offset(&stage);
        stage.now += Duration::from_secs(1);
        for _ in 0..4 {
            stage.shell.wheel(notch(1.0), stage.now);
            stage.now += Duration::from_millis(30);
            stage.draw();
        }
        settle(&mut stage);
        let spun = offset(&stage) - before;
        assert!(
            spun > moved + 2,
            "a spin of 4 went {spun} rows, 4 notches {moved}"
        );

        // Turning back stops what is easing out.
        stage.now += Duration::from_secs(1);
        stage.shell.wheel(notch(4.0), stage.now);
        let before = offset(&stage);
        stage.shell.wheel(notch(-1.0), stage.now);
        settle(&mut stage);
        assert!(offset(&stage) < before, "went back down");

        // Each turn is capped at `MOST_LINES`, so two get back from
        // however far the spin went.
        for _ in 0..2 {
            stage.shell.wheel(notch(-400.0), stage.now);
            settle(&mut stage);
        }
        assert_eq!(offset(&stage), 0, "and back to the bottom, on its rows");
        assert!(stage.shell.text().contains("line 199"));
        assert!(stage.pixels() == crate::shell::tests::afresh(&stage.shell));

        // Not over the tab bar.
        stage.shell.pointer_moved(Point::new(width / 2.0, 4.0));
        stage.draw();
        stage.shell.wheel(notch(30.0), stage.now);
        assert!(!stage.shell.dirty());
    }

    #[test]
    #[allow(clippy::cast_precision_loss)]
    fn the_graph_canvas_drags_by_the_pixel_and_clicks_its_cards() {
        let mut stage = stage();
        stage.seed(false);
        stage.draw();
        let graph = PaneTab::Chat(ChatTab::Graph);
        let tab = stage.shell.chrome.spot(graph).expect("the tab is drawn");
        stage.drag(tab, tab);
        super::super::snapshot::flow(&mut stage);
        stage
            .shell
            .key_pressed(&Press::of(NamedKey::Home), stage.now);
        stage.draw();
        assert_eq!(pan(&stage), (0.0, 0.0));

        // Canvas cell (x, y) on the grid's part of the window.
        let area = stage.shell.tabs.front().tui.flow_canvas().expect("drawn");
        let origin = stage
            .shell
            .metrics
            .origin(stage.shell.grid_area(), stage.shell.cols);
        let cell = stage.shell.metrics.cell;
        let at = |x: f32, y: f32| {
            Point::new(
                origin.x + (f32::from(area.x) + x) * cell.width,
                origin.y + (f32::from(area.y) + y) * cell.height,
            )
        };
        let selected = |stage: &Stage| {
            let flow = stage.shell.tabs.front().tui.flow();
            flow.selected_node().map(|node| node.id.clone())
        };

        // The model call's head line, in the subcard under the turn's.
        let model = stage.shell.tabs.front().tui.flow().layout().card_rect(1);
        let head = at(model.x as f32 + 4.5, model.y as f32 + 1.5);
        stage.drag(head, head);
        assert_eq!(selected(&stage).as_deref(), Some("a1:2"));

        // Zoomed in, the canvas is wider than the view. Empty canvas
        // drags the view with the pointer, by the pixel.
        stage
            .shell
            .tabs
            .front_mut()
            .tui
            .zoom_flow(2.0, Some((0.0, 0.0)));
        stage.draw();
        let zoom = stage.shell.tabs.front().tui.flow().zoom();
        assert_eq!(zoom, 2.0);
        let bar = tab_bar::height(&stage.shell.metrics);
        let on_window = |at: Point| Point::new(at.x, at.y + bar);
        let from = at(6.5, 12.5);
        let before = pan(&stage);
        stage.shell.pointer_moved(on_window(from));
        stage.shell.button(MouseButton::Left, true, stage.now);
        assert_eq!(stage.shell.cursor_icon(), CursorIcon::Default, "not yet");
        stage
            .shell
            .pointer_moved(on_window(Point::new(from.x - 30.0, from.y)));
        assert_eq!(stage.shell.cursor_icon(), CursorIcon::Grabbing);
        stage.shell.button(MouseButton::Left, false, stage.now);
        stage.draw();
        let (x, y) = pan(&stage);
        let step = |pixels: f32| pixels / (cell.width * zoom);
        assert!((x - before.0 - step(30.0)).abs() < 1e-3, "{x}, {y}");
        assert_eq!(y, before.1);
        assert!(x.fract() != 0.0, "a fraction of a cell");
        assert_eq!(
            selected(&stage).as_deref(),
            Some("a1:2"),
            "the drag selects nothing"
        );
        assert!(stage.shell.take_effects().is_empty(), "nor any text");

        // A touchpad pans by what it says.
        let nudge = MouseScrollDelta::PixelDelta(PhysicalPosition::new(-12.0, 0.0));
        stage.shell.wheel(nudge, stage.now);
        stage.draw();
        assert!((pan(&stage).0 - before.0 - step(42.0)).abs() < 1e-3);
    }

    #[test]
    #[allow(clippy::cast_precision_loss)]
    fn the_graph_canvas_zooms_around_the_pointer() {
        let mut stage = stage();
        stage.seed(false);
        stage.draw();
        let graph = PaneTab::Chat(ChatTab::Graph);
        let tab = stage.shell.chrome.spot(graph).expect("the tab is drawn");
        stage.drag(tab, tab);
        super::super::snapshot::flow(&mut stage);
        stage
            .shell
            .key_pressed(&Press::of(NamedKey::Home), stage.now);
        stage.draw();

        let area = stage.shell.tabs.front().tui.flow_canvas().expect("drawn");
        let origin = stage
            .shell
            .metrics
            .origin(stage.shell.grid_area(), stage.shell.cols);
        let cell = stage.shell.metrics.cell;
        let bar = tab_bar::height(&stage.shell.metrics);
        let pointer = Point::new(
            origin.x + (f32::from(area.x) + 12.25) * cell.width,
            origin.y + (f32::from(area.y) + 6.5) * cell.height + bar,
        );
        stage.shell.pointer_moved(pointer);
        let under = |stage: &Stage| {
            let flow = stage.shell.tabs.front().tui.flow();
            let (pan, zoom) = (flow.pan(), flow.zoom());
            (pan.0 + 12.25 / zoom, pan.1 + 6.5 / zoom)
        };
        let before = under(&stage);

        // Ctrl and the wheel zoom in; the point under the pointer stays.
        stage.shell.modifiers = ModifiersState::CONTROL;
        stage
            .shell
            .wheel(MouseScrollDelta::LineDelta(0.0, 3.0), stage.now);
        stage.draw();
        let zoom = stage.shell.tabs.front().tui.flow().zoom();
        assert!((zoom - 1.1f32.powi(3)).abs() < 1e-4, "{zoom}");
        let after = under(&stage);
        assert!(
            (after.0 - before.0).abs() < 1e-3 && (after.1 - before.1).abs() < 1e-3,
            "{before:?} {after:?}"
        );

        // A pinch zooms too, no further out than the least, and around
        // the pointer though that takes the view off the canvas.
        stage.shell.modifiers = ModifiersState::empty();
        for _ in 0..10 {
            stage.shell.pinch(-0.5);
        }
        stage.draw();
        assert_eq!(stage.shell.tabs.front().tui.flow().zoom(), FLOW_MIN_ZOOM);
        let after = under(&stage);
        assert!(
            (after.0 - before.0).abs() < 1e-3 && (after.1 - before.1).abs() < 1e-3,
            "{before:?} {after:?}"
        );
        assert!(pan(&stage).0 < 0.0, "{:?}", pan(&stage));

        // Without Ctrl, the wheel pans.
        stage
            .shell
            .wheel(MouseScrollDelta::LineDelta(0.0, 1.0), stage.now);
        assert_eq!(stage.shell.tabs.front().tui.flow().zoom(), FLOW_MIN_ZOOM);
    }

    fn pan(stage: &Stage) -> (f32, f32) {
        stage.shell.tabs.front().tui.flow().pan()
    }

    #[test]
    #[allow(clippy::cast_precision_loss)]
    fn right_clicking_a_project_opens_its_menu() {
        let mut stage = stage();
        stage.seed(false);
        stage.draw();
        let text = stage.shell.text();
        let row = text
            .lines()
            .position(|line| line.contains("AGENT01"))
            .expect("the project is listed");
        let line = text.lines().nth(row).unwrap_or_default();
        let col = line
            .find("AGENT01")
            .map_or(0, |at| line[..at].chars().count());
        let cell = stage.shell.metrics.cell;
        let origin = stage
            .shell
            .metrics
            .origin(stage.shell.grid_area(), stage.shell.cols);
        let bar = tab_bar::height(&stage.shell.metrics);
        stage.shell.pointer_moved(Point::new(
            origin.x + (col as f32 + 0.5) * cell.width,
            origin.y + (row as f32 + 0.5) * cell.height + bar,
        ));
        stage.shell.button(MouseButton::Right, true, stage.now);
        stage.shell.button(MouseButton::Right, false, stage.now);
        stage.draw();
        let text = stage.shell.text();
        assert!(text.contains("Add agent"), "{text}");
        assert!(text.contains("Remove project…"), "{text}");
    }

    #[test]
    fn dragging_over_text_selects_it_for_the_clipboard() {
        let mut stage = stage();
        stage.seed(false);
        stage.shell.commands([
            UiCommand::StartStreaming,
            UiCommand::AppendText("a reply from the agent".into()),
            UiCommand::FinishStreaming,
            UiCommand::Complete,
        ]);
        stage.draw();
        // The row of the reply, from its first cell to well past its end.
        let text = stage.shell.text();
        let row = text
            .lines()
            .position(|line| line.contains("a reply from"))
            .expect("the reply is drawn");
        let line = text.lines().nth(row).unwrap_or_default();
        let col = line
            .find("a reply")
            .map_or(0, |at| line[..at].chars().count());
        let cell = stage.shell.metrics.cell;
        let origin = stage
            .shell
            .metrics
            .origin(stage.shell.grid_area(), stage.shell.cols);
        #[allow(clippy::cast_precision_loss)]
        let at = |col: usize| {
            Point::new(
                origin.x + (col as f32 + 0.5) * cell.width,
                origin.y + (row as f32 + 0.5) * cell.height,
            )
        };
        stage.drag(at(col), at(col + 40));
        let copied = stage.shell.take_effects();
        assert!(
            matches!(&copied[..], [Effect::Copy(text)] if text.contains("a reply from the agent")),
            "{copied:?}"
        );
    }
}
