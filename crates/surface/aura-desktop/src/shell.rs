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
mod screen;
mod signals;
#[cfg(test)]
mod snapshot;
mod stats;
mod window;

pub use window::run;

use crate::canvas::{self, Canvas};
use crate::chrome::{self, Chrome};
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
        Self {
            tabs: Tabs::new(events, settings, saved.clone()),
            tabs_file: options.tabs_file.clone(),
            saved,
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
            canvas: canvas_of(self.tabs.front(), &self.tui_theme, self.palette),
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

    /// Render the UI of the tab in front into the grid, then act on what
    /// the tabs asked for.
    fn refresh(&mut self, now: Instant) {
        let area = self.grid_area();
        let (cols, rows) = self.metrics.grid(area);
        let cell = self.metrics.cell;

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
        // This frame shows whatever changed so far.
        tui.take_redraw();
        match tab
            .terminal
            .draw(|frame| aura_ui_terminal::render(frame, tui, tui_theme))
        {
            Ok(frame) => {
                if let Some(text) = tui.take_copy_text(frame.buffer) {
                    self.effects.push(Effect::Copy(text));
                }
                self.cells.clone_from(&frame.buffer.content);
                (self.cols, self.rows) = (cols, rows);
                tui.prose_rows().clone_into(&mut self.prose);
                self.panes.clear();
                self.panes
                    .extend(tui.pane_chrome().iter().map(|chrome| chrome.area));
                let at = (self.metrics.origin(area, cols), self.metrics, self.palette);
                let model = chrome::Model::new(tui.pane_chrome(), frame.buffer, at);
                self.chrome.update(model, self.pointer.on_grid);
            }
            Err(e) => tracing::warn!(error = %e, "render failed"),
        }
        self.cursor = tab.terminal.backend().cursor();

        for tab in self.tabs.iter_mut() {
            if let Some(path) = tab.tui.take_editor_request() {
                self.effects.extend(editor::open(tab, path, now));
            }
        }

        let origin = self.metrics.origin(area, cols);
        // The panels' outer borders, which the UI keeps a column in from
        // the grid's edges, run through the middle of their cells; the
        // bar's boxes line up with them.
        let left = self.panes.iter().map(|pane| pane.x).min().unwrap_or(1);
        let right = self
            .panes
            .iter()
            .map(|pane| pane.right())
            .max()
            .unwrap_or_else(|| cols.saturating_sub(1))
            .max(left + 1);
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
            x: (origin.x + (f32::from(left) + 0.5) * cell.width).round(),
            width: f32::from(right - left - 1) * cell.width,
            metrics: self.metrics,
            palette: self.palette,
        };
        self.bar.update(model, self.pointer.on_bar);
        self.save_tabs();
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

/// The GRAPH tab's canvas, when `tab` shows it.
fn canvas_of<'a>(
    tab: &'a Tab,
    theme: &aura_ui_terminal::Theme,
    palette: Palette,
) -> Option<Canvas<'a>> {
    let tui = &tab.tui;
    tui.flow_canvas().map(|area| Canvas {
        flow: tui.flow(),
        area,
        clip: tui.flow_canvas_clip(),
        colors: canvas::Colors::new(theme, palette),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raster;
    use aura_ui::events::AgentSummary;
    use std::time::Duration;

    pub(super) struct Stage {
        pub shell: Shell,
        pub events: mpsc::Receiver<UiEvent>,
        pub now: Instant,
    }

    pub(super) fn stage() -> Stage {
        let (events, events_rx) = mpsc::channel(64);
        let options = Options {
            theme: "cyber".into(),
            verbose: false,
            tabs_file: None,
        };
        let now = Instant::now();
        let mut shell = Shell::new(events, &options, now);
        shell.ticked = now;
        shell.blink_from = now;
        Stage {
            shell,
            events: events_rx,
            now,
        }
    }

    fn agent(id: &str, name: &str, running: bool) -> AgentSummary {
        AgentSummary {
            id: id.into(),
            name: name.into(),
            zns_id: format!("0://{name}"),
            running,
            ..AgentSummary::default()
        }
    }

    impl Stage {
        /// The frame at `self.now`, as pixels.
        pub fn pixels(&mut self) -> Vec<u8> {
            let scene = self.shell.frame(self.now).clone();
            let mut pixels = Vec::new();
            text::with(|text| raster::paint(&scene, text.atlas(), &mut pixels));
            pixels
        }

        pub fn seed(&mut self, running: bool) {
            self.shell.commands([
                UiCommand::SetAgents(vec![agent("a1", "agent01", running)]),
                UiCommand::SetActiveAgent("a1".into()),
            ]);
        }

        /// What the window asked of the session since the last call.
        pub fn asked(&mut self) -> Vec<String> {
            let mut out = Vec::new();
            while let Ok(event) = self.events.try_recv() {
                out.push(format!("{event:?}"));
            }
            out
        }

        /// The session's answer to creating the agent `id`, beside the
        /// one [`Self::seed`] lists.
        pub fn created(&mut self, id: &str, name: &str) {
            self.shell.commands([
                UiCommand::SetActiveAgent(id.into()),
                UiCommand::SetAgents(vec![agent("a1", "agent01", false), agent(id, name, false)]),
                UiCommand::SetActiveAgent(id.into()),
            ]);
        }

        /// Move the clock on and let the shell catch up.
        pub fn pass(&mut self, time: Duration) {
            self.now += time;
            self.shell.advance(self.now);
        }
    }

    #[test]
    fn the_first_frame_fills_the_window_with_the_ui() {
        let mut stage = stage();
        assert!(stage.shell.dirty(), "nothing is drawn yet");
        let scene = stage.shell.frame(stage.now).clone();
        assert!(!stage.shell.dirty());
        assert_eq!((scene.width, scene.height), (1320, 860));
        // Two rows fewer than the window alone holds: the tab bar is above.
        assert_eq!((stage.shell.cols, stage.shell.rows), (155, 45));
        assert!(scene.quads.len() > 500, "{} quads", scene.quads.len());
        let text = stage.shell.text();
        assert_eq!(text.lines().count(), 45);
        assert!(text.contains("AURA"), "{text}");
    }

    #[test]
    fn commands_reach_the_tabs_and_ask_for_a_frame() {
        let mut stage = stage();
        stage.seed(false);
        let before = stage.pixels();
        assert!(!stage.shell.dirty());

        stage.shell.commands([
            UiCommand::StartStreaming,
            UiCommand::AppendText("a reply from the agent".into()),
        ]);
        assert!(stage.shell.dirty());
        let after = stage.pixels();
        assert!(stage.shell.text().contains("a reply from the agent"));
        assert!(before != after, "the reply is on screen");
        assert!(stage.shell.take_effects().is_empty());
    }

    #[test]
    fn commands_for_a_chat_off_screen_ask_for_no_frame() {
        let mut stage = stage();
        stage.shell.commands([
            UiCommand::SetAgents(vec![
                agent("a1", "agent01", false),
                agent("a2", "agent02", false),
            ]),
            UiCommand::SetActiveAgent("a1".into()),
        ]);
        stage.shell.frame(stage.now);
        let to = |agent: &str, command| UiCommand::ForAgent {
            agent_id: agent.into(),
            command: Box::new(command),
        };

        stage
            .shell
            .commands([to("a2", UiCommand::TurnStarted { prompt: None })]);
        assert!(stage.shell.dirty(), "the tab bar marks the tab at work");
        let before = stage.shell.frame(stage.now).clone();
        stage.shell.commands([
            to("a2", UiCommand::StartStreaming),
            to("a2", UiCommand::AppendText("off screen".into())),
        ]);
        assert!(!stage.shell.dirty(), "its chat is not on screen");
        assert!(before == *stage.shell.frame(stage.now));

        stage.shell.commands([
            to("a1", UiCommand::StartStreaming),
            to("a1", UiCommand::AppendText("on screen".into())),
        ]);
        assert!(stage.shell.dirty());
    }

    /// What `shell` last drew, painted again with nothing kept.
    pub(super) fn afresh(shell: &Shell) -> Vec<u8> {
        let view = View {
            size: shell.window,
            cells: &shell.cells,
            cols: shell.cols,
            rows: shell.rows,
            prose: &shell.prose,
            panes: &shell.panes,
            cursor: shell.cursor.filter(|_| shell.cursor_shown),
            metrics: shell.metrics,
            palette: shell.palette,
            bar: &shell.bar,
            chrome: &shell.chrome,
            slide: shell.glide.slide(&shell.tabs.front().tui),
            canvas: canvas_of(shell.tabs.front(), &shell.tui_theme, shell.palette),
        };
        let mut pixels = Vec::new();
        text::with(|text| {
            let mut painter = Painter::default();
            let scene = painter.paint_with(&view, text);
            raster::paint(scene, text.atlas(), &mut pixels);
        });
        pixels
    }

    #[test]
    fn a_streaming_reply_is_drawn_from_kept_rows_as_it_would_be_afresh() {
        let mut stage = stage();
        // A small window: every frame is rasterized twice.
        stage.shell.resized(Size::new(700.0, 420.0), 1.0);
        stage.seed(true);
        stage.shell.commands([UiCommand::StartStreaming]);
        let reply = "The **kernel** records every step, then `cargo test` runs.\n\n\
                     - one thing that wraps over more than a single line of the chat\n\
                     - another\n\n```rust\nfn main() {}\n```\n\n";
        let text: Vec<char> = reply.repeat(6).chars().collect();
        let mut kept = 0;
        for (i, chunk) in text.chunks(9).enumerate() {
            stage
                .shell
                .commands([UiCommand::AppendText(chunk.iter().collect())]);
            // Spinners turn and the cursor blinks as it streams.
            stage.pass(Duration::from_millis(50));
            let drawn = stage.pixels();
            kept += usize::from(stage.shell.painter.built() < usize::from(stage.shell.rows));
            assert!(drawn == afresh(&stage.shell), "chunk {i} left a stale row");
        }
        assert!(kept > 50, "rows were kept from frame to frame: {kept}");
    }

    #[test]
    fn an_idle_window_wakes_once_a_second_and_looks_the_same() {
        let mut stage = stage();
        stage.seed(false);
        // No cursor blinks in a window without the keyboard.
        stage.shell.focused(false, stage.now);
        let before = stage.shell.frame(stage.now).clone();
        assert_eq!(
            stage.shell.deadline(stage.now),
            stage.now + Duration::from_secs(1)
        );

        stage.pass(Duration::from_millis(400));
        assert!(!stage.shell.dirty(), "nothing is due yet");
        stage.pass(Duration::from_millis(600));
        assert!(stage.shell.dirty(), "the second's ticks were made up");
        assert_eq!(
            stage.shell.deadline(stage.now),
            stage.now + Duration::from_secs(1)
        );
        let after = stage.shell.frame(stage.now).clone();
        assert!(before == after, "and nothing moved: nothing to show");
    }

    #[test]
    fn a_working_agent_ticks_twenty_times_a_second() {
        let mut stage = stage();
        stage.seed(true);
        stage.shell.focused(false, stage.now);
        stage.shell.frame(stage.now);
        assert_eq!(
            stage.shell.deadline(stage.now),
            stage.now + Duration::from_millis(50)
        );
        stage.pass(Duration::from_millis(49));
        assert!(!stage.shell.dirty());
        stage.pass(Duration::from_millis(1));
        assert!(stage.shell.dirty(), "a tick turns the spinners");
        stage.shell.frame(stage.now);

        // Held up for a while, it makes the ticks up and is on time again.
        stage.pass(Duration::from_millis(730));
        assert_eq!(
            stage.shell.deadline(stage.now),
            stage.now + Duration::from_millis(20)
        );
        // After a night asleep it does not spin through the night.
        stage.pass(Duration::from_secs(8 * 60 * 60));
        assert_eq!(
            stage.shell.deadline(stage.now),
            stage.now + Duration::from_millis(50)
        );
    }

    #[test]
    fn the_cursor_blinks_only_while_the_window_has_the_keyboard() {
        let mut stage = stage();
        stage.seed(false);
        let on = stage.pixels();
        assert!(stage.shell.cursor.is_some(), "the input line has a cursor");
        assert_eq!(stage.shell.deadline(stage.now), stage.now + BLINK);

        stage.pass(BLINK - Duration::from_millis(1));
        assert!(!stage.shell.dirty());
        stage.pass(Duration::from_millis(1));
        assert!(stage.shell.dirty(), "half a blink on, the cursor goes");
        let off = stage.pixels();
        assert!(on != off);
        // The second's ticks come due before the cursor is back.
        assert_eq!(
            stage.shell.deadline(stage.now),
            stage.now + Duration::from_secs(1) - BLINK
        );

        stage.shell.focused(false, stage.now);
        assert!(on == stage.pixels(), "solid without the keyboard");
        // Nothing left to wake for but the second's ticks.
        assert_eq!(
            stage.shell.deadline(stage.now),
            stage.now + Duration::from_secs(1) - BLINK
        );
        stage.pass(Duration::from_millis(400));
        assert!(!stage.shell.dirty());
    }

    #[test]
    fn a_denser_display_draws_the_same_grid_with_more_pixels() {
        let mut stage = stage();
        stage.shell.frame(stage.now);
        let grid = (stage.shell.cols, stage.shell.rows);
        let cell = stage.shell.metrics.cell;

        stage.shell.resized(Size::new(2640.0, 1720.0), 2.0);
        assert!(stage.shell.dirty());
        let scene = stage.shell.frame(stage.now).clone();
        assert_eq!((scene.width, scene.height), (2640, 1720));
        assert_eq!(stage.shell.metrics.cell.height, cell.height * 2.0);
        assert_eq!(stage.shell.metrics.pad, 12.0, "and the space around it");
        let (cols, rows) = (stage.shell.cols, stage.shell.rows);
        assert!(cols.abs_diff(grid.0) <= 1 && rows.abs_diff(grid.1) <= 1);
    }

    #[test]
    fn a_bell_asks_for_attention() {
        let mut stage = stage();
        stage.seed(false);
        stage.shell.frame(stage.now);
        assert!(stage.shell.take_effects().is_empty());
        let alerts = stage.shell.tabs.settings().alerts;
        if alerts.desktop {
            // Not from a test: it would pop up a notification.
            return;
        }
        stage.shell.commands([UiCommand::ForAgent {
            agent_id: "a1".into(),
            command: Box::new(UiCommand::RequestApproval {
                id: "r1".into(),
                tool: "run_command".into(),
                description: "rm -rf target/".into(),
                always: "`rm` commands in this project".into(),
            }),
        }]);
        let effects = stage.shell.take_effects();
        assert_eq!(
            effects.contains(&Effect::Attention),
            alerts.bell,
            "{effects:?}"
        );
    }
}
