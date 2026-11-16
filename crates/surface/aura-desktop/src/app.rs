//! The window: hosts the terminal UI (`aura_ui_terminal::App`) and draws it
//! on a GPU cell grid instead of a terminal, one copy per tab under a
//! tab bar. A tab is a workspace holding some of the session's projects.
//!
//! The loop mirrors `aura_ui_terminal::Terminal`: tick every 50 ms (the UI's
//! animations count ticks), apply agent commands, render the frame, then
//! copy selections and hand files to an editor when the UI asks. Every
//! tab is kept up to date; only the one in front is rendered.
//!
//! The window has no frame from the system: the tab bar is its title bar,
//! and its edges resize it.

use crate::grid::{Grid, Metrics, Palette, Screen};
use crate::perf::Perf;
use crate::router::{self, Arrangement, Tabs};
use crate::tab::{Settings, Watch};
use crate::tab_bar::{self, Bar, Hit};
use crate::{input, theme};
use aura_ui::events::{UiCommand, UiEvent};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use futures::Stream;
use iced::keyboard::{self, key::Named, Key, Modifiers};
use iced::widget::{canvas, column, mouse_area};
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
/// Width of the band along the window's sides that resizes it.
const EDGE: f32 = 6.0;
/// How far along a side from a corner the band resizes both ways.
const CORNER: f32 = 16.0;
/// Two presses on empty tab bar space this close together maximize.
const DOUBLE_CLICK: Duration = Duration::from_millis(400);

/// The channel pair connecting the UI to an agent session.
pub struct Handles {
    pub events: mpsc::Sender<UiEvent>,
    pub commands: mpsc::Receiver<UiCommand>,
}

/// Startup choices from the command line.
pub struct Options {
    pub theme: String,
    pub verbose: bool,
    /// Where the split of projects over tabs is kept between runs.
    pub tabs_file: Option<PathBuf>,
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
    /// The editor a tab (by id) opened on a file exited.
    EditorClosed(u64, PathBuf, Result<(), String>),
}

pub struct App {
    tabs: Tabs,
    tabs_file: Option<PathBuf>,
    /// The split of projects over tabs as last written to `tabs_file`.
    saved: Arrangement,
    tui_theme: aura_ui_terminal::Theme,
    /// The grid of the tab in front.
    screen: Screen,
    bar: Bar,
    metrics: Metrics,
    palette: Palette,
    window: Size,
    modifiers: Modifiers,
    /// The grid cell under the pointer.
    pointer: Option<Position>,
    /// Where the pointer is while it is over the tab bar.
    bar_pointer: Option<Point>,
    /// The window side or corner under the pointer.
    edge: Option<window::Direction>,
    /// When empty tab bar space was last pressed, for a double click.
    last_bar_press: Option<Instant>,
    left_down: bool,
    /// Scroll distance not yet turned into whole lines.
    scroll: f32,
    focused: bool,
    blink_from: Instant,
    now: Instant,
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
    .decorations(false)
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
        let settings = Settings {
            verbose: options.verbose,
            alerts: aura_ui_terminal::AlertSettings::from_env(),
        };
        let tui_theme = aura_ui_terminal::Theme::by_name(&options.theme);
        let now = Instant::now();
        let saved = options
            .tabs_file
            .as_deref()
            .map(router::load_arrangement)
            .unwrap_or_default();
        let mut app = Self {
            tabs: Tabs::new(handles.events, settings, saved.clone()),
            tabs_file: options.tabs_file.clone(),
            saved,
            palette: Palette::from_theme(&tui_theme),
            tui_theme,
            screen: Screen::default(),
            bar: Bar::default(),
            metrics: Metrics::new(theme::initial_font_size()),
            window: WINDOW,
            modifiers: Modifiers::empty(),
            pointer: None,
            bar_pointer: None,
            edge: None,
            last_bar_press: None,
            left_down: false,
            scroll: 0.0,
            focused: true,
            blink_from: now,
            now,
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
        let bar = canvas(&self.bar)
            .width(Length::Fill)
            .height(Length::Fixed(tab_bar::height(&self.metrics)));
        let grid = canvas(Grid {
            screen: &self.screen,
            metrics: self.metrics,
            palette: self.palette,
            cursor_on: self.cursor_on(),
        })
        .width(Length::Fill)
        .height(Length::Fill);
        let mut area = mouse_area(column![bar, grid]);
        if let Some(edge) = self.edge {
            area = area.interaction(resize_cursor(edge));
        }
        let element = area.into();
        self.perf.view(started.elapsed());
        element
    }

