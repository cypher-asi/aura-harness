//! The chat panel's LOG tab: the run log of the agent on screen and of
//! every agent it spawned, as panels that fold open and shut. The run
//! asked for is always open: its details (metadata, problems and files on
//! disk), its turns and the runs below it. A new run opens on its last
//! turn; a turn on its steps; a step on its fields, thinking, reply, calls
//! and output. The runs of spawned agents sit under the step whose call
//! started them. `Copy log` puts the whole `log.json` on the clipboard.
//! While steps are being recorded the log reloads about once a second.
//!
//! Invariants:
//! - The state belongs to one agent ([`LogState::show_agent`]); switching
//!   parks it and brings back the other agent's, so the tab never shows
//!   the log of an agent that is not on screen.
//! - Coming onto the tab, or onto another agent, holds the log on hand
//!   back: its folds are kept, but nothing of it is drawn until the fresh
//!   log (or a notice) arrives, which then fades in. So the tab never
//!   flashes an out-of-date log or message before the real one.
//! - A live reload while a log is shown changes nothing on screen until
//!   the new log arrives; a notice then doesn't replace the shown log.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::{Duration, Instant};

use super::file_view::FileView;
use super::{App, ChatTab, DetailView, NotificationType};
use crate::events::{
    LogRequest, LogRunView, LogStatus, LogStepView, LogTurnView, LogView, UiEvent,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;

/// A panel of the LOG tab, by the run it is in.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum LogPanel {
    /// A subagent's run; the run asked for has no row of its own
    Run(String),
    /// A run's metadata, totals, problems and files on disk
    Details(String),
    /// A turn of a run
    Turn(String, u32),
    /// A step of a run
    Step(String, u64),
}

impl LogPanel {
    /// The run the panel is in.
    #[must_use]
    pub fn run_id(&self) -> &str {
        match self {
            Self::Run(run) | Self::Details(run) | Self::Turn(run, _) | Self::Step(run, _) => run,
        }
    }
}

/// One panel header on screen, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogRow {
    pub panel: LogPanel,
    /// How far it is indented
    pub depth: usize,
}

/// What the LOG tab showed for an agent, kept while another is on screen.
#[derive(Debug, Clone, Default)]
struct ParkedLog {
    view: Option<LogView>,
    notice: Option<String>,
    expanded: HashSet<LogPanel>,
    raw: HashSet<LogPanel>,
    selected: Option<LogPanel>,
    scroll: usize,
}

/// The LOG tab's state.
#[derive(Debug, Clone, Default)]
pub struct LogState {
    open: bool,
    /// The agent the state below belongs to
    agent: String,
    /// The log, once it arrived
    pub view: Option<LogView>,
    /// Why there is no log to show
    pub notice: Option<String>,
    /// When a log was asked for that hasn't arrived
    pub loading_since: Option<Instant>,
    /// When a log or a notice came on screen where nothing was shown, to
    /// fade it in
    pub revealed_at: Option<Instant>,
    /// The log on hand is from an earlier visit to the tab, or of the
    /// agent before a switch: kept for its folds, but not drawn until a
    /// fresh one arrives, so the tab never shows what may be out of date
    held: bool,
    /// Panels opened
    pub expanded: HashSet<LogPanel>,
    /// Steps shown as their JSON
    pub raw: HashSet<LogPanel>,
    pub selected: Option<LogPanel>,
    /// Lines scrolled down
    pub scroll: usize,
    /// Scroll the selected panel into sight on the next frame
    pub follow: bool,
    /// What other agents' tabs showed, the most recently parked last
    parked: Vec<(String, ParkedLog)>,
    /// The panel each line drawn last frame belongs to, from its top
    hits: Vec<(u16, LogPanel)>,
    /// Where `Copy log` was drawn
    copy_button: Option<Rect>,
    /// A step was recorded since the log was asked for
    stale: bool,
    /// When the log was last asked for
    requested: Option<Instant>,
    /// The agent whose log to show once the switch to it lands
    pending_agent: Option<String>,
    /// A copy from the tab waits for the next frame
    copying: bool,
    /// When a copy from the tab last went through
    copied_at: Option<Instant>,
}

/// How long `Copy log` says it copied.
const COPIED_FLASH: Duration = Duration::from_secs(2);

/// How often an open log is reloaded while steps are being recorded.
const LIVE_EVERY: Duration = Duration::from_secs(1);

/// How long a reload may stay unanswered before another is asked for.
const RELOAD_GIVE_UP: Duration = Duration::from_secs(10);

/// How long the tab stays blank waiting for a log before it says it is
/// loading, so a quick load shows no message at all.
pub const LOADING_DELAY: Duration = Duration::from_millis(150);

/// How long a log takes to fade in once it arrives.
pub const FADE_IN: Duration = Duration::from_millis(180);

/// Agents whose tab state is kept while another is on screen.
const PARKED_AGENTS: usize = 8;

