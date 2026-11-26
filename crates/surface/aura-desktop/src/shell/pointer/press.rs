//! Presses on the tab bar and on the panels' chrome.

use super::super::{Effect, Shell};
use super::DOUBLE_CLICK;
use crate::chrome;
use crate::tab_bar::Hit;
use std::time::Instant;
use winit::event::MouseButton;

impl Shell {
    /// A button went down on the tab bar: left picks a tab and starts
    /// dragging it along the bar, opens a tab or works a window box,
    /// middle closes the tab under the pointer, and left on empty space
    /// moves the window, or maximizes it when pressed twice.
    pub(super) fn bar_press(&mut self, button: MouseButton, now: Instant) {
        match (self.bar.hover(), button) {
            (Some(Hit::Tab(index)), MouseButton::Left) => {
                self.tabs.focus(index);
                // By id: the `settle` after this press may close tabs.
                self.pointer.tab_drag = self.tabs.id_at(index);
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

    /// A button went down on a panel's title bar, a divider or the
    /// hamburger box: left picks the tab, closes what covers the panel,
    /// expands or restores it, switches its lists between the agent's and
    /// the project's, focuses the panel, starts dragging the divider, or
    /// hides or shows the right column.
    pub(super) fn chrome_press(&mut self, hit: chrome::Hit, button: MouseButton) {
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
            chrome::Hit::SideToggle => tui.toggle_side_column(),
        }
    }
}
