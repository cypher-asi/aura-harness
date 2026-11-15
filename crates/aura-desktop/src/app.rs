//! The window: hosts the terminal UI (`aura_terminal::App`) and draws it
//! on a GPU cell grid instead of a terminal.
//!
//! The loop mirrors `aura_terminal::Terminal`: tick every 50 ms (the UI's
//! animations count ticks), apply agent commands, render the frame, then
//! copy selections and hand files to an editor when the UI asks.

use crate::grid::{Grid, GridBackend, Metrics, Palette, Screen};
use crate::perf::Perf;
use crate::{input, theme};
use aura_ui_core::events::{UiCommand, UiEvent};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use futures::Stream;
use iced::keyboard::{self, key::Named, Key, Modifiers};
use iced::widget::canvas;
use iced::{event, mouse, window, Element, Length, Point, Size, Subscription, Task, Theme};
use ratatui::layout::Position;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};
use tokio::sync::mpsc;

/// Most commands applied in one update.
const MAX_BATCH: usize = 1024;
/// The terminal UI's tick; its spinners advance one frame per tick.
const TICK: Duration = Duration::from_millis(50);
/// Half a cursor blink.
const BLINK: Duration = Duration::from_millis(530);
/// How often a file opened in an external editor is checked for a save.
const EDIT_POLL: Duration = Duration::from_secs(1);
const WINDOW: Size = Size::new(1320.0, 860.0);

/// The channel pair connecting the UI to an agent session.
pub struct Handles {
    pub events: mpsc::Sender<UiEvent>,
    pub commands: mpsc::Receiver<UiCommand>,
}

/// Startup choices from the command line.
pub struct Options {
    pub theme: String,
    pub verbose: bool,
}

#[derive(Debug, Clone)]
pub enum Message {
    Commands(Vec<UiCommand>),
    SessionEnded,
    Tick(Instant),
    Key(keyboard::Event),
    Mouse(mouse::Event),
    Resized(Size),
    Focused(bool),
    Copied(usize),
    EditorClosed(PathBuf, Result<(), String>),
}

/// A file the user is editing in an app that doesn't report closing; it
/// counts as done once it's saved.
struct Watch {
    path: PathBuf,
    modified: Option<SystemTime>,
    checked: Instant,
}

pub struct App {
    tui: aura_terminal::App,
    tui_theme: aura_terminal::Theme,
    terminal: ratatui::Terminal<GridBackend>,
    screen: Screen,
    metrics: Metrics,
    palette: Palette,
    window: Size,
    modifiers: Modifiers,
    pointer: Option<Position>,
    left_down: bool,
    /// Scroll distance not yet turned into whole lines.
    scroll: f32,
    focused: bool,
    blink_from: Instant,
    now: Instant,
    watch: Option<Watch>,
    perf: Perf,
}

pub fn run(handles: Handles, options: Options, launched: Instant) -> iced::Result {
    let slot = Mutex::new(Some((handles, options)));
    let mut app = iced::application(
        move || {
            let (handles, options) = slot
                .lock()
                .ok()
                .and_then(|mut slot| slot.take())
                .expect("the application boots once");
            App::boot(handles, &options, launched)
        },
        App::update,
        App::view,
    )
    .title("AURA")
    .subscription(App::subscription)
    .theme(App::theme)
    .default_font(theme::mono())
    .antialiasing(true)
    .window_size(WINDOW);
    for font in theme::font_files() {
        app = app.font(font);
    }
    app.run()
}

impl App {
    pub(crate) fn boot(
        handles: Handles,
        options: &Options,
        launched: Instant,
    ) -> (Self, Task<Message>) {
        let mut tui = aura_terminal::App::new()
            .with_event_sender(handles.events)
            .with_alert_settings(aura_terminal::AlertSettings::from_env());
        if options.verbose {
            tui.set_verbose(true);
        }
        let tui_theme = aura_terminal::Theme::by_name(&options.theme);
        let now = Instant::now();
        let mut app = Self {
            palette: Palette::from_theme(&tui_theme),
            tui,
            tui_theme,
            terminal: ratatui::Terminal::new(GridBackend::new(1, 1))
                .expect("the grid backend never fails"),
            screen: Screen::default(),
            metrics: Metrics::new(theme::initial_font_size()),
            window: WINDOW,
            modifiers: Modifiers::empty(),
            pointer: None,
            left_down: false,
            scroll: 0.0,
            focused: true,
            blink_from: now,
            now,
            watch: None,
            perf: Perf::new(launched),
        };
        let first = app.refresh();
        let commands = Task::run(command_batches(handles.commands), |m| m);
        (app, Task::batch([commands, first]))
    }

