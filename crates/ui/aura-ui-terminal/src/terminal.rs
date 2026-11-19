//! Terminal abstraction for the UI.
//!
//! Provides a high-level interface for terminal operations using ratatui and crossterm.

use crate::{alerts, clipboard, graphics, layout::LayoutMode, renderer, App, Theme};
use crossterm::{
    cursor::{SetCursorStyle, Show},
    event::{
        self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event, KeyCode, KeyEventKind, KeyModifiers,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Frame, Terminal as RatatuiTerminal};
use std::io::{self, Stdout, Write};
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};
use tracing::{error, warn};

/// Most queued input events handled between two frames, so a flood of
/// input still lets the screen update.
const MAX_EVENTS_PER_FRAME: usize = 256;
/// Time between frames without input; spinners advance once a frame.
const FRAME: Duration = Duration::from_millis(50);
/// Longest wait for input before taking the agents' commands again.
const COMMAND_POLL: Duration = Duration::from_millis(5);

/// Terminal wrapper providing high-level UI operations.
pub struct Terminal {
    inner: RatatuiTerminal<CrosstermBackend<Stdout>>,
    theme: Theme,
    width: u16,
    height: u16,
    /// Whether mouse capture is currently enabled
    mouse_capture_enabled: bool,
    /// Kitty graphics protocol state; a no-op when the terminal doesn't
    /// support it.
    graphics: graphics::GraphicsState,
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
        enable_bracketed_paste(&mut stdout);
        let backend = CrosstermBackend::new(stdout);
        let mut inner = RatatuiTerminal::new(backend)?;
        inner.clear()?;
        let size = inner.size()?;
        let protocol = graphics::detect(|name| std::env::var(name).ok());

        Ok(Self {
            inner,
            theme,
            width: size.width,
            height: size.height,
            mouse_capture_enabled: true,
            graphics: graphics::GraphicsState::new(protocol),
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
            self.raise_alerts(app);

            if let Some(path) = app.take_editor_request() {
                let result = self.edit_file(&path);
                app.editor_closed(path, &result);
                continue;
            }
            if app.take_paste_request() {
                app.paste_clipboard(clipboard::read());
            }

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
            // Overlay Kitty graphics-protocol images over the cells ratatui
            // just painted, if the terminal supports it.  The half-block
            // rendering already in the buffer acts as an automatic fallback
            // for terminals that silently ignore APC sequences.
            if self.graphics.protocol().is_kitty() {
                let placements = app.visible_image_placements();
                let triples: Vec<(u32, ratatui::layout::Rect, &[u8])> = placements
                    .iter()
                    .filter_map(|(id, rect, preview)| {
                        preview.graphics_png().map(|png| (*id, *rect, png))
                    })
                    .collect();
                if let Err(e) = self.graphics.sync(self.inner.backend_mut(), &triples) {
                    warn!(error = %e, "Kitty graphics sync failed");
                }
            }

            // Wait for input until the next animation frame, then take every
            // event already queued before drawing again, so a burst of wheel
            // ticks or key repeats costs one frame instead of one frame each.
            if !wait_for_input(app)? {
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

    /// Ring the bell once for everything `app` wants the user to notice,
    /// and show each as a desktop notification when those are on.
    fn raise_alerts(&mut self, app: &mut App) {
        let alerts = app.take_alerts();
        if alerts.is_empty() {
            return;
        }
        let settings = *app.alert_settings();
        if settings.bell {
            let backend = self.inner.backend_mut();
            if let Err(e) = backend.write_all(b"\x07").and_then(|()| backend.flush()) {
                warn!(error = %e, "Failed to ring the terminal bell");
            }
        }
        if settings.desktop {
            for alert in &alerts {
                alerts::desktop_notify(alert);
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
                    if !app.is_busy() {
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
            Event::Paste(ref text) => {
                app.handle_paste(text);
                false
            }
            _ => false,
        }
    }

    /// Hand the screen to the user's editor (`$VISUAL`, then `$EDITOR`,
    /// then `vi`) on `path` and take it back once the editor exits.
    fn edit_file(&mut self, path: &Path) -> io::Result<()> {
        disable_raw_mode()?;
        let _ = execute!(self.inner.backend_mut(), DisableBracketedPaste);
        execute!(
            self.inner.backend_mut(),
            DisableMouseCapture,
            LeaveAlternateScreen
        )?;
        // No timeout: the editor is interactive and runs until the user quits it.
        let status = editor_command(path).status();
        enable_raw_mode()?;
        execute!(
            self.inner.backend_mut(),
            EnterAlternateScreen,
            EnableMouseCapture
        )?;
        enable_bracketed_paste(self.inner.backend_mut());
        self.mouse_capture_enabled = true;
        self.inner.clear()?;
        match status? {
            status if status.success() => Ok(()),
            status => Err(io::Error::other(format!("editor exited with {status}"))),
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
        let _ = execute!(self.inner.backend_mut(), DisableBracketedPaste);
        if let Err(e) = execute!(
            self.inner.backend_mut(),
            DisableMouseCapture,
            LeaveAlternateScreen
        ) {
            error!(error = %e, "Failed to leave alternate screen");
        }
    }
}

/// Wait up to one [`FRAME`] for input, applying the agents' commands as
/// they arrive. Returns whether input is waiting.
fn wait_for_input(app: &mut App) -> io::Result<bool> {
    let deadline = Instant::now() + FRAME;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Ok(false);
        }
        if event::poll(left.min(COMMAND_POLL))? {
            return Ok(true);
        }
        app.receive_commands();
    }
}

/// Have the terminal wrap pastes so they arrive as one [`Event::Paste`]:
/// a pasted image path can be attached, and pasted newlines no longer send
/// the message line by line. Terminals without it paste as typed keys.
fn enable_bracketed_paste(out: &mut impl io::Write) {
    if let Err(e) = execute!(out, EnableBracketedPaste) {
        tracing::debug!(error = %e, "Bracketed paste is not available");
    }
}

/// Command that opens `path` in the user's editor. The editor variable may
/// carry arguments, e.g. `code --wait`.
fn editor_command(path: &Path) -> Command {
    let editor = ["VISUAL", "EDITOR"]
        .iter()
        .filter_map(|var| std::env::var(var).ok())
        .find(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "vi".to_string());
    let mut parts = editor.split_whitespace();
    let mut command = Command::new(parts.next().unwrap_or("vi"));
    command.args(parts).arg(path);
    command
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
