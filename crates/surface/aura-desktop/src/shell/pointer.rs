//! The pointer: over the grid it is the terminal UI's mouse, cell by
//! cell; on the panels' title bars and the borders between them it picks
//! tabs, focuses panels and drags; on the tab bar it works the tabs and
//! the window, which has no frame from the system: empty bar space moves
//! it and its edges resize it.

use super::{Effect, Shell};
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
const MOST_LINES: f32 = 200.0;

/// Where the pointer is and what it is doing.
#[derive(Default)]
pub struct Pointer {
    /// The grid cell under it
    cell: Option<Position>,
    /// Where it is on the grid's part of the window, while over it
    pub(super) on_grid: Option<Point>,
    /// Where it is while over the tab bar
    pub(super) on_bar: Option<Point>,
    /// The divider being dragged
    dragging: Option<Divider>,
    /// The window side or corner under it
    edge: Option<ResizeDirection>,
    /// When empty tab bar space was last pressed, for a double click
    last_bar_press: Option<Instant>,
    left_down: bool,
    /// Scroll distance not yet turned into whole lines
    scroll: f32,
}

impl Shell {
    /// The pointer moved to `at`, in the window.
    pub fn pointer_moved(&mut self, at: Point) {
        let started = Instant::now();
        let hovers = (self.bar.hover(), self.chrome.hover());
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
        if !pointer.left_down {
            self.chrome.point(Some(in_grid));
        }
        self.dirty |= hovers != (self.bar.hover(), self.chrome.hover());
        let cell = self
            .metrics
            .cell_at(self.grid_area(), self.cols, self.rows, in_grid);
        if self.pointer.cell == Some(cell) {
            return;
        }
        self.pointer.cell = Some(cell);
        self.send_mouse(if self.pointer.left_down {
            MouseEventKind::Drag(Button::Left)
        } else {
            MouseEventKind::Moved
        });
        self.settle(started);
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
            let Some(button) = mouse_button(button) else {
                return;
            };
            self.pointer.left_down |= button == Button::Left;
            MouseEventKind::Down(button)
        } else {
            if button == MouseButton::Left && self.pointer.dragging.take().is_some() {
                self.chrome.point(self.pointer.on_grid);
                self.dirty = true;
                return;
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
    pub fn wheel(&mut self, delta: MouseScrollDelta) {
        if self.pointer.on_bar.is_some() {
            return;
        }
        let started = Instant::now();
        #[allow(clippy::cast_possible_truncation)]
        let lines = match delta {
            MouseScrollDelta::LineDelta(_, y) => y,
            MouseScrollDelta::PixelDelta(by) => by.y as f32 / self.metrics.cell.height,
        };
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
        if let Some(edge) = self.pointer.edge {
            resize_cursor(edge)
        } else if let Some(divider) = divider {
            if divider.sideways() {
                CursorIcon::EwResize
            } else {
                CursorIcon::NsResize
            }
        } else if matches!(hover, Some(chrome::Hit::Tab(_))) {
            CursorIcon::Pointer
        } else {
            CursorIcon::Default
        }
    }

    /// A button went down on the tab bar: left picks or opens a tab or
    /// works a window box, middle closes the tab under the pointer, and
    /// left on empty space moves the window, or maximizes it when pressed
    /// twice.
    fn bar_press(&mut self, button: MouseButton, now: Instant) {
        match (self.bar.hover(), button) {
            (Some(Hit::Tab(index)), MouseButton::Left) => self.tabs.focus(index),
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
    /// the tab, focuses the panel or starts dragging the divider.
    fn chrome_press(&mut self, hit: chrome::Hit, button: MouseButton) {
        if button != MouseButton::Left {
            return;
        }
        let tui = &mut self.tabs.front_mut().tui;
        match hit {
            chrome::Hit::Tab(tab) => tui.select_pane_tab(tab),
            chrome::Hit::Bar(pane) => tui.focus_pane(pane),
            chrome::Hit::Divider(divider) => self.pointer.dragging = Some(divider),
        }
    }

    fn send_mouse(&mut self, kind: MouseEventKind) {
        let Some(at) = self.pointer.cell else {
            return;
        };
        let modifiers = super::keys::modifiers(self.modifiers);
        self.tabs.front_mut().tui.handle_mouse(MouseEvent {
            kind,
            column: at.x,
            row: at.y,
            modifiers,
        });
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
    use crate::shell::WINDOW;
    use aura_ui::events::UiCommand;
    use aura_ui_terminal::{ChatTab, Pane, PaneChrome, PaneTab};
    use std::time::Duration;
    use winit::dpi::PhysicalPosition;

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
        assert_eq!(stage.asked(), ["CreateAgent(None)"]);
        assert_eq!(stage.shell.tabs.len(), 1, "the tab opens with its agent");
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
    fn the_wheel_scrolls_the_chat_by_whole_lines() {
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

        // A touchpad: a third of a line at a time moves nothing twice.
        let third = f64::from(stage.shell.metrics.cell.height) / 3.0;
        let nudge = MouseScrollDelta::PixelDelta(PhysicalPosition::new(0.0, third + 0.01));
        stage.shell.wheel(nudge);
        stage.shell.wheel(nudge);
        assert!(!stage.shell.dirty());
        stage.shell.wheel(nudge);
        assert!(stage.shell.dirty(), "the third makes a line");
        stage.draw();

        stage.shell.wheel(MouseScrollDelta::LineDelta(0.0, 30.0));
        stage.draw();
        assert!(!stage.shell.text().contains("line 199"), "scrolled up");
        stage.shell.wheel(MouseScrollDelta::LineDelta(0.0, -400.0));
        stage.draw();
        assert!(stage.shell.text().contains("line 199"), "and back down");

        // Not over the tab bar.
        stage.shell.pointer_moved(Point::new(width / 2.0, 4.0));
        stage.draw();
        stage.shell.wheel(MouseScrollDelta::LineDelta(0.0, 30.0));
        assert!(!stage.shell.dirty());
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
        let origin =
            stage
                .shell
                .metrics
                .origin(stage.shell.grid_area(), stage.shell.cols, stage.shell.rows);
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