    fn theme(&self) -> Theme {
        Theme::custom(
            "AURA".to_string(),
            iced::theme::Palette {
                background: self.palette.bg,
                text: self.palette.fg,
                ..Theme::Dark.palette()
            },
        )
    }

    fn subscription(&self) -> Subscription<Message> {
        Subscription::batch([
            keyboard::listen().map(Message::Key),
            event::listen_with(window_event),
            iced::time::every(TICK).map(Message::Tick),
        ])
    }

    pub(crate) fn view(&self) -> Element<'_, Message> {
        let started = Instant::now();
        let element = canvas(Grid {
            screen: &self.screen,
            metrics: self.metrics,
            palette: self.palette,
            cursor_on: self.cursor_on(),
        })
        .width(Length::Fill)
        .height(Length::Fill)
        .into();
        self.perf.view(started.elapsed());
        element
    }

    pub(crate) fn update(&mut self, message: Message) -> Task<Message> {
        let started = Instant::now();
        let task = self.handle(message);
        let task = Task::batch([task, self.refresh()]);
        self.perf.update(started.elapsed());
        task
    }

    /// The cursor shows while typing and then blinks; it stays solid when
    /// the window isn't focused so it's easy to find.
    fn cursor_on(&self) -> bool {
        if !self.focused {
            return true;
        }
        let phase = self
            .now
            .saturating_duration_since(self.blink_from)
            .as_millis()
            / BLINK.as_millis();
        phase.is_multiple_of(2)
    }

    fn handle(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Commands(batch) => {
                for cmd in batch {
                    self.tui.process_command(cmd);
                }
            }
            Message::SessionEnded => {
                tracing::warn!("the agent session closed its command channel");
            }
            Message::Tick(now) => {
                self.now = now;
                self.perf.frame(now);
                self.tui.tick();
                self.poll_watch(now);
                return self.raise_alerts();
            }
            Message::Key(event) => return self.key(event),
            Message::Mouse(event) => self.mouse(event),
            Message::Resized(size) => {
                self.window = size;
                self.tui.clear_selection();
            }
            Message::Focused(focused) => {
                self.focused = focused;
                self.restart_blink();
            }
            Message::Copied(chars) => self.tui.notify_copied(chars, &Ok(())),
            Message::EditorClosed(path, result) => {
                self.tui
                    .editor_closed(path, &result.map_err(std::io::Error::other));
            }
        }
        Task::none()
    }

    fn restart_blink(&mut self) {
        self.now = Instant::now();
        self.blink_from = self.now;
    }

    fn key(&mut self, event: keyboard::Event) -> Task<Message> {
        match event {
            keyboard::Event::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers;
                Task::none()
            }
            keyboard::Event::KeyPressed {
                key,
                modified_key,
                modifiers,
                text,
                ..
            } => {
                self.modifiers = modifiers;
                self.restart_blink();
                if let Some(task) = self.shortcut(&modified_key, modifiers) {
                    return task;
                }
                for event in input::key_events(&key, text.as_deref(), modifiers) {
                    if self.key_press(event) {
                        return iced::exit();
                    }
                }
                Task::none()
            }
            keyboard::Event::KeyReleased { .. } => Task::none(),
        }
    }

    /// Window shortcuts a terminal emulator would handle itself: zoom,
    /// copy and paste.
    fn shortcut(&mut self, key: &Key, m: Modifiers) -> Option<Task<Message>> {
        let chord = if cfg!(target_os = "macos") {
            m.logo()
        } else {
            m.control()
        };
        if !chord {
            if m.shift() && matches!(key.as_ref(), Key::Named(Named::Insert)) {
                self.tui.paste_from_clipboard();
                return Some(Task::none());
            }
            return None;
        }
        let size = self.metrics.font_size;
        let zoom = |size: f32| Some(size.clamp(theme::MIN_FONT_SIZE, theme::MAX_FONT_SIZE));
        let new_size = match key.as_ref() {
            Key::Character("=" | "+") => zoom(size + 1.0),
            Key::Character("-" | "_") => zoom(size - 1.0),
            Key::Character("0" | ")") => Some(theme::DEFAULT_FONT_SIZE),
            // Plain Ctrl+V reaches the UI, which pastes (images too) itself.
            Key::Character("v" | "V") if m.shift() || cfg!(target_os = "macos") => {
                self.tui.paste_from_clipboard();
                return Some(Task::none());
            }
            // Ctrl+C on its own interrupts the agent, as in a terminal.
            Key::Character("c" | "C") if m.shift() || cfg!(target_os = "macos") => {
                self.tui.request_copy();
                return Some(Task::none());
            }
            _ => None,
        }?;
        if (new_size - size).abs() > f32::EPSILON {
            self.metrics = Metrics::new(new_size);
            self.screen.invalidate();
            self.tui.clear_selection();
        }
        Some(Task::none())
    }

    /// One key, with the terminal's Ctrl+C and Ctrl+D rules. `true` quits.
    fn key_press(&mut self, key: KeyEvent) -> bool {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('c') => {
                    if self.tui.request_copy() {
                        return false;
                    }
                    if !self.tui.is_busy() {
                        return true;
                    }
                    self.tui.cancel();
                    return false;
                }
                KeyCode::Char('d') => return true,
                _ => {}
            }
        }
        self.tui.handle_key(key).should_quit()
    }

    fn mouse(&mut self, event: mouse::Event) {
        let (cols, rows) = self.screen.size();
        let cell_at =
            |metrics: &Metrics, window: Size, at: Point| metrics.cell_at(window, cols, rows, at);
        let kind = match event {
            mouse::Event::CursorMoved { position } => {
                let at = cell_at(&self.metrics, self.window, position);
                if self.pointer == Some(at) {
                    return;
                }
                self.pointer = Some(at);
                if self.left_down {
                    MouseEventKind::Drag(MouseButton::Left)
                } else {
                    MouseEventKind::Moved
                }
            }
            mouse::Event::CursorLeft => {
                self.pointer = None;
                return;
            }
            mouse::Event::ButtonPressed(button) => {
                let Some(button) = mouse_button(button) else {
                    return;
                };
                self.left_down |= button == MouseButton::Left;
                MouseEventKind::Down(button)
            }
            mouse::Event::ButtonReleased(button) => {
                let Some(button) = mouse_button(button) else {
                    return;
                };
                if button == MouseButton::Left {
                    self.left_down = false;
                }
                MouseEventKind::Up(button)
            }
            mouse::Event::WheelScrolled { delta } => {
                let lines = match delta {
                    mouse::ScrollDelta::Lines { y, .. } => y,
                    mouse::ScrollDelta::Pixels { y, .. } => y / self.metrics.cell.height,
                };
                self.scroll += lines;
                while self.scroll.abs() >= 1.0 {
                    let kind = if self.scroll > 0.0 {
                        self.scroll -= 1.0;
                        MouseEventKind::ScrollUp
                    } else {
                        self.scroll += 1.0;
                        MouseEventKind::ScrollDown
                    };
                    self.send_mouse(kind);
                }
                return;
            }
            mouse::Event::CursorEntered => return,
        };
        self.send_mouse(kind);
    }

    fn send_mouse(&mut self, kind: MouseEventKind) {
        let Some(at) = self.pointer else {
            return;
        };
        self.tui.handle_mouse(MouseEvent {
            kind,
            column: at.x,
            row: at.y,
            modifiers: input::modifiers(self.modifiers),
        });
    }

    /// Ring for what the UI wants noticed: a desktop notification when
    /// those are on, and the window asks for attention in place of the
    /// terminal bell.
    fn raise_alerts(&mut self) -> Task<Message> {
        let alerts = self.tui.take_alerts();
        if alerts.is_empty() {
            return Task::none();
        }
        let settings = *self.tui.alert_settings();
        if settings.desktop {
            for alert in &alerts {
                aura_terminal::alerts::desktop_notify(alert);
            }
        }
        if !settings.bell {
            return Task::none();
        }
        window::latest().and_then(|id| {
            window::request_user_attention(id, Some(window::UserAttention::Informational))
        })
    }

    /// Render the UI into the grid, then act on what the frame asked for.
    fn refresh(&mut self) -> Task<Message> {
        if self.tui.take_paste_request() {
            self.tui.paste_from_clipboard();
        }
        let (cols, rows) = self.metrics.grid(self.window);
        let area = self.terminal.backend().buffer().area;
        if (area.width, area.height) != (cols, rows) {
            let pixels = Size::new(
                f32::from(cols) * self.metrics.cell.width,
                f32::from(rows) * self.metrics.cell.height,
            );
            self.terminal.backend_mut().resize(cols, rows, pixels);
        }
        let (tui, tui_theme) = (&mut self.tui, &self.tui_theme);
        let mut tasks = Vec::new();
        match self
            .terminal
            .draw(|frame| aura_terminal::render(frame, tui, tui_theme))
        {
            Ok(frame) => {
                if let Some(text) = tui.take_copy_text(frame.buffer) {
                    let chars = text.chars().count();
                    tasks.push(
                        iced::clipboard::write(text).chain(Task::done(Message::Copied(chars))),
                    );
                }
            }
            Err(e) => tracing::warn!(error = %e, "render failed"),
        }
        self.screen.update(self.terminal.backend());
        if let Some(path) = self.tui.take_editor_request() {
            tasks.push(self.open_editor(path));
        }
        Task::batch(tasks)
    }

    /// Hand a file to the user's editor. Editors that need a terminal can't
    /// run in a window, so those (and no editor at all) fall back to the
    /// system's default app, and saving the file counts as closing it.
    fn open_editor(&mut self, path: PathBuf) -> Task<Message> {
        let editor = std::env::var("VISUAL")
            .or_else(|_| std::env::var("EDITOR"))
            .ok()
            .filter(|e| !e.trim().is_empty() && !is_terminal_editor(e));
        let Some(editor) = editor else {
            open_external(&path.display().to_string());
            self.watch = Some(Watch {
                modified: modified(&path),
                path,
                checked: self.now,
            });
            return Task::none();
        };
        let target = path.clone();
        Task::perform(
            async move {
                let mut words = editor.split_whitespace();
                let program = words.next().unwrap_or_default().to_string();
                let status = tokio::process::Command::new(program)
                    .args(words)
                    .arg(&target)
                    .status()
                    .await
                    .map_err(|e| e.to_string())?;
                if status.success() {
                    Ok(())
                } else {
                    Err(format!("editor exited with {status}"))
                }
            },
            move |result| Message::EditorClosed(path.clone(), result),
        )
    }

    fn poll_watch(&mut self, now: Instant) {
        let Some(watch) = self.watch.as_mut() else {
            return;
        };
        if now.saturating_duration_since(watch.checked) < EDIT_POLL {
            return;
        }
        watch.checked = now;
        if modified(&watch.path) != watch.modified {
            if let Some(watch) = self.watch.take() {
                self.tui.editor_closed(watch.path, &Ok(()));
            }
        }
    }

    #[cfg(test)]
    pub fn screen(&self) -> &Screen {
        &self.screen
    }
}

