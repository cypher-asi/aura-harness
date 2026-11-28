//! The window without a toolkit: the tabs of terminal UI, what the
//! window is told (commands from the session, time passing, its size and
//! focus), and the frame they add up to, as a [`Scene`] of quads.
//!
//! [`Shell`] knows nothing of the window it is in. It is told things,
//! says whether it now looks different ([`Shell::dirty`]) and when it
//! next needs waking ([`Shell::deadline`]), draws a frame when asked
//! ([`Shell::frame`]), and leaves what only a window can do as
//! [`Effect`]s. [`window`] is the window: it sleeps until something
//! happens, tells the shell, and draws at most one frame per frame of
//! the display.
//!
//! Time is the terminal UI's: its animations count ticks of 50 ms. While
//! an agent works the shell ticks at that rate, and redraws when the tab
//! in front animates; otherwise it wakes once a second to catch the
//! ticks up, and at the edges of the cursor's blink. Commands from the
//! session redraw only when they change the tab in front or the tab
//! bar's marks: thirty agents streaming behind the chat on screen cost
//! no frames.

#[cfg(test)]
mod bench;
mod editor;
mod glide;
mod keys;
mod pointer;
mod present;
mod refresh;
mod screen;
mod selection;
mod signals;
#[cfg(test)]
mod snapshot;
mod stats;
mod window;

pub use window::run;

use crate::chrome::Chrome;
use crate::geom::Size;
use crate::grid::{Metrics, Palette};
use crate::paint::{Painter, View};
use crate::router::{self, Arrangement, Tabs};
use crate::scene::Scene;
use crate::tab::{Settings, Tab};
use crate::tab_bar::{self, Bar};
use crate::{text, theme};
use aura_ui::events::{UiCommand, UiEvent};
use aura_ui_terminal::ProseRow;
use pointer::Pointer;
use ratatui::buffer::Cell;
use ratatui::layout::{Position, Rect};
use selection::SelectionFile;
use stats::Stats;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use winit::keyboard::ModifiersState;
use winit::window::ResizeDirection;

/// The terminal UI's tick; its spinners advance one frame per tick.
const TICK: Duration = Duration::from_millis(50);
/// While no agent works, ticks redraw only this often, for the few things
/// that change with time alone (notices expiring).
const IDLE_REDRAW_TICKS: u32 = 20;
/// Half a cursor blink.
const BLINK: Duration = Duration::from_millis(530);
/// The window's size when it opens, before the display's scale.
const WINDOW: Size = Size::new(1320.0, 860.0);
/// Most ticks made up for at once: after the machine slept, the UI's
/// animations pick up where they were instead of spinning through the
/// night.
const MAX_OWED: u128 = 40;

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
    /// Where what the tabs had selected is kept between runs.
    pub selection_file: Option<PathBuf>,
}

/// What the shell asks of the window it is in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Close the window.
    Exit,
    /// Put text on the clipboard, and say how it went
    /// ([`Shell::copied`]).
    Copy(String),
    /// Run `editor` on `path` for the tab with this id, and say when it
    /// exits ([`Shell::editor_closed`]).
    Edit {
        tab: u64,
        editor: String,
        path: PathBuf,
    },
    /// Ask for the user's attention, as a terminal's bell does.
    Attention,
    /// Move the window with the pointer until its button comes up.
    Drag,
    /// Resize the window by this side or corner the same way.
    Resize(ResizeDirection),
    Minimize,
    /// Fill the screen, or go back to the size before.
    ToggleMaximize,
}