    pub(crate) fn update(&mut self, message: Message) -> Task<Message> {
        let started = Instant::now();
        let task = self.handle(message);
        self.tabs.forward_events();
        let task = Task::batch([task, self.refresh()]);
        self.perf.update(started.elapsed());
        task
    }

    /// The part of the window below the tab bar, which the grid fills.
    fn grid_area(&self) -> Size {
        let bar = tab_bar::height(&self.metrics);
        Size::new(self.window.width, (self.window.height - bar).max(0.0))
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
                    self.tabs.apply(cmd);
                }
            }
            Message::SessionEnded => {
                tracing::warn!("the agent session closed its command channel");
            }
            Message::Tick(now) => {
                self.now = now;
                self.perf.frame(now);
                for tab in self.tabs.iter_mut() {
                    tab.tui.tick();
                    poll_watch(tab, now);
                }
                return self.raise_alerts();
            }
            Message::Key(event) => return self.key(event),
            Message::Mouse(event) => return self.mouse(event),
            Message::Resized(size) => {
                self.window = size;
                self.clear_selections();
            }
            Message::Focused(focused) => {
                self.focused = focused;
                self.restart_blink();
            }
            Message::Copied(chars) => self.tabs.front_mut().tui.notify_copied(chars, &Ok(())),
            Message::EditorClosed(tab, path, result) => {
                if let Some(tab) = self.tabs.by_id_mut(tab) {
                    tab.tui
                        .editor_closed(path, &result.map_err(std::io::Error::other));
                }
            }
        }
        Task::none()
    }

    fn restart_blink(&mut self) {
        self.now = Instant::now();
        self.blink_from = self.now;
    }

    /// Selections are in cells, which move when the grid changes size.
    fn clear_selections(&mut self) {
        for tab in self.tabs.iter_mut() {
            tab.tui.clear_selection();
        }
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
                if let Some(task) = self.shortcut(&key, &modified_key, modifiers) {
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

    /// Window shortcuts a terminal emulator would handle itself: tabs,
    /// zoom, copy and paste.
    fn shortcut(&mut self, plain: &Key, key: &Key, m: Modifiers) -> Option<Task<Message>> {
        if m.control() && matches!(key.as_ref(), Key::Named(Named::Tab)) {
            if m.shift() {
                self.tabs.previous();
            } else {
                self.tabs.next();
            }
            return Some(Task::none());
        }
        let chord = if cfg!(target_os = "macos") {
            m.logo()
        } else {
            m.control()
        };
        // Tabs by number, as window managers switch workspaces. Option
        // types symbols on macOS, so Alt is for the other systems.
        if chord || (m.alt() && !cfg!(target_os = "macos")) {
            if let Some(number) = tab_number(plain) {
                self.tabs.focus(number - 1);
                return Some(Task::none());
            }
        }
        if !chord {
            if m.shift() && matches!(key.as_ref(), Key::Named(Named::Insert)) {
                self.tabs.front_mut().tui.paste_from_clipboard();
                return Some(Task::none());
            }
            return None;
        }
        // Ctrl+C, Ctrl+V, Ctrl+T and Ctrl+W on their own belong to the
        // UI, as in a terminal; the window takes them with Shift.
        let shifted = m.shift() || cfg!(target_os = "macos");
        let size = self.metrics.font_size;
        let zoom = |size: f32| Some(size.clamp(theme::MIN_FONT_SIZE, theme::MAX_FONT_SIZE));
        let new_size = match key.as_ref() {
            Key::Character("=" | "+") => zoom(size + 1.0),
            Key::Character("-" | "_") => zoom(size - 1.0),
            Key::Character("0" | ")") => Some(theme::DEFAULT_FONT_SIZE),
            Key::Character("v" | "V") if shifted => {
                self.tabs.front_mut().tui.paste_from_clipboard();
                return Some(Task::none());
            }
            Key::Character("c" | "C") if shifted => {
                self.tabs.front_mut().tui.request_copy();
                return Some(Task::none());
            }
            Key::Character("t" | "T") if shifted => {
                self.tabs.new_tab();
                return Some(Task::none());
            }
            Key::Character("w" | "W") if shifted => {
                self.tabs.close(self.tabs.front_index());
                return Some(Task::none());
            }
            _ => None,
        }?;
        if (new_size - size).abs() > f32::EPSILON {
            self.metrics = Metrics::new(new_size);
            self.screen.invalidate();
            self.clear_selections();
        }
        Some(Task::none())
    }

    /// One key, with the terminal's Ctrl+C and Ctrl+D rules. `true` quits.
    fn key_press(&mut self, key: KeyEvent) -> bool {
        let tui = &mut self.tabs.front_mut().tui;
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('c') => {
                    if tui.request_copy() {
                        return false;
                    }
                    if !tui.is_busy() {
                        return true;
                    }
                    tui.cancel();
                    return false;
                }
                KeyCode::Char('d') => return true,
                _ => {}
            }
        }
        tui.handle_key(key).should_quit()
    }

    fn mouse(&mut self, event: mouse::Event) -> Task<Message> {
        let kind = match event {
            mouse::Event::CursorMoved { position } => {
                self.edge = (!self.left_down)
                    .then(|| edge(self.window, position))
                    .flatten();
                let bar = tab_bar::height(&self.metrics);
                // A drag that began in the grid keeps selecting there.
                if position.y < bar && !self.left_down {
                    self.bar_pointer = Some(position);
                    self.pointer = None;
                    return Task::none();
                }
                self.bar_pointer = None;
                let (cols, rows) = self.screen.size();
                let in_grid = Point::new(position.x, position.y - bar);
                let at = self.metrics.cell_at(self.grid_area(), cols, rows, in_grid);
                if self.pointer == Some(at) {
                    return Task::none();
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
                self.bar_pointer = None;
                self.edge = None;
                return Task::none();
            }
            mouse::Event::ButtonPressed(button) => {
                if let (Some(edge), mouse::Button::Left) = (self.edge, button) {
                    return window::latest().and_then(move |id| window::drag_resize(id, edge));
                }
                if self.bar_pointer.is_some() {
                    return self.bar_press(button);
                }
                let Some(button) = mouse_button(button) else {
                    return Task::none();
                };
                self.left_down |= button == MouseButton::Left;
                MouseEventKind::Down(button)
            }
            mouse::Event::ButtonReleased(button) => {
                let Some(button) = mouse_button(button) else {
                    return Task::none();
                };
                if button == MouseButton::Left {
                    self.left_down = false;
                }
                MouseEventKind::Up(button)
            }
            mouse::Event::WheelScrolled { delta } => {
                if self.bar_pointer.is_some() {
                    return Task::none();
                }
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
                return Task::none();
            }
            mouse::Event::CursorEntered => return Task::none(),
        };
        self.send_mouse(kind);
        Task::none()
    }

    /// A button went down on the tab bar: left picks or opens a tab or
    /// works a window box, middle closes the tab under the pointer, and
    /// left on empty space moves the window, or maximizes it when pressed
    /// twice.
    fn bar_press(&mut self, button: mouse::Button) -> Task<Message> {
        match (self.bar.hover(), button) {
            (Some(Hit::Tab(index)), mouse::Button::Left) => self.tabs.focus(index),
            (Some(Hit::Tab(index)), mouse::Button::Middle) => {
                self.tabs.close(index);
            }
            (Some(Hit::New), mouse::Button::Left) => self.tabs.new_tab(),
            (Some(Hit::Minimize), mouse::Button::Left) => {
                return window::latest().and_then(|id| window::minimize(id, true));
            }
            (Some(Hit::Maximize), mouse::Button::Left) => {
                return window::latest().and_then(window::toggle_maximize);
            }
            (Some(Hit::Close), mouse::Button::Left) => return iced::exit(),
            (None, mouse::Button::Left) => {
                let now = Instant::now();
                let double = self
                    .last_bar_press
                    .is_some_and(|at| now.saturating_duration_since(at) < DOUBLE_CLICK);
                self.last_bar_press = (!double).then_some(now);
                return if double {
                    window::latest().and_then(window::toggle_maximize)
                } else {
                    window::latest().and_then(window::drag)
                };
            }
            _ => {}
        }
        Task::none()
    }

    fn send_mouse(&mut self, kind: MouseEventKind) {
        let Some(at) = self.pointer else {
            return;
        };
        let modifiers = input::modifiers(self.modifiers);
        self.tabs.front_mut().tui.handle_mouse(MouseEvent {
            kind,
            column: at.x,
            row: at.y,
            modifiers,
        });
    }

    /// Ring for what the UI wants noticed: a desktop notification when
    /// those are on, and the window asks for attention in place of the
    /// terminal bell.
    fn raise_alerts(&mut self) -> Task<Message> {
        // An agent is in one tab, so each tab rings for its own.
        let mut alerts = Vec::new();
        for tab in self.tabs.iter_mut() {
            alerts.extend(tab.tui.take_alerts());
        }
        if alerts.is_empty() {
            return Task::none();
        }
        let settings = self.tabs.settings().alerts;
        if settings.desktop {
            for alert in &alerts {
                aura_ui_terminal::alerts::desktop_notify(alert);
            }
        }
        if !settings.bell {
            return Task::none();
        }
        window::latest().and_then(|id| {
            window::request_user_attention(id, Some(window::UserAttention::Informational))
        })
    }

    /// Render the UI of the tab in front into the grid, then act on what
    /// the tabs asked for.
    fn refresh(&mut self) -> Task<Message> {
        let area = self.grid_area();
        let (cols, rows) = self.metrics.grid(area);
        let cell = self.metrics.cell;
        let mut tasks = Vec::new();

        let tab = self.tabs.front_mut();
        if tab.tui.take_paste_request() {
            tab.tui.paste_from_clipboard();
        }
        let drawn = tab.terminal.backend().buffer().area;
        if (drawn.width, drawn.height) != (cols, rows) {
            let pixels = Size::new(f32::from(cols) * cell.width, f32::from(rows) * cell.height);
            tab.terminal.backend_mut().resize(cols, rows, pixels);
        }
        let (tui, tui_theme) = (&mut tab.tui, &self.tui_theme);
        match tab
            .terminal
            .draw(|frame| aura_ui_terminal::render(frame, tui, tui_theme))
        {
            Ok(frame) => {
                if let Some(text) = tui.take_copy_text(frame.buffer) {
                    let chars = text.chars().count();
                    tasks.push(
                        iced::clipboard::write(text).chain(Task::done(Message::Copied(chars))),
                    );
                }
                self.screen.update(frame.buffer, tui.prose_rows());
            }
            Err(e) => tracing::warn!(error = %e, "render failed"),
        }
        self.screen.set_cursor(tab.terminal.backend().cursor());

        let now = self.now;
        for tab in self.tabs.iter_mut() {
            if let Some(path) = tab.tui.take_editor_request() {
                tasks.push(open_editor(tab, path, now));
            }
        }

        let origin = self.metrics.origin(area, cols, rows);
        let model = tab_bar::Model {
            items: self
                .tabs
                .iter()
                .enumerate()
                .map(|(index, tab)| tab_bar::Item {
                    label: (index + 1).to_string(),
                    busy: tab.busy(),
                })
                .collect(),
            front: self.tabs.front_index(),
            title: self.tabs.front().title(),
            // Panel borders run through the middle of the outermost cells.
            x: (origin.x + cell.width / 2.0).round(),
            width: f32::from(cols.saturating_sub(1)) * cell.width,
            metrics: self.metrics,
            palette: self.palette,
        };
        self.bar.update(model, self.bar_pointer);
        self.save_tabs();
        Task::batch(tasks)
    }

    /// Write the split of projects over tabs when it changed, once the
    /// session has listed its projects.
    fn save_tabs(&mut self) {
        let Some(path) = &self.tabs_file else {
            return;
        };
        let arrangement = self.tabs.arrangement();
        if arrangement == self.saved || arrangement.iter().any(Vec::is_empty) {
            return;
        }
        if let Err(e) = router::save_arrangement(path, &arrangement) {
            tracing::warn!(error = %e, path = %path.display(), "could not save the tabs");
        }
        self.saved = arrangement;
    }

    #[cfg(test)]
    pub fn screen(&self) -> &Screen {
        &self.screen
    }

    #[cfg(test)]
    pub fn tabs(&self) -> &Tabs {
        &self.tabs
    }

    #[cfg(test)]
    pub fn bar(&self) -> &Bar {
        &self.bar
    }
}