/// How far something that came on screen at `revealed_at` has faded in,
/// as milliseconds of [`FADE_IN`], while it still is. Shared by the LOG
/// tab and the chat so both fade alike.
pub(crate) fn fade_progress(revealed_at: Option<Instant>) -> Option<(usize, usize)> {
    let elapsed = revealed_at?.elapsed();
    (elapsed < FADE_IN).then(|| {
        let whole = usize::try_from(FADE_IN.as_millis()).unwrap_or(usize::MAX);
        let part = usize::try_from(elapsed.as_millis()).unwrap_or(whole);
        (part.min(whole), whole.max(1))
    })
}

impl LogState {
    #[must_use]
    pub const fn is_open(&self) -> bool {
        self.open
    }

    /// Open or shut the tab. Coming onto it holds the log on hand back
    /// until a fresh one arrives.
    pub(super) fn set_open(&mut self, open: bool) {
        if open && !self.open {
            self.hold();
        }
        self.open = open;
    }

    /// Stop drawing the log on hand until the next one arrives.
    fn hold(&mut self) {
        self.held = self.view.is_some() || self.notice.is_some();
        self.revealed_at = None;
    }

    /// Make the state the one of `agent_id`: park the state of the agent
    /// it belonged to and bring back what `agent_id`'s tab showed, or
    /// start empty. Nothing changes when it already is `agent_id`'s.
    pub fn show_agent(&mut self, agent_id: &str) {
        if self.agent == agent_id {
            return;
        }
        let previous = std::mem::replace(&mut self.agent, agent_id.to_string());
        let coming = self.take_parked(agent_id).unwrap_or_default();
        let leaving = ParkedLog {
            view: self.view.take(),
            notice: self.notice.take(),
            expanded: std::mem::take(&mut self.expanded),
            raw: std::mem::take(&mut self.raw),
            selected: self.selected.take(),
            scroll: std::mem::take(&mut self.scroll),
        };
        self.parked.retain(|(id, _)| id != &previous);
        if leaving.view.is_some() || leaving.notice.is_some() {
            self.parked.push((previous, leaving));
        }
        if self.parked.len() > PARKED_AGENTS {
            let extra = self.parked.len() - PARKED_AGENTS;
            self.parked.drain(..extra);
        }
        self.view = coming.view;
        self.notice = coming.notice;
        self.expanded = coming.expanded;
        self.raw = coming.raw;
        self.selected = coming.selected;
        self.scroll = coming.scroll;
        self.follow = false;
        self.loading_since = None;
        self.hold();
    }

    /// The state parked for `agent_id`, taken out of the cache.
    fn take_parked(&mut self, agent_id: &str) -> Option<ParkedLog> {
        let at = self.parked.iter().position(|(id, _)| id == agent_id)?;
        Some(self.parked.remove(at).1)
    }

    /// The log to draw for `agent_id`: the loaded one, when it is that
    /// agent's and not held back for a fresh one.
    #[must_use]
    pub fn shown(&self, agent_id: &str) -> Option<&LogView> {
        self.view
            .as_ref()
            .filter(|v| !self.held && v.agent_id == agent_id)
    }

    /// Whether the log or notice on hand waits for a fresh one before it
    /// is drawn.
    #[must_use]
    pub const fn is_held(&self) -> bool {
        self.held
    }

    /// Whether a log was asked for and hasn't arrived.
    #[must_use]
    pub const fn is_loading(&self) -> bool {
        self.loading_since.is_some()
    }

    /// Whether a log has been awaited long enough to say it is loading.
    #[must_use]
    pub fn loading_shown(&self) -> bool {
        self.loading_since
            .is_some_and(|at| at.elapsed() >= LOADING_DELAY)
    }

    /// How far a log arriving has faded in, as milliseconds of
    /// [`FADE_IN`], while it still is.
    #[must_use]
    pub fn fading(&self) -> Option<(usize, usize)> {
        fade_progress(self.revealed_at)
    }

    /// A step was recorded: reload the log if it is open.
    pub(super) fn mark_stale(&mut self) {
        self.stale |= self.open;
    }

    /// Whether to reload the log now: it is open, a step was recorded
    /// since it was asked for, and it was asked for long enough ago.
    fn reload_due(&self) -> bool {
        let since = self.requested.map(|at| at.elapsed());
        self.open
            && self.stale
            && since.is_none_or(|since| {
                since >= LIVE_EVERY && (!self.is_loading() || since >= RELOAD_GIVE_UP)
            })
    }

    /// The run with id `run_id`.
    #[must_use]
    pub fn run(&self, run_id: &str) -> Option<&LogRunView> {
        self.view.as_ref()?.runs.iter().find(|r| r.run_id == run_id)
    }

    /// Turn `turn` of run `run_id`.
    #[must_use]
    pub fn turn(&self, run_id: &str, turn: u32) -> Option<&LogTurnView> {
        self.run(run_id)?.turns.iter().find(|t| t.turn == turn)
    }

    /// Step `seq` of run `run_id`.
    #[must_use]
    pub fn step(&self, run_id: &str, seq: u64) -> Option<&LogStepView> {
        self.run(run_id)?
            .turns
            .iter()
            .flat_map(|t| &t.steps)
            .find(|s| s.seq == seq)
    }

    /// Whether `panel` is open. The run asked for always is.
    #[must_use]
    pub fn is_expanded(&self, panel: &LogPanel) -> bool {
        self.is_root(panel) || self.expanded.contains(panel)
    }

