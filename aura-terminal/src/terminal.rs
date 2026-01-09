//! Terminal abstraction for the UI.
//!
//! Provides a high-level interface for terminal operations using ratatui and crossterm.

use crate::{layout::LayoutMode, renderer, App, Theme};
use crossterm::{
    cursor::{SetCursorStyle, Show},
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers, MouseEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Frame, Terminal as RatatuiTerminal};
use std::io::{self, Stdout};
use std::time::Duration;
use tracing::error;

/// Terminal wrapper providing high-level UI operations.
pub struct Terminal {
    inner: RatatuiTerminal<CrosstermBackend<Stdout>>,
    theme: Theme,
    width: u16,
    height: u16,
}

impl Terminal {
    /// Create a new terminal with the given theme.
    ///
    /// # Errors
    ///
    /// Returns error if terminal initialization fails.
    pub fn new(theme: Theme) -> io::Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(
            stdout,
            EnterAlternateScreen,
            EnableMouseCapture,
            Show,
            SetCursorStyle::BlinkingBar
        )?;
        let backend = CrosstermBackend::new(stdout);
        let inner = RatatuiTerminal::new(backend)?;
        let size = inner.size()?;

        Ok(Self {
            inner,
            theme,
            width: size.width,
            height: size.height,
        })
    }

    /// Get the current layout mode based on terminal width.
    #[must_use]
    pub const fn layout_mode(&self) -> LayoutMode {
        LayoutMode::from_width(self.width)
    }

    /// Get the current theme.
    #[must_use]
    pub const fn theme(&self) -> &Theme {
        &self.theme
    }

    /// Set a new theme.
    pub fn set_theme(&mut self, theme: Theme) {
        self.theme = theme;
    }

    /// Get the terminal width.
    #[must_use]
    pub const fn width(&self) -> u16 {
        self.width
    }

    /// Get the terminal height.
    #[must_use]
    pub const fn height(&self) -> u16 {
        self.height
    }

    /// Update terminal size from actual dimensions.
    fn update_size(&mut self) -> io::Result<()> {
        let size = self.inner.size()?;
        self.width = size.width;
        self.height = size.height;
        Ok(())
    }

    /// Run the main event loop.
    ///
    /// # Errors
    ///
    /// Returns error if rendering or event handling fails.
    pub fn run(&mut self, app: &mut App) -> anyhow::Result<()> {
        loop {
            // Update size in case of resize
            self.update_size()?;

            // Render current state
            let theme = self.theme.clone();
            self.inner.draw(|frame| {
                renderer::render(frame, app, &theme);
            })?;

            // Poll for events with timeout for animations
            if event::poll(Duration::from_millis(50))? {
                match event::read()? {
                    Event::Key(key) => {
                        // Only process key press events (not release or repeat)
                        // This fixes double-character input on Windows
                        if key.kind != KeyEventKind::Press {
                            continue;
                        }

                        if key.code == KeyCode::Char('c')
                            && key.modifiers.contains(KeyModifiers::CONTROL)
                        {
                            // Ctrl+C: cancel or quit
                            if app.is_processing() {
                                app.cancel();
                            } else {
                                break;
                            }
                        } else if key.code == KeyCode::Char('d')
                            && key.modifiers.contains(KeyModifiers::CONTROL)
                        {
                            // Ctrl+D: quit
                            break;
                        } else if app.handle_key(key).should_quit() {
                            break;
                        }
                    }
                    Event::Resize(width, height) => {
                        self.width = width;
                        self.height = height;
                    }
                    Event::Mouse(mouse) => {
                        // When Shift is held, don't handle mouse events - let them
                        // pass through to the terminal for native text selection/copy
                        if mouse.modifiers.contains(KeyModifiers::SHIFT) {
                            continue;
                        }
                        
                        match mouse.kind {
                            MouseEventKind::ScrollUp => {
                                app.scroll_up(3);
                            }
                            MouseEventKind::ScrollDown => {
                                app.scroll_down(3);
                            }
                            _ => {}
                        }
                    }
                    _ => {}
                }
            }

            // Process any pending updates from the command channel
            app.tick();
        }

        Ok(())
    }

    /// Render a single frame (for testing or custom loops).
    ///
    /// # Errors
    ///
    /// Returns error if rendering fails.
    pub fn render<F>(&mut self, f: F) -> io::Result<()>
    where
        F: FnOnce(&mut Frame, &Theme),
    {
        let theme = self.theme.clone();
        self.inner.draw(|frame| f(frame, &theme))?;
        Ok(())
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        // Always restore terminal state on exit
        if let Err(e) = disable_raw_mode() {
            error!("Failed to disable raw mode: {}", e);
        }
        if let Err(e) = execute!(self.inner.backend_mut(), DisableMouseCapture, LeaveAlternateScreen) {
            error!("Failed to leave alternate screen: {}", e);
        }
    }
}

/// Result of handling a key event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyResult {
    /// Whether the app should quit
    quit: bool,
}

impl KeyResult {
    /// Create a result indicating continue.
    #[must_use]
    pub const fn continue_running() -> Self {
        Self { quit: false }
    }

    /// Create a result indicating quit.
    #[must_use]
    pub const fn quit() -> Self {
        Self { quit: true }
    }

    /// Check if this result indicates quitting.
    #[must_use]
    pub const fn should_quit(self) -> bool {
        self.quit
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_key_result() {
        let result = KeyResult::continue_running();
        assert!(!result.should_quit());

        let result = KeyResult::quit();
        assert!(result.should_quit());
    }
}