/// The tab a number key picks, counting from 1.
fn tab_number(key: &Key) -> Option<usize> {
    match key.as_ref() {
        Key::Character(digit) => digit.parse().ok().filter(|number| (1..=9).contains(number)),
        _ => None,
    }
}

/// Hand a file to the user's editor for `tab`. Editors that need a
/// terminal can't run in a window, so those (and no editor at all) fall
/// back to the system's default app, and saving the file counts as
/// closing it.
fn open_editor(tab: &mut crate::tab::Tab, path: PathBuf, now: Instant) -> Task<Message> {
    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .ok()
        .filter(|e| !e.trim().is_empty() && !is_terminal_editor(e));
    let Some(editor) = editor else {
        open_external(&path.display().to_string());
        tab.watch = Some(Watch {
            modified: modified(&path),
            path,
            checked: now,
        });
        return Task::none();
    };
    let (id, target) = (tab.id, path.clone());
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
        move |result| Message::EditorClosed(id, path.clone(), result),
    )
}

/// Tell `tab`'s UI the file it is waiting on was saved, once it was.
fn poll_watch(tab: &mut crate::tab::Tab, now: Instant) {
    let Some(watch) = tab.watch.as_mut() else {
        return;
    };
    if now.saturating_duration_since(watch.checked) < EDIT_POLL {
        return;
    }
    watch.checked = now;
    if modified(&watch.path) != watch.modified {
        if let Some(watch) = tab.watch.take() {
            tab.tui.editor_closed(watch.path, &Ok(()));
        }
    }
}

