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
//! an agent works the shell ticks at that rate and redraws; otherwise it
//! wakes once a second to catch the ticks up, and at the edges of the
//! cursor's blink.

mod keys;
mod pointer;
mod stats;
mod window;

pub use window::run;

use crate::app::{self, Options, BLINK, IDLE_REDRAW_TICKS, TICK, WINDOW};
use crate::chrome::{self, Chrome};
use crate::geom::Size;
use crate::grid::{Metrics, Palette};
use crate::paint::{Painter, View};
use crate::router::{self, Arrangement, Tabs};
use crate::scene::Scene;
use crate::tab::{Settings, Tab, Watch};
use crate::tab_bar::{self, Bar};
use crate::{text, theme};
use aura_ui::events::{UiCommand, UiEvent};
use aura_ui_terminal::ProseRow;
use pointer::Pointer;
use ratatui::buffer::Cell;
use ratatui::layout::Position;
use stats::Stats;
use std::path::PathBuf;
use std::time::Instant;
use tokio::sync::mpsc;
use winit::keyboard::ModifiersState;
use winit::window::ResizeDirection;

/// Most ticks made up for at once: after the machine slept, the UI's
/// animations pick up where they were instead of spinning through the
/// night.
const MAX_OWED: u128 = 40;

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
    cursor: Option<Position>,
    chrome: Chrome,
    bar: Bar,
    painter: Painter,
    /// The modifier keys held
    modifiers: ModifiersState,
    pointer: Pointer,
    focused: bool,
    blink_from: Instant,
    /// Whether the last frame showed the cursor
    cursor_shown: bool,
    /// The time the terminal UIs have been ticked up to
    ticked: Instant,
    /// Whether the window looks different from the last frame
    dirty: bool,
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
            cursor: None,
            chrome: Chrome::default(),
            bar: Bar::default(),
            painter: Painter::default(),
            modifiers: ModifiersState::empty(),
            pointer: Pointer::default(),
            focused: true,
            blink_from: now,
            cursor_shown: true,
            ticked: now,
            dirty: true,
            effects: Vec::new(),
            stats: Stats::new(launched),
        }
    }

    /// Whether the window looks different from the last frame drawn.
    pub const fn dirty(&self) -> bool {
        self.dirty
    }

    /// What the window is asked to do since this was last called.
    pub fn take_effects(&mut self) -> Vec<Effect> {
        std::mem::take(&mut self.effects)
    }

    /// The window's timings, for it to add what showing a frame took.
    pub const fn stats(&mut self) -> &mut Stats {
        &mut self.stats
    }

    /// Commands from the session, for the tabs they concern.
    pub fn commands(&mut self, batch: impl IntoIterator<Item = UiCommand>) {
        let started = Instant::now();
        for command in batch {
            self.tabs.apply(command);
        }
        self.raise_alerts();
        self.settle(started);
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
            self.metrics = Metrics::new(self.font_size * scale);
        }
        if size != self.window {
            self.window = size;
            // Selections are in cells, which move when the grid changes
            // size.
            self.clear_selections();
        }
        self.dirty = true;
    }

    /// The window gained or lost the keyboard.
    pub fn focused(&mut self, focused: bool, now: Instant) {
        self.focused = focused;
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
                app::poll_watch(tab, now);
            }
            self.raise_alerts();
            self.settle(started);
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
    /// title bars painted as quads.
    pub fn frame(&mut self, now: Instant) -> &Scene {
        let started = Instant::now();
        self.refresh(now);
        self.tabs.forward_events();
        let rendered = started.elapsed();

        let cursor_on = self.cursor_on(now);
        self.cursor_shown = cursor_on;
        self.dirty = false;
        let view = View {
            size: self.window,
            cells: &self.cells,
            cols: self.cols,
            rows: self.rows,
            prose: &self.prose,
            cursor: self.cursor.filter(|_| cursor_on),
            metrics: self.metrics,
            palette: self.palette,
            bar: &self.bar,
            chrome: &self.chrome,
        };
        // After the UI is rendered: it measures prose with the same
        // fonts.
        self.painter.paint(&view);
        let painted = started.elapsed().saturating_sub(rendered);
        self.stats.frame(rendered, painted, self.painter.built());
        self.painter.scene()
    }

    /// Whether an agent of any tab is working: spinners turn.
    fn busy(&self) -> bool {
        self.tabs.iter().any(Tab::busy)
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

    /// After something was handled: pass on what the UIs asked of the
    /// session, and count the time it took.
    fn settle(&mut self, started: Instant) {
        self.tabs.forward_events();
        self.dirty = true;
        self.stats.update(started.elapsed());
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
                let at = (
                    self.metrics.origin(area, cols, rows),
                    self.metrics,
                    self.palette,
                );
                let model = chrome::Model::new(tui.pane_chrome(), frame.buffer, at);
                self.chrome.update(model, self.pointer.on_grid);
            }
            Err(e) => tracing::warn!(error = %e, "render failed"),
        }
        self.cursor = tab.terminal.backend().cursor();

        for tab in self.tabs.iter_mut() {
            if let Some(path) = tab.tui.take_editor_request() {
                self.effects.extend(open_editor(tab, path, now));
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

/// Hand a file to the user's editor for `tab`. Editors that need a
/// terminal can't run in a window, so those (and no editor at all) fall
/// back to the system's default app, and saving the file counts as
/// closing it.
fn open_editor(tab: &mut Tab, path: PathBuf, now: Instant) -> Option<Effect> {
    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .ok()
        .filter(|e| !e.trim().is_empty() && !app::is_terminal_editor(e));
    let Some(editor) = editor else {
        app::open_external(&path.display().to_string());
        tab.watch = Some(Watch {
            modified: app::modified(&path),
            path,
            checked: now,
        });
        return None;
    };
    Some(Effect::Edit {
        tab: tab.id,
        editor,
        path,
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