pub struct Shell {
    tabs: Tabs,
    tabs_file: Option<PathBuf>,
    /// The split of projects over tabs as last written to `tabs_file`.
    saved: Arrangement,
    selection: SelectionFile,
    tui_theme: aura_ui_terminal::Theme,
    palette: Palette,
    /// The font size chosen, before the display's scale
    font_size: f32,
    /// Pixels of the display per pixel of a plain one
    scale: f32,
    /// Cell size at `font_size` on this display
    metrics: Metrics,
    /// The window, in the display's pixels
    window: Size,
    /// The grid of the tab in front, as last drawn
    cells: Vec<Cell>,
    cols: u16,
    rows: u16,
    /// Its rows of chat prose, set in the proportional face
    prose: Vec<ProseRow>,
    /// Where its panels are, in cells
    panes: Vec<Rect>,
    cursor: Option<Position>,
    chrome: Chrome,
    bar: Bar,
    painter: Painter,
    /// The modifier keys held
    modifiers: ModifiersState,
    pointer: Pointer,
    glide: glide::Glide,
    focused: bool,
    blink_from: Instant,
    /// Whether the last frame showed the cursor
    cursor_shown: bool,
    /// The time the terminal UIs have been ticked up to
    ticked: Instant,
    /// Whether the window looks different from the last frame
    dirty: bool,
    /// When the last frame was drawn, as the time passed to it
    drawn: Instant,
    effects: Vec<Effect>,
    stats: Stats,
}

impl Shell {
    /// The window's contents at its opening size, talking to the session
    /// through `events`.
    pub fn new(events: mpsc::Sender<UiEvent>, options: &Options, launched: Instant) -> Self {
        let settings = Settings {
            verbose: options.verbose,
            alerts: aura_ui_terminal::AlertSettings::from_env(),
            measure: text::prose_cells,
        };
        let tui_theme = aura_ui_terminal::Theme::by_name(&options.theme);
        let now = Instant::now();
        let saved = options
            .tabs_file
            .as_deref()
            .map(router::load_arrangement)
            .unwrap_or_default();
        let font_size = theme::initial_font_size();
        let mut tabs = Tabs::new(events, settings, saved.clone());
        let selection = SelectionFile::open(options.selection_file.clone(), &mut tabs);
        Self {
            tabs,
            tabs_file: options.tabs_file.clone(),
            saved,
            selection,
            palette: Palette::from_theme(&tui_theme),
            tui_theme,
            font_size,
            scale: 1.0,
            metrics: Metrics::new(font_size),
            window: WINDOW,
            cells: Vec::new(),
            cols: 0,
            rows: 0,
            prose: Vec::new(),
            panes: Vec::new(),
            cursor: None,
            chrome: Chrome::default(),
            bar: Bar::default(),
            painter: Painter::default(),
            modifiers: ModifiersState::empty(),
            pointer: Pointer::default(),
            glide: glide::Glide::default(),
            focused: true,
            blink_from: now,
            cursor_shown: true,
            ticked: now,
            dirty: true,
            drawn: now,
            effects: Vec::new(),
            stats: Stats::new(launched),
        }
    }

    /// Whether the window looks different from the last frame drawn, or
    /// the chat is sliding.
    pub const fn dirty(&self) -> bool {
        self.dirty || self.glide.moved
    }

    /// What the window is asked to do since this was last called.
    pub fn take_effects(&mut self) -> Vec<Effect> {
        std::mem::take(&mut self.effects)
    }

    /// The window's timings, for it to add what showing a frame took.
    pub const fn stats(&mut self) -> &mut Stats {
        &mut self.stats
    }

    /// Commands from the session, for the tabs they concern. Only the tab
    /// in front and the tab bar's marks are on screen, so commands for
    /// chats off screen ask for no frame.
    pub fn commands(&mut self, batch: impl IntoIterator<Item = UiCommand>) {
        let started = Instant::now();
        let marks = self.bar_marks();
        for command in batch {
            self.tabs.apply(command);
        }
        self.raise_alerts();
        self.handled(started, &marks);
    }

    /// The session closed its end of the command channel.
    #[allow(clippy::unused_self)] // told like everything else
    pub fn session_ended(&self) {
        tracing::warn!("the agent session closed its command channel");
    }

    /// The window is `size` of the display's pixels, `scale` of them to
    /// a pixel of a plain display.
    pub fn resized(&mut self, size: Size, scale: f32) {
        let scale = if scale.is_finite() && scale > 0.0 {
            scale
        } else {
            1.0
        };
        if (scale - self.scale).abs() > f32::EPSILON {
            self.scale = scale;
            self.metrics = Metrics::on_display(self.font_size, scale);
        }
        if size != self.window {
            self.window = size;
            // Selections are in cells, which move when the grid changes
            // size.
            self.clear_selections();
            self.glide.stop();
        }
        self.dirty = true;
    }