/// The side or corner of a `window`-sized window that `at` resizes.
fn edge(window: Size, at: Point) -> Option<window::Direction> {
    use window::Direction::{East, North, NorthEast, NorthWest, South, SouthEast, SouthWest, West};
    let within = |band: f32| {
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

fn resize_cursor(edge: window::Direction) -> mouse::Interaction {
    use window::Direction::{East, North, NorthEast, NorthWest, South, SouthEast, SouthWest, West};
    match edge {
        North | South => mouse::Interaction::ResizingVertically,
        East | West => mouse::Interaction::ResizingHorizontally,
        NorthEast | SouthWest => mouse::Interaction::ResizingDiagonallyUp,
        NorthWest | SouthEast => mouse::Interaction::ResizingDiagonallyDown,
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

    #[test]
    fn the_window_edges_resize_it() {
        use window::Direction::{East, North, NorthWest, SouthEast, SouthWest};
        let size = Size::new(800.0, 600.0);
        let at = |x, y| edge(size, Point::new(x, y));
        assert!(matches!(at(400.0, 2.0), Some(North)));
        assert!(matches!(at(798.0, 300.0), Some(East)));
        assert!(matches!(at(2.0, 2.0), Some(NorthWest)));
        assert!(matches!(at(10.0, 2.0), Some(NorthWest)), "near the corner");
        assert!(matches!(at(2.0, 590.0), Some(SouthWest)));
        assert!(matches!(at(799.0, 599.0), Some(SouthEast)));
        assert!(at(400.0, 300.0).is_none());
        assert!(at(400.0, 10.0).is_none(), "the tab bar below the band");
    }
}
