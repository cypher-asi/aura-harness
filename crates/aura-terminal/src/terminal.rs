//! Terminal abstraction for the UI.
//!
//! Provides a high-level interface for terminal operations using ratatui and crossterm.

use crate::{clipboard, layout::LayoutMode, renderer, App, Theme};
use crossterm::{
    cursor::{SetCursorStyle, Show},
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Frame, Terminal as RatatuiTerminal};
use std::io::{self, Stdout};
use std::time::Duration;
use tracing::error;

/// Most queued input events handled between two frames, so a flood of
/// input still lets the screen update.
const MAX_EVENTS_PER_FRAME: usize = 256;

/// Terminal wrapper providing high-level UI operations.
pub struct Terminal {
    inner: RatatuiTerminal<CrosstermBackend<Stdout>>,
    theme: Theme,
    width: u16,
    height: u16,
    /// Whether mouse capture is currently enabled
    mouse_capture_enabled: bool,
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
        let mut inner = RatatuiTerminal::new(backend)?;
        inner.clear()?;
        let size = inner.size()?;

        Ok(Self {
            inner,
            theme,
            width: size.width,
            height: size.height,
            mouse_capture_enabled: true,
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
    pub fn run(&mut self, app: &mut App) -> Result<(), crate::TerminalError> {
        loop {
            // Process any pending updates from the command channel FIRST
            // This ensures records loaded at startup are displayed on the first frame
            app.tick();

            // Update size in case of resize
            self.update_size()?;

            // Render current state
            let theme = self.theme.clone();
            let frame = self.inner.draw(|frame| {
                renderer::render(frame, app, &theme);
            })?;
            if let Some(text) = app.take_copy_text(frame.buffer) {
                let result = clipboard::copy(self.inner.backend_mut(), &text);
                app.notify_copied(text.chars().count(), &result);
            }

            // Poll for events with timeout for animations, then take every
            // event already queued before drawing again, so a burst of wheel
            // ticks or key repeats costs one frame instead of one frame each.
            if !event::poll(Duration::from_millis(50))? {
                continue;
            }
            for _ in 0..MAX_EVENTS_PER_FRAME {
                if self.handle_event(app, &event::read()?) {
                    return Ok(());
                }
                // A copy reads the next frame, so draw it before later input
                // can clear the selection.
                if app.copy_pending() || !event::poll(Duration::ZERO)? {
                    break;
                }
            }
        }
    }

    /// Apply one terminal event to `app`. Returns `true` to quit.
    fn handle_event(&mut self, app: &mut App, event: &Event) -> bool {
        match *event {
            Event::Key(key) => {
                // Re-enable mouse capture if it was disabled for text selection
                self.enable_mouse_capture();

                // Only process key press events (not release or repeat)
                // This fixes double-character input on Windows
                if key.kind != KeyEventKind::Press {
                    return false;
                }

                if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    // Ctrl+C: copy the mouse selection, else cancel or quit
                    if app.request_copy() {
                        return false;
                    }
                    if !app.is_processing() {
                        return true;
                    }
                    app.cancel();
                    false
                } else if key.code == KeyCode::Char('d')
                    && key.modifiers.contains(KeyModifiers::CONTROL)
                {
                    // Ctrl+D: quit
                    true
                } else {
                    app.handle_key(key).should_quit()
                }
            }
            Event::Resize(width, height) => {
                self.width = width;
                self.height = height;
                app.clear_selection();
                false
            }
            Event::Mouse(mouse) => {
                // When Shift is held, disable mouse capture to allow terminal's
                // native text selection (Shift+click+drag to select, auto-copies
                // to clipboard in most terminal emulators like Windows Terminal)
                if mouse.modifiers.contains(KeyModifiers::SHIFT) {
                    if self.mouse_capture_enabled {
                        let _ = execute!(self.inner.backend_mut(), DisableMouseCapture);
                        self.mouse_capture_enabled = false;
                    }
                    return false;
                }

                // Re-enable mouse capture if it was disabled for selection
                self.enable_mouse_capture();
                app.handle_mouse(mouse);
                false
            }
            _ => false,
        }
    }

    fn enable_mouse_capture(&mut self) {
        if !self.mouse_capture_enabled {
            let _ = execute!(self.inner.backend_mut(), EnableMouseCapture);
            self.mouse_capture_enabled = true;
        }
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
            error!(error = %e, "Failed to disable raw mode");
        }
        if let Err(e) = execute!(
            self.inner.backend_mut(),
            DisableMouseCapture,
            LeaveAlternateScreen
        ) {
            error!(error = %e, "Failed to leave alternate screen");
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
