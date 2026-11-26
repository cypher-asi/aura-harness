//! Pointer and keys while the menu is open.

use super::{step, ContextMenu, MenuHit};
use crate::app::{App, AppState};
use crossterm::event::{KeyCode, KeyEvent};

impl App {
    /// The pointer is on item `index`: highlight it, and open its submenu
    /// or close another item's.
    fn point_at_menu_item(&mut self, index: usize) {
        let Some(menu) = self.context_menu.as_mut() else {
            return;
        };
        menu.selected = index;
        if menu.highlights_submenu_item() {
            self.open_submenu();
        } else {
            menu.submenu = None;
        }
    }

    /// A pointer event while the menu is open. Returns whether the menu
    /// used it; a press outside closes the menu and is used up too.
    pub(in crate::app) fn context_menu_mouse(
        &mut self,
        kind: crossterm::event::MouseEventKind,
        column: u16,
        row: u16,
    ) -> bool {
        use crossterm::event::{MouseButton, MouseEventKind};
        let Some(hit) = self.context_menu.as_ref().map(|menu| menu.hit(column, row)) else {
            return false;
        };
        match kind {
            MouseEventKind::Moved => {
                if let Some(index) = hit.entry {
                    self.select_submenu_entry(index);
                } else if let Some(index) = hit.item {
                    self.point_at_menu_item(index);
                }
                hit.covers
            }
            MouseEventKind::Down(MouseButton::Left) => {
                self.context_menu_left_press(hit);
                true
            }
            MouseEventKind::Down(_) => {
                if hit.covers {
                    return true;
                }
                self.context_menu = None;
                false
            }
            MouseEventKind::Up(_) | MouseEventKind::Drag(_) => hit.covers,
            _ => {
                self.context_menu = None;
                false
            }
        }
    }

    /// Highlight entry `index` of the open submenu.
    fn select_submenu_entry(&mut self, index: usize) {
        if let Some(submenu) = self.context_menu.as_mut().and_then(|m| m.submenu.as_mut()) {
            submenu.selected = index;
        }
    }

    /// A left press while the menu is open: run what is under it, or
    /// close the menu when the press is outside.
    fn context_menu_left_press(&mut self, hit: MenuHit) {
        if let Some(index) = hit.entry {
            self.select_submenu_entry(index);
            self.run_submenu_entry();
        } else if let Some(index) = hit.item {
            self.point_at_menu_item(index);
            let opens_submenu = self
                .context_menu
                .as_ref()
                .is_some_and(ContextMenu::highlights_submenu_item);
            if !opens_submenu {
                self.run_context_menu_item();
            }
        } else if !hit.covers {
            self.context_menu = None;
        }
    }

    /// Keys while the menu is open. Returns whether the key was used; any
    /// other key closes the menu and goes on to where it would have.
    pub(in crate::app) fn handle_context_menu_key(&mut self, key: KeyEvent) -> bool {
        if !matches!(self.state, AppState::Idle | AppState::Processing) {
            self.context_menu = None;
            return false;
        }
        let Some(menu) = self.context_menu.as_mut() else {
            return false;
        };
        if let Some(submenu) = menu.submenu.as_mut() {
            if let Some(selected) = step(submenu.selected, submenu.entries.len(), key.code) {
                submenu.selected = selected;
                return true;
            }
            match key.code {
                KeyCode::Left | KeyCode::Esc => menu.submenu = None,
                KeyCode::Enter => self.run_submenu_entry(),
                KeyCode::Right => {}
                _ => {
                    self.context_menu = None;
                    return false;
                }
            }
            return true;
        }
        if let Some(selected) = step(menu.selected, menu.items.len(), key.code) {
            menu.selected = selected;
            return true;
        }
        match key.code {
            KeyCode::Enter | KeyCode::Right if menu.highlights_submenu_item() => {
                self.open_submenu();
            }
            KeyCode::Enter => self.run_context_menu_item(),
            KeyCode::Esc => self.context_menu = None,
            _ => {
                self.context_menu = None;
                return false;
            }
        }
        true
    }
}