fn mouse_button(button: mouse::Button) -> Option<MouseButton> {
    match button {
        mouse::Button::Left => Some(MouseButton::Left),
        mouse::Button::Right => Some(MouseButton::Right),
        mouse::Button::Middle => Some(MouseButton::Middle),
        _ => None,
    }
}

fn modified(path: &std::path::Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Commands from the event loop, grouped so a burst is applied in one update.
fn command_batches(commands: mpsc::Receiver<UiCommand>) -> impl Stream<Item = Message> {
    futures::stream::unfold(Some(commands), |commands| async move {
        let mut commands = commands?;
        match commands.recv().await {
            Some(first) => {
                let mut batch = vec![first];
                while batch.len() < MAX_BATCH {
                    match commands.try_recv() {
                        Ok(cmd) => batch.push(cmd),
                        Err(_) => break,
                    }
                }
                Some((Message::Commands(batch), Some(commands)))
            }
            None => Some((Message::SessionEnded, None)),
        }
    })
}

#[allow(clippy::needless_pass_by_value)] // the signature `event::listen_with` takes
fn window_event(event: iced::Event, _status: event::Status, _id: window::Id) -> Option<Message> {
    match event {
        iced::Event::Window(window::Event::Resized(size) | window::Event::Opened { size, .. }) => {
            Some(Message::Resized(size))
        }
        iced::Event::Window(window::Event::Focused) => Some(Message::Focused(true)),
        iced::Event::Window(window::Event::Unfocused) => Some(Message::Focused(false)),
        iced::Event::Mouse(event) => Some(Message::Mouse(event)),
        _ => None,
    }
}

/// Editors that need a terminal of their own, which a desktop window
/// doesn't have.
fn is_terminal_editor(editor: &str) -> bool {
    let program = editor.split_whitespace().next().unwrap_or_default();
    let name = std::path::Path::new(program)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(program);
    matches!(
        name,
        "vi" | "vim"
            | "nvim"
            | "nano"
            | "pico"
            | "micro"
            | "hx"
            | "helix"
            | "kak"
            | "ed"
            | "joe"
            | "mg"
    ) || (name == "emacs" && editor.contains("-nw"))
}

/// Open a URL or file with the platform's default handler.
fn open_external(target: &str) {
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open").arg(target).spawn();
    #[cfg(target_os = "windows")]
    let result = std::process::Command::new("cmd")
        .args(["/C", "start", ""])
        .arg(target)
        .spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let result = std::process::Command::new("xdg-open").arg(target).spawn();
    if let Err(e) = result {
        tracing::warn!(error = %e, target, "could not open");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_editors_are_recognised() {
        assert!(is_terminal_editor("nvim"));
        assert!(is_terminal_editor("/usr/bin/vim -u NONE"));
        assert!(is_terminal_editor("emacs -nw"));
        assert!(!is_terminal_editor("code --wait"));
        assert!(!is_terminal_editor("emacs"));
    }
}