    /// Whether `panel` is the run asked for, which has no row.
    fn is_root(&self, panel: &LogPanel) -> bool {
        matches!(panel, LogPanel::Run(id)
            if self.view.as_ref().and_then(LogView::root).is_some_and(|r| &r.run_id == id))
    }

    /// Whether `panel` has anything to open.
    #[must_use]
    pub fn opens(&self, panel: &LogPanel) -> bool {
        match panel {
            LogPanel::Run(_) | LogPanel::Details(_) => true,
            LogPanel::Turn(run, turn) => self.turn(run, *turn).is_some_and(|t| !t.steps.is_empty()),
            LogPanel::Step(run, seq) => self
                .step(run, *seq)
                .is_some_and(|s| !s.fields.is_empty() || !s.sections.is_empty()),
        }
    }

    /// The panel headers on screen, top to bottom. The run asked for has
    /// no row: its panels are the top level.
    #[must_use]
    pub fn rows(&self) -> Vec<LogRow> {
        let mut rows = Vec::new();
        if let Some(view) = &self.view {
            if let Some(root) = view.root() {
                let mut seen = HashSet::from([0]);
                self.push_contents(view, root, 0, &mut rows, &mut seen);
            }
        }
        rows
    }

    /// The row of run `index` of `view` at `depth`, and its panels below
    /// it when it is open.
    fn push_run(
        &self,
        view: &LogView,
        index: usize,
        depth: usize,
        rows: &mut Vec<LogRow>,
        seen: &mut HashSet<usize>,
    ) {
        let Some(run) = view.runs.get(index) else {
            return;
        };
        if !seen.insert(index) {
            return;
        }
        let panel = LogPanel::Run(run.run_id.clone());
        let open = self.is_expanded(&panel);
        rows.push(LogRow { panel, depth });
        if open {
            self.push_contents(view, run, depth + 1, rows, seen);
        }
    }

    /// The panels of `run` at `depth`: its details, its turns with their
    /// steps, and the runs below it.
    fn push_contents(
        &self,
        view: &LogView,
        run: &LogRunView,
        depth: usize,
        rows: &mut Vec<LogRow>,
        seen: &mut HashSet<usize>,
    ) {
        let id = &run.run_id;
        rows.push(LogRow {
            panel: LogPanel::Details(id.clone()),
            depth,
        });
        for turn in &run.turns {
            let panel = LogPanel::Turn(id.clone(), turn.turn);
            let open = self.is_expanded(&panel);
            rows.push(LogRow { panel, depth });
            if !open {
                continue;
            }
            for step in &turn.steps {
                rows.push(LogRow {
                    panel: LogPanel::Step(id.clone(), step.seq),
                    depth: depth + 1,
                });
                for &child in &step.spawned {
                    self.push_run(view, child, depth + 2, rows, seen);
                }
            }
            for &child in &turn.spawned {
                self.push_run(view, child, depth + 1, rows, seen);
            }
        }
        for &child in &run.runs {
            self.push_run(view, child, depth, rows, seen);
        }
    }

    /// Take `view` in: a new run opens on its last turn, its details
    /// shut; the same run again keeps what was open and opens a new turn.
    /// A log coming where none was drawn, or one held back, fades in.
    fn load(&mut self, view: LogView) {
        self.loading_since = None;
        self.notice = None;
        if self.shown(&view.agent_id).is_none() {
            self.revealed_at = Some(Instant::now());
        }
        self.held = false;
        self.agent.clone_from(&view.agent_id);
        let same = self
            .view
            .as_ref()
            .and_then(LogView::root)
            .zip(view.root())
            .is_some_and(|(old, new)| old.run_id == new.run_id);
        let last_turn = |view: &LogView| view.root().and_then(|r| r.turns.last()).map(|t| t.turn);
        let rows = self.rows();
        let at_end = same
            && self.selected.is_some()
            && rows.last().map(|r| &r.panel) == self.selected.as_ref();
        if same {
            let before = self.view.as_ref().and_then(last_turn);
            if let (Some(root), Some(turn)) = (view.root(), last_turn(&view)) {
                if before != Some(turn) {
                    self.expanded
                        .insert(LogPanel::Turn(root.run_id.clone(), turn));
                }
            }
        } else {
            self.expanded.clear();
            self.raw.clear();
            self.scroll = 0;
            self.selected = None;
            if let (Some(root), Some(turn)) = (view.root(), last_turn(&view)) {
                self.expanded
                    .insert(LogPanel::Turn(root.run_id.clone(), turn));
            }
        }
        self.view = Some(view);
        if at_end {
            // Watching the newest step: keep watching as steps come in.
            let rows = self.rows();
            self.select_at(&rows, rows.len().saturating_sub(1));
        }
        self.keep_selection();
    }

    /// Keep the selection on a panel that is on screen.
    fn keep_selection(&mut self) {
        let rows = self.rows();
        let shown = self
            .selected
            .as_ref()
            .is_some_and(|s| rows.iter().any(|r| &r.panel == s));
        if !shown {
            self.selected = rows.first().map(|r| r.panel.clone());
        }
    }

    fn selected_index(&self, rows: &[LogRow]) -> usize {
        self.selected
            .as_ref()
            .and_then(|s| rows.iter().position(|r| &r.panel == s))
            .unwrap_or(0)
    }