    /// The window gained or lost the keyboard.
    pub fn focused(&mut self, focused: bool, now: Instant) {
        self.focused = focused;
        if !focused {
            // The release may go to another window.
            self.pointer.tab_drag = None;
        }
        self.blink_from = now;
        self.dirty = true;
    }

    /// The editor a tab (by id) opened on `path` exited.
    pub fn editor_closed(&mut self, tab: u64, path: PathBuf, result: Result<(), String>) {
        if let Some(tab) = self.tabs.by_id_mut(tab) {
            tab.tui
                .editor_closed(path, &result.map_err(std::io::Error::other));
        }
        self.dirty = true;
    }

    /// How putting `chars` characters on the clipboard went.
    pub fn copied(&mut self, chars: usize, result: Result<(), String>) {
        let result = result.map_err(std::io::Error::other);
        self.tabs.front_mut().tui.notify_copied(chars, &result);
        self.dirty = true;
    }

    /// Catch the terminal UIs up with the clock: the ticks due by `now`,
    /// and the cursor's blink.
    pub fn advance(&mut self, now: Instant) {
        let owed = now.saturating_duration_since(self.ticked).as_millis() / TICK.as_millis();
        let due = if self.busy() {
            1
        } else {
            u128::from(IDLE_REDRAW_TICKS)
        };
        if owed >= due {
            let started = Instant::now();
            let marks = self.bar_marks();
            let ticks = u32::try_from(owed.min(MAX_OWED)).unwrap_or(1);
            self.ticked = if owed > MAX_OWED {
                now
            } else {
                self.ticked + TICK * ticks
            };
            for tab in self.tabs.iter_mut() {
                for _ in 0..ticks {
                    tab.tui.tick();
                }
                editor::poll_watch(tab, now);
            }
            self.raise_alerts();
            // A tab in front that doesn't animate is still drawn now and
            // then, for what changes with time alone.
            if now.saturating_duration_since(self.drawn) >= TICK * IDLE_REDRAW_TICKS {
                self.dirty = true;
            }
            self.handled(started, &marks);
            self.stats.report(now);
        }
        if self.blinking() && self.cursor_on(now) != self.cursor_shown {
            self.dirty = true;
        }
    }

    /// When to wake the shell ([`Self::advance`]) if nothing happens
    /// before: the next tick while an agent works, else a second after
    /// the last one, or sooner when the cursor is about to blink.
    pub fn deadline(&self, now: Instant) -> Instant {
        let ticks = if self.busy() { 1 } else { IDLE_REDRAW_TICKS };
        let tick = self.ticked + TICK * ticks;
        if !self.blinking() {
            return tick;
        }
        let since = now.saturating_duration_since(self.blink_from).as_millis();
        let edges = u32::try_from(since / BLINK.as_millis() + 1).unwrap_or(u32::MAX);
        tick.min(self.blink_from + BLINK.saturating_mul(edges))
    }

    /// The window as it is at `now`: the terminal UI of the tab in front
    /// rendered into the grid, and the grid, the tab bar and the panels'
    /// title bars painted as quads. A frame that only slides the chat is
    /// painted from the grid as it was.
    pub fn frame(&mut self, now: Instant) -> &Scene {
        let started = Instant::now();
        self.ease(now);
        let slid_only = self.glide.moved && !self.dirty && !self.cells.is_empty();
        if !slid_only {
            self.refresh(now);
            self.tabs.forward_events();
            self.fit_glide();
        }
        let rendered = started.elapsed();

        let cursor_on = self.cursor_on(now);
        self.cursor_shown = cursor_on;
        self.dirty = false;
        self.drawn = now;
        self.glide.moved = self.easing();
        let view = View {
            size: self.window,
            cells: &self.cells,
            cols: self.cols,
            rows: self.rows,
            prose: &self.prose,
            panes: &self.panes,
            cursor: self.cursor.filter(|_| cursor_on),
            metrics: self.metrics,
            palette: self.palette,
            bar: &self.bar,
            chrome: &self.chrome,
            slide: self.glide.slide(&self.tabs.front().tui),
            canvas: refresh::canvas_of(self.tabs.front(), &self.tui_theme, self.palette),
        };
        // After the UI is rendered: it measures prose with the same
        // fonts.
        self.painter.paint(&view);
        let painted = started.elapsed().saturating_sub(rendered);
        self.stats.frame(rendered, painted, self.painter.built());
        self.painter.scene()
    }

