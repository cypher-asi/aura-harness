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

mod state;

use std::collections::HashSet;
use std::path::Path;
use std::time::{Duration, Instant};

use super::file_view::FileView;
use super::{App, ChatTab, DetailView, NotificationType};
use crate::events::{LogRequest, LogStatus, LogView, UiEvent};
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

/// How long the chat stays blank waiting for its records before it says
/// it is loading, so a quick load shows no message and no fade at all.
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

    /// Whether the open LOG tab changes from frame to frame: what arrived
    /// is fading in, or the run on screen is still running so its spinner
    /// turns. A blank tab waiting for its log is still; the log's arrival
    /// asks for a frame of its own.
    #[must_use]
    pub fn log_animating(&self) -> bool {
        if !self.log.is_open() {
            return false;
        }
        if self.log.fading().is_some() {
            return true;
        }
        match self.log.shown(&self.active_agent_id) {
            None => false,
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
        self.open_log_on_switch(agent_id.clone());
        self.request_agent_switch(agent_id);
    }

    /// Show the LOG tab once the switch to `agent_id` lands.
    pub(super) fn open_log_on_switch(&mut self, agent_id: String) {
        self.log.pending_agent = Some(agent_id);
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
                self.select.copy_pending_text = Some(text);
                self.select.copy_label = Some(what.to_string());
                self.log.copying = true;
            }
            None => {
                self.notifications.current = Some((
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
mod tests;