    fn select_at(&mut self, rows: &[LogRow], index: usize) {
        if let Some(row) = rows.get(index.min(rows.len().saturating_sub(1))) {
            self.selected = Some(row.panel.clone());
            self.follow = true;
        }
    }

    fn move_by(&mut self, delta: isize) {
        let rows = self.rows();
        let at = self.selected_index(&rows);
        let to = at.saturating_add_signed(delta);
        self.select_at(&rows, to);
    }

    /// Open or shut `panel`; the run asked for stays open.
    pub(crate) fn toggle(&mut self, panel: &LogPanel) {
        if self.is_root(panel) {
            return;
        }
        if !self.expanded.remove(panel) && self.opens(panel) {
            self.expanded.insert(panel.clone());
        }
        self.keep_selection();
    }

    /// Open the selected panel, else go to its first panel inside.
    fn open_or_enter(&mut self) {
        let Some(panel) = self.selected.clone() else {
            return;
        };
        if self.opens(&panel) && !self.is_expanded(&panel) {
            self.expanded.insert(panel);
        } else {
            self.move_by(1);
        }
    }

    /// Shut the selected panel, else go to the panel it is in.
    fn close_or_leave(&mut self) {
        let Some(panel) = self.selected.clone() else {
            return;
        };
        if !self.is_root(&panel) && self.expanded.remove(&panel) {
            return;
        }
        let rows = self.rows();
        let at = self.selected_index(&rows);
        let depth = rows.get(at).map_or(0, |r| r.depth);
        let parent = rows
            .get(..at)
            .and_then(|above| above.iter().rposition(|r| r.depth < depth));
        if let Some(parent) = parent {
            self.select_at(&rows, parent);
        }
    }

    /// Open every panel, the runs of every subagent included.
    fn expand_all(&mut self) {
        let Some(view) = &self.view else {
            return;
        };
        let mut all = Vec::new();
        for run in &view.runs {
            let id = &run.run_id;
            all.push(LogPanel::Run(id.clone()));
            all.push(LogPanel::Details(id.clone()));
            for turn in &run.turns {
                all.push(LogPanel::Turn(id.clone(), turn.turn));
                for step in &turn.steps {
                    all.push(LogPanel::Step(id.clone(), step.seq));
                }
            }
        }
        self.expanded.extend(all);
    }

    /// Shut every panel; the run asked for stays open.
    fn collapse_all(&mut self) {
        self.expanded.clear();
        self.keep_selection();
        self.follow = true;
    }

    /// The JSON of `panel`: a step's, a turn's, or its run's.
    #[must_use]
    pub fn json_of(&self, panel: &LogPanel) -> Option<&str> {
        match panel {
            LogPanel::Step(run, seq) => self.step(run, *seq).map(|s| s.json.as_str()),
            LogPanel::Turn(run, turn) => self.turn(run, *turn).map(|t| t.json.as_str()),
            other => self.run(other.run_id()).map(|r| r.json.as_str()),
        }
    }

    /// Scroll by `lines`, down when positive.
    pub(super) fn scroll_by(&mut self, lines: isize) {
        self.scroll = self.scroll.saturating_add_signed(lines);
        self.follow = false;
    }

    /// Where the lines of the last frame were and what they belong to.
    pub(crate) fn set_hits(&mut self, hits: Vec<(u16, LogPanel)>, copy_button: Option<Rect>) {
        self.hits = hits;
        self.copy_button = copy_button;
    }

    /// Where `Copy log` was drawn last frame.
    #[must_use]
    pub const fn copy_button_area(&self) -> Option<Rect> {
        self.copy_button
    }

    /// A copy from the tab went through (`ok`) or failed.
    pub(super) fn copy_finished(&mut self, ok: bool) {
        if std::mem::take(&mut self.copying) && ok {
            self.copied_at = Some(Instant::now());
        }
    }

    /// Whether a copy from the tab went through a moment ago.
    #[must_use]
    pub fn just_copied(&self) -> bool {
        self.copied_at.is_some_and(|at| at.elapsed() < COPIED_FLASH)
    }

    fn panel_at(&self, row: u16) -> Option<LogPanel> {
        self.hits
            .iter()
            .find(|(y, _)| *y == row)
            .map(|(_, panel)| panel.clone())
    }

    /// Index of the run with id `run_id` in the view.
    #[must_use]
    pub fn run_indices(&self) -> HashMap<&str, usize> {
        self.view.as_ref().map_or_else(HashMap::new, |view| {
            view.runs
                .iter()
                .enumerate()
                .map(|(i, r)| (r.run_id.as_str(), i))
                .collect()
        })
    }
}

impl App {
    /// The LOG tab's state.
    #[must_use]
    pub const fn run_log(&self) -> &LogState {
        &self.log
    }

    pub(crate) fn run_log_mut(&mut self) -> &mut LogState {
        &mut self.log
    }

    /// Ask for the log of the agent on screen. Nothing on screen changes
    /// until it arrives.
    pub(super) fn request_log(&mut self) {
        self.log.loading_since = Some(Instant::now());
        self.log.stale = false;
        self.log.requested = Some(Instant::now());
        if let Some(tx) = &self.event_tx {
            let _ = tx.try_send(UiEvent::Log {
                agent_id: self.active_agent_id.clone(),
                request: LogRequest::Load,
            });
        }
    }