    /// Whether an agent of any tab is working, so spinners turn, the
    /// chat in front scrolls under a selection held past its edge, or a
    /// tab in front animates (a running benchmark, the LOG tab or the
    /// chat loading or fading in).
    fn busy(&self) -> bool {
        let front = &self.tabs.front().tui;
        self.tabs.iter().any(Tab::busy)
            || front.selection_scrolling()
            || front.bench_animating()
            || front.log_animating()
            || front.chat_animating()
    }

    /// Whether the cursor is on screen and blinking.
    const fn blinking(&self) -> bool {
        self.focused && self.cursor.is_some()
    }

    /// The cursor shows while typing and then blinks; it stays solid when
    /// the window isn't focused so it's easy to find.
    fn cursor_on(&self, now: Instant) -> bool {
        if !self.focused {
            return true;
        }
        let since = now.saturating_duration_since(self.blink_from).as_millis();
        (since / BLINK.as_millis()).is_multiple_of(2)
    }

    /// After input was handled: pass on what the UIs asked of the
    /// session, ask for a frame, and count the time it took.
    fn settle(&mut self, started: Instant) {
        self.tabs.forward_events();
        self.dirty = true;
        self.stats.update(started.elapsed());
    }

    /// After what the session sent or a tick was handled: pass on what
    /// the UIs asked of the session, ask for a frame when the tab in front
    /// changed or the tab bar's `marks` did, and count the time it took.
    fn handled(&mut self, started: Instant, marks: &(u64, Vec<bool>)) {
        self.tabs.forward_events();
        let front = self.tabs.front_mut().tui.take_redraw();
        if front || self.bar_marks() != *marks {
            self.dirty = true;
        }
        self.stats.update(started.elapsed());
    }

    /// What the tab bar shows that a tab's agents change: the tab in
    /// front, by id, and which tabs have an agent at work.
    fn bar_marks(&self) -> (u64, Vec<bool>) {
        let busy = self.tabs.iter().map(Tab::busy).collect();
        (self.tabs.front().id, busy)
    }

    fn clear_selections(&mut self) {
        for tab in self.tabs.iter_mut() {
            tab.tui.clear_selection();
        }
    }

    /// The part of the window below the tab bar, which the grid fills.
    fn grid_area(&self) -> Size {
        let bar = tab_bar::height(&self.metrics);
        Size::new(self.window.width, (self.window.height - bar).max(0.0))
    }

    /// Ring for what the UIs want noticed: a desktop notification when
    /// those are on, and the window asks for attention in place of the
    /// terminal bell.
    fn raise_alerts(&mut self) {
        // An agent is in one tab, so each tab rings for its own.
        let mut alerts = Vec::new();
        for tab in self.tabs.iter_mut() {
            alerts.extend(tab.tui.take_alerts());
        }
        if alerts.is_empty() {
            return;
        }
        let settings = self.tabs.settings().alerts;
        if settings.desktop {
            for alert in &alerts {
                aura_ui_terminal::alerts::desktop_notify(alert);
            }
        }
        if settings.bell {
            self.effects.push(Effect::Attention);
        }
    }

    /// The grid as text, a line per row.
    #[cfg(test)]
    pub fn text(&self) -> String {
        let mut out = String::new();
        for row in self.cells.chunks(usize::from(self.cols.max(1))) {
            for cell in row {
                out.push_str(cell.symbol());
            }
            out.push('\n');
        }
        out
    }
}

#[cfg(test)]
mod tests;
