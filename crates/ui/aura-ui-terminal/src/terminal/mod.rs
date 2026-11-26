//! Terminal abstraction for the UI.
//!
//! Provides a high-level interface for terminal operations using ratatui and crossterm.

use crate::{alerts, clipboard, graphics, layout::LayoutMode, renderer, App, Theme};
use crossterm::{
    cursor::{SetCursorStyle, Show},
    event::{
        self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags,
        PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{
        disable_raw_mode, enable_raw_mode, supports_keyboard_enhancement, EnterAlternateScreen,
        LeaveAlternateScreen,
    },
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
/// Longest the screen goes undrawn while nothing on it changes, for what
/// changes with time alone.
const IDLE_FRAME: Duration = Duration::from_secs(1);
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
    /// Whether the terminal speaks the kitty keyboard protocol, so
    /// [`enable_keyboard_enhancement`] can be pushed again after the
    /// editor hands the screen back.
    keyboard_supported: bool,
    /// Whether our keyboard enhancement flags are on the terminal's stack
    /// for the alternate screen, and so must be popped before leaving it.
    keyboard_enhanced: bool,
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
        let keyboard_supported = keyboard_enhancement_supported();
        let keyboard_enhanced = keyboard_supported && enable_keyboard_enhancement(&mut stdout);
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
            keyboard_supported,
            keyboard_enhanced,
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
        let mut drawn: Option<Instant> = None;
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
            let size = (self.width, self.height);
            self.update_size()?;
            let resized = size != (self.width, self.height);

            // Draw what changed: what the agents sent, input, a turning
            // spinner. A screen where nothing moves is drawn now and then
            // all the same.
            let stale = drawn.is_none_or(|at| at.elapsed() >= IDLE_FRAME);
            if app.take_redraw() || resized || stale {
                self.draw(app)?;
                drawn = Some(Instant::now());
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
            app.request_redraw();
        }
    }

    /// Draw `app` as it is, copy what it asked to, and lay its images
    /// over the cells.
    fn draw(&mut self, app: &mut App) -> io::Result<()> {
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
            let wanted: Vec<graphics::Placement<'_>> = placements
                .iter()
                .filter_map(|(slot, rect, preview)| {
                    preview.graphics_png().map(|png| graphics::Placement {
                        image_id: preview.id(),
                        slot: *slot,
                        rect: *rect,
                        png,
                    })
                })
                .collect();
            if let Err(e) = self.graphics.sync(self.inner.backend_mut(), &wanted) {
                warn!(error = %e, "Kitty graphics sync failed");
            }
        }
        Ok(())
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
                let key = legacy_control_key(key);

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
        // The flags would make the editor's keys arrive as CSI u codes it
        // may not read; they belong to the alternate screen we leave.
        self.disable_keyboard_enhancement();
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
        self.keyboard_enhanced =
            self.keyboard_supported && enable_keyboard_enhancement(self.inner.backend_mut());
        self.mouse_capture_enabled = true;
        self.inner.clear()?;
        match status? {
            status if status.success() => Ok(()),
            status => Err(io::Error::other(format!("editor exited with {status}"))),
        }
    }

    /// Pop the keyboard enhancement flags pushed by [`Self::new`], if they
    /// are on. A failure is only logged: the worst outcome is a terminal
    /// that keeps reporting keys as CSI u codes.
    fn disable_keyboard_enhancement(&mut self) {
        if !std::mem::take(&mut self.keyboard_enhanced) {
            return;
        }
        if let Err(e) = execute!(self.inner.backend_mut(), PopKeyboardEnhancementFlags) {
            tracing::debug!(error = %e, "Failed to pop keyboard enhancement flags");
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
        // Always restore terminal state on exit. The keyboard flags go
        // first: each screen keeps its own stack of them.
        self.disable_keyboard_enhancement();
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

/// Whether the terminal answers the kitty keyboard protocol query. Legacy
/// and Windows terminals say no or fail; either way the UI starts with
/// Alt+Enter and Ctrl+J as its line-break keys, so an error is no reason
/// to stop.
fn keyboard_enhancement_supported() -> bool {
    supports_keyboard_enhancement().unwrap_or_else(|e| {
        tracing::debug!(error = %e, "Keyboard enhancement query failed");
        false
    })
}

/// Ask the terminal to report modified keys unambiguously, so Shift+Enter
/// arrives apart from Enter. Only the disambiguation flag is pushed: event
/// types or every key as an escape code would change how plain typing
/// arrives. Returns whether the flags were pushed.
fn enable_keyboard_enhancement(out: &mut impl io::Write) -> bool {
    let flags = PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES);
    match execute!(out, flags) {
        Ok(()) => true,
        Err(e) => {
            tracing::debug!(error = %e, "Keyboard enhancement is not available");
            false
        }
    }
}

/// Report Ctrl+[, Ctrl+M and Ctrl+I as the Esc, Enter and Tab they are
/// without the kitty keyboard protocol. With [`enable_keyboard_enhancement`]
/// on, the terminal sends them as the letter with Control, and the UI's
/// keys are written for the legacy codes. Only plain Control is mapped, so
/// Ctrl+Shift+M and the like stay what they are.
fn legacy_control_key(key: KeyEvent) -> KeyEvent {
    if key.modifiers != KeyModifiers::CONTROL {
        return key;
    }
    let code = match key.code {
        KeyCode::Char('[') => KeyCode::Esc,
        KeyCode::Char('m') => KeyCode::Enter,
        KeyCode::Char('i') => KeyCode::Tab,
        _ => return key,
    };
    KeyEvent {
        code,
        modifiers: KeyModifiers::NONE,
        ..key
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
mod tests;