    /// Whether the open LOG tab changes from frame to frame: it waits
    /// for a log with none shown, a log is fading in, or the run on
    /// screen is still running so its spinner turns.
    #[must_use]
    pub fn log_animating(&self) -> bool {
        if !self.log.is_open() {
            return false;
        }
        if self.log.fading().is_some() {
            return true;
        }
        match self.log.shown(&self.active_agent_id) {
            None => self.log.is_loading(),
            Some(view) => view
                .root()
                .is_some_and(|root| root.status == LogStatus::Running),
        }
    }

    /// Reload the open log when steps were recorded since it was asked
    /// for, at most every [`LIVE_EVERY`].
    pub(super) fn refresh_live_log(&mut self) {
        if self.log.reload_due() {
            self.request_log();
        }
    }

    /// Show the LOG tab of the agent `agent_id`, switching to it first
    /// when another agent is on screen.
    pub(super) fn open_log_of(&mut self, agent_id: String) {
        if agent_id == self.active_agent_id {
            self.show_chat_tab(ChatTab::Log);
            return;
        }
        self.log.pending_agent = Some(agent_id.clone());
        self.request_agent_switch(agent_id);
    }

    /// Whether the switch to `agent_id` was asked for to show its log.
    /// Any switch ends the wait, so a later one doesn't open it.
    pub(super) fn log_wanted_for(&mut self, agent_id: &str) -> bool {
        self.log.pending_agent.take().as_deref() == Some(agent_id)
    }

    pub(super) fn cmd_log_view(&mut self, view: LogView) {
        if view.agent_id == self.active_agent_id {
            self.log.show_agent(&view.agent_id);
            self.log.load(view);
        }
    }

    /// Why the log of `agent_id` can't be shown. It replaces what the tab
    /// shows only when no log is shown: a reload that failed leaves the
    /// log on screen, and one held back comes back rather than vanish.
    pub(super) fn cmd_log_notice(&mut self, agent_id: &str, notice: String) {
        if agent_id != self.active_agent_id {
            return;
        }
        self.log.show_agent(agent_id);
        if self.log.shown(agent_id).is_none() {
            self.log.revealed_at = Some(Instant::now());
        }
        self.log.held = false;
        self.log.loading_since = None;
        self.log.notice = Some(notice);
    }

    /// Copy `text` to the clipboard after the next frame, saying `what`
    /// was copied.
    fn copy_text(&mut self, text: Option<String>, what: &str) {
        match text.filter(|t| !t.is_empty()) {
            Some(text) => {
                self.copy_pending_text = Some(text);
                self.copy_label = Some(what.to_string());
                self.log.copying = true;
            }
            None => {
                self.notification = Some((
                    "There is no log to copy yet".to_string(),
                    NotificationType::Warning,
                ));
            }
        }
    }

    /// Copy the whole `log.json`: the run of the agent on screen and of
    /// every agent below it.
    pub(super) fn copy_whole_log(&mut self) {
        let json = self.log.view.as_ref().map(|v| v.json.clone());
        self.copy_text(json, "the log");
    }

    fn copy_selected_log(&mut self) {
        let Some(panel) = self.log.selected.clone() else {
            return;
        };
        let what = match &panel {
            LogPanel::Step(_, seq) => format!("step {seq}"),
            LogPanel::Turn(_, turn) => format!("turn {turn}"),
            _ => "the run".to_string(),
        };
        let json = self.log.json_of(&panel).map(str::to_string);
        self.copy_text(json, &what);
    }

    fn open_log_file(&mut self) {
        let Some(path) = self.log.view.as_ref().map(|v| v.path.clone()) else {
            return;
        };
        let path = Path::new(&path);
        let view = FileView::load(path, path.parent());
        self.open_detail(DetailView::File(Box::new(view)));
    }

    /// A key in the LOG tab with nothing typed. Returns whether it used
    /// the key.
    pub(super) fn handle_log_key(&mut self, key: KeyEvent) -> bool {
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return false;
        }
        match key.code {
            KeyCode::Esc => self.show_chat_tab(ChatTab::Chat),
            KeyCode::Up | KeyCode::Char('k') => self.log.move_by(-1),
            KeyCode::Down | KeyCode::Char('j') => self.log.move_by(1),
            KeyCode::PageUp => self.log.scroll_by(-10),
            KeyCode::PageDown => self.log.scroll_by(10),
            KeyCode::Home | KeyCode::Char('g') => {
                let rows = self.log.rows();
                self.log.select_at(&rows, 0);
            }
            KeyCode::End | KeyCode::Char('G') => {
                let rows = self.log.rows();
                self.log.select_at(&rows, rows.len().saturating_sub(1));
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                if let Some(panel) = self.log.selected.clone() {
                    self.log.toggle(&panel);
                }
            }
            KeyCode::Right | KeyCode::Char('l') => self.log.open_or_enter(),
            KeyCode::Left | KeyCode::Char('h') => self.log.close_or_leave(),
            KeyCode::Char('e') => self.log.expand_all(),
            KeyCode::Char('E') => self.log.collapse_all(),
            KeyCode::Char('J') => {
                if let Some(panel @ LogPanel::Step(..)) = self.log.selected.clone() {
                    if !self.log.raw.remove(&panel) {
                        self.log.raw.insert(panel.clone());
                        self.log.expanded.insert(panel);
                    }
                }
            }
            KeyCode::Char('c') => self.copy_whole_log(),
            KeyCode::Char('y') => self.copy_selected_log(),
            KeyCode::Char('p') => {
                let path = self.log.view.as_ref().map(|v| v.path.clone());
                self.copy_text(path, "the log's path");
            }
            KeyCode::Char('o') => self.open_log_file(),
            KeyCode::Char('r') => self.request_log(),
            _ => return false,
        }
        true
    }

    /// A click in the LOG tab: `Copy log` copies the log, a panel's
    /// header opens or shuts it. Returns whether it hit either.
    pub(crate) fn click_log(&mut self, column: u16, row: u16) -> bool {
        if super::contains(self.log.copy_button, column, row) {
            self.copy_whole_log();
            return true;
        }
        let Some(panel) = self.log.panel_at(row) else {
            return false;
        };
        self.log.selected = Some(panel.clone());
        self.log.toggle(&panel);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{LogRunView, LogStatus, LogStepView, LogTurnView};

    fn step(seq: u64, spawned: Vec<usize>) -> LogStepView {
        LogStepView {
            seq,
            kind: "model".into(),
            label: format!("step {seq}"),
            fields: vec![("seq".into(), seq.to_string())],
            spawned,
            json: format!("{{\"seq\":{seq}}}"),
            ..LogStepView::default()
        }
    }

    fn run(id: &str, depth: usize, turns: Vec<LogTurnView>) -> LogRunView {
        LogRunView {
            run_id: id.into(),
            title: id.into(),
            depth,
            turns,
            json: format!("{{\"run\":\"{id}\"}}"),
            ..LogRunView::default()
        }
    }

    fn turn(n: u32, steps: Vec<LogStepView>) -> LogTurnView {
        LogTurnView {
            turn: n,
            steps,
            status: LogStatus::Ok,
            json: format!("{{\"turn\":{n}}}"),
            ..LogTurnView::default()
        }
    }

    fn view() -> LogView {
        LogView {
            agent_id: "a".into(),
            path: "/tmp/runs/r/log.json".into(),
            json: "{\"whole\":true}".into(),
            runs: vec![
                run(
                    "root",
                    0,
                    vec![
                        turn(1, vec![step(1, vec![]), step(2, vec![1])]),
                        turn(2, vec![step(5, vec![])]),
                    ],
                ),
                run("child", 1, vec![turn(1, vec![step(1, vec![])])]),
            ],
        }
    }

    fn panels(state: &LogState) -> Vec<LogPanel> {
        state.rows().into_iter().map(|r| r.panel).collect()
    }

    #[test]
    fn a_new_log_opens_on_its_last_turn_with_its_details_shut() {
        let mut state = LogState::default();
        state.load(view());
        assert_eq!(
            panels(&state),
            vec![
                LogPanel::Details("root".into()),
                LogPanel::Turn("root".into(), 1),
                LogPanel::Turn("root".into(), 2),
                LogPanel::Step("root".into(), 5),
            ],
            "the run asked for has no row of its own"
        );
        assert_eq!(state.selected, Some(LogPanel::Details("root".into())));
    }

    #[test]
    fn a_subagent_sits_under_the_step_that_spawned_it() {
        let mut state = LogState::default();
        state.load(view());
        state.toggle(&LogPanel::Turn("root".into(), 1));
        let rows = state.rows();
        let at = rows
            .iter()
            .position(|r| r.panel == LogPanel::Step("root".into(), 2))
            .unwrap();
        assert_eq!(rows[at + 1].panel, LogPanel::Run("child".into()));
        assert!(rows[at + 1].depth > rows[at].depth);
        state.expand_all();
        assert!(panels(&state).contains(&LogPanel::Step("child".into(), 1)));
    }

    #[test]
    fn runs_no_turn_started_follow_the_turns() {
        let mut group = run("agent-a", 0, vec![]);
        group.runs = vec![1, 2];
        let mut state = LogState::default();
        state.load(LogView {
            runs: vec![group, run("a-1", 1, vec![]), run("b-1", 1, vec![])],
            ..view()
        });
        let rows = state.rows();
        let listed: Vec<(&LogPanel, usize)> = rows.iter().map(|r| (&r.panel, r.depth)).collect();
        let (a, b) = (LogPanel::Run("a-1".into()), LogPanel::Run("b-1".into()));
        assert_eq!(&listed[listed.len() - 2..], [(&a, 0), (&b, 0)]);
    }

    #[test]
    fn closing_a_panel_keeps_the_selection_on_screen() {
        let mut state = LogState::default();
        state.load(view());
        state.selected = Some(LogPanel::Step("root".into(), 5));
        state.toggle(&LogPanel::Turn("root".into(), 2));
        let shown = panels(&state);
        assert!(!shown.contains(&LogPanel::Step("root".into(), 5)));
        assert!(shown.contains(state.selected.as_ref().unwrap()));
    }

    #[test]
    fn the_run_asked_for_stays_open() {
        let mut state = LogState::default();
        state.load(view());
        let before = panels(&state);
        state.toggle(&LogPanel::Run("root".into()));
        state.collapse_all();
        assert!(state.is_expanded(&LogPanel::Run("root".into())));
        assert_eq!(panels(&state)[0], before[0]);
    }

    #[test]
    fn reloading_the_same_run_keeps_what_was_open() {
        let mut state = LogState::default();
        state.load(view());
        state.toggle(&LogPanel::Details("root".into()));
        let mut next = view();
        next.runs[0].turns.push(turn(3, vec![step(9, vec![])]));
        state.load(next);
        assert!(state.is_expanded(&LogPanel::Details("root".into())));
        assert!(
            state.is_expanded(&LogPanel::Turn("root".into(), 3)),
            "a new turn opens"
        );
    }

    #[test]
    fn copy_log_copies_the_whole_json() {
        let mut app = App::new();
        app.log.load(view());
        app.copy_whole_log();
        let buffer = ratatui::buffer::Buffer::empty(Rect::new(0, 0, 1, 1));
        assert_eq!(
            app.take_copy_text(&buffer).as_deref(),
            Some("{\"whole\":true}")
        );
        app.log.selected = Some(LogPanel::Turn("root".into(), 2));
        app.copy_selected_log();
        assert_eq!(app.take_copy_text(&buffer).as_deref(), Some("{\"turn\":2}"));
    }

    #[test]
    fn the_log_of_another_agent_is_ignored() {
        let mut app = App::new();
        app.active_agent_id = "b".into();
        app.cmd_log_view(view());
        assert!(app.log.view.is_none());
    }

    /// The agents whose logs were asked for since the last call.
    fn logs_asked(rx: &mut tokio::sync::mpsc::Receiver<UiEvent>) -> Vec<String> {
        std::iter::from_fn(|| rx.try_recv().ok())
            .filter_map(|event| match event {
                UiEvent::Log { agent_id, .. } => Some(agent_id),
                _ => None,
            })
            .collect()
    }

    fn a_second_ago() -> Option<Instant> {
        Instant::now().checked_sub(LIVE_EVERY)
    }

    #[test]
    fn a_recorded_step_reloads_the_open_log_at_most_once_a_second() {
        use crate::events::UiCommand;
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let mut app = App::new().with_event_sender(tx);
        app.active_agent_id = "a".into();
        app.process_command(UiCommand::GraphUpsert {
            run_id: "root".into(),
            nodes: Vec::new(),
        });
        app.refresh_live_log();
        assert!(logs_asked(&mut rx).is_empty(), "the tab is shut");

        app.show_chat_tab(ChatTab::Log);
        app.cmd_log_view(view());
        assert_eq!(logs_asked(&mut rx), ["a"]);
        app.process_command(UiCommand::GraphUpsert {
            run_id: "root".into(),
            nodes: Vec::new(),
        });
        app.refresh_live_log();
        assert!(logs_asked(&mut rx).is_empty(), "not again within a second");
        app.log.requested = a_second_ago();
        app.refresh_live_log();
        assert_eq!(logs_asked(&mut rx), ["a"]);

        app.cmd_log_view(view());
        app.log.requested = a_second_ago();
        app.refresh_live_log();
        assert!(logs_asked(&mut rx).is_empty(), "nothing new was recorded");
    }

    #[test]
    fn watching_the_newest_step_follows_the_steps_that_come_in() {
        let mut state = LogState::default();
        state.load(view());
        state.selected = Some(LogPanel::Step("root".into(), 5));
        let mut next = view();
        next.runs[0].turns[1].steps.push(step(6, vec![]));
        state.load(next);
        assert_eq!(state.selected, Some(LogPanel::Step("root".into(), 6)));

        state.selected = Some(LogPanel::Turn("root".into(), 2));
        let mut next = view();
        next.runs[0].turns[1].steps.push(step(7, vec![]));
        state.load(next);
        assert_eq!(state.selected, Some(LogPanel::Turn("root".into(), 2)));
    }

    #[test]
    fn the_log_of_another_agent_opens_once_the_switch_to_it_lands() {
        use crate::events::UiCommand;
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let mut app = App::new().with_event_sender(tx);
        app.active_agent_id = "a".into();
        app.open_log_of("b".into());
        assert!(matches!(rx.try_recv(), Ok(UiEvent::SelectAgent(id)) if id == "b"));
        assert_ne!(app.chat_tab(), ChatTab::Log);
        app.process_command(UiCommand::SetActiveAgent("b".into()));
        assert_eq!(app.chat_tab(), ChatTab::Log);
        assert_eq!(logs_asked(&mut rx), ["b"]);

        app.show_chat_tab(ChatTab::Chat);
        app.process_command(UiCommand::SetActiveAgent("a".into()));
        assert_eq!(
            app.chat_tab(),
            ChatTab::Chat,
            "the wait ended with the switch"
        );
    }

    #[test]
    fn switching_agents_parks_the_log_and_brings_it_back() {
        use crate::events::UiCommand;
        let (tx, _rx) = tokio::sync::mpsc::channel(32);
        let mut app = App::new().with_event_sender(tx);
        app.process_command(UiCommand::SetActiveAgent("a".into()));
        app.show_chat_tab(ChatTab::Log);
        app.cmd_log_view(view());
        app.log.selected = Some(LogPanel::Step("root".into(), 5));

        app.process_command(UiCommand::SetActiveAgent("b".into()));
        assert!(app.log.view.is_none(), "a's log is not b's");
        assert!(app.log.shown("b").is_none());

        app.process_command(UiCommand::SetActiveAgent("a".into()));
        assert!(app.log.is_held(), "a's old log waits for the fresh one");
        assert!(app.log.shown("a").is_none(), "nothing old is drawn");
        app.cmd_log_view(view());
        assert!(app.log.shown("a").is_some());
        assert_eq!(
            app.log.selected,
            Some(LogPanel::Step("root".into(), 5)),
            "the fresh log keeps a's folds and selection"
        );
        assert!(app.log.fading().is_some(), "and fades in");
    }

    #[test]
    fn coming_back_to_the_tab_hides_the_old_log_until_the_fresh_one() {
        let (tx, _rx) = tokio::sync::mpsc::channel(32);
        let mut app = App::new().with_event_sender(tx);
        app.active_agent_id = "a".into();
        app.show_chat_tab(ChatTab::Log);
        app.cmd_log_view(view());
        app.log.revealed_at = None;

        app.show_chat_tab(ChatTab::Chat);
        app.show_chat_tab(ChatTab::Log);
        assert!(app.log.shown("a").is_none(), "the old log is not flashed");
        assert!(app.log.is_loading());

        app.cmd_log_view(view());
        assert!(app.log.shown("a").is_some());
        assert!(app.log.fading().is_some());

        // A live reload on the open tab keeps the log on screen.
        app.request_log();
        assert!(app.log.shown("a").is_some());
    }

    #[test]
    fn an_old_notice_is_not_shown_while_the_log_is_asked_for_again() {
        let (tx, _rx) = tokio::sync::mpsc::channel(32);
        let mut app = App::new().with_event_sender(tx);
        app.active_agent_id = "a".into();
        app.show_chat_tab(ChatTab::Log);
        app.cmd_log_notice("a", "no run log yet".into());
        app.show_chat_tab(ChatTab::Chat);
        app.show_chat_tab(ChatTab::Log);
        assert!(app.log.is_held() && app.log.is_loading());
    }

    #[test]
    fn a_notice_during_a_reload_keeps_the_log_on_screen() {
        let mut app = App::new();
        app.active_agent_id = "a".into();
        app.cmd_log_view(view());
        app.request_log();
        app.cmd_log_notice("a", "could not read the run".into());
        assert!(app.log.shown("a").is_some());
        assert!(!app.log.is_loading());
    }

    #[test]
    fn a_slow_log_says_it_is_loading_only_after_a_moment() {
        let mut state = LogState {
            loading_since: Some(Instant::now()),
            ..LogState::default()
        };
        assert!(state.is_loading() && !state.loading_shown());
        state.loading_since = Instant::now().checked_sub(LOADING_DELAY);
        assert!(state.loading_shown());
    }

    #[test]
    fn a_log_fades_in_only_where_none_was_shown() {
        let mut state = LogState::default();
        state.load(view());
        assert!(state.fading().is_some(), "the first log fades in");
        state.revealed_at = None;
        state.load(view());
        assert!(state.revealed_at.is_none(), "a reload doesn't fade");
    }

    #[test]
    fn the_tab_animates_while_it_waits_or_fades() {
        let mut app = App::new();
        app.active_agent_id = "a".into();
        assert!(!app.log_animating(), "the tab is shut");
        app.show_chat_tab(ChatTab::Log);
        assert!(app.log_animating(), "waiting for the log");
        app.cmd_log_view(view());
        assert!(app.log_animating(), "fading in");
        app.log.revealed_at = Instant::now().checked_sub(FADE_IN);
        assert!(!app.log_animating(), "a finished run on screen is still");
    }

    #[test]
    fn l_in_a_benchmark_shows_the_log_of_the_picked_run() {
        use crate::events::{BenchArm, BenchRun, BenchView, UiCommand};
        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        let mut app = App::new().with_event_sender(tx);
        app.active_agent_id = "a".into();
        app.process_command(UiCommand::BenchList {
            agent_id: "a".into(),
            root: String::new(),
            benches: vec![BenchView {
                id: "ab".into(),
                name: "Compare #1".into(),
                arms: [BenchArm::default(), BenchArm::default()],
                runs: vec![BenchRun {
                    name: "a-1".into(),
                    agent_id: "run".into(),
                    ..BenchRun::default()
                }],
                ..BenchView::default()
            }],
        });
        app.show_chat_tab(ChatTab::Bench);
        app.handle_key(KeyEvent::from(KeyCode::Enter));
        app.handle_key(KeyEvent::from(KeyCode::Right));
        while rx.try_recv().is_ok() {}
        app.handle_key(KeyEvent::new(KeyCode::Char('L'), KeyModifiers::SHIFT));
        assert!(matches!(rx.try_recv(), Ok(UiEvent::SelectAgent(id)) if id == "run"));
    }
}
