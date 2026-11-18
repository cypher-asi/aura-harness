//! One tab of the window: a workspace with its own projects and agents,
//! and its own copy of the terminal UI to show them in, with its own
//! input draft, panels, overlays and scroll position.

use crate::grid::GridBackend;
use aura_ui::events::{AgentSummary, UiEvent};
use std::path::PathBuf;
use std::time::{Instant, SystemTime};
use tokio::sync::mpsc;

/// Events a tab's UI can queue between two updates of the window.
const EVENT_QUEUE: usize = 256;

/// What every tab's UI is created with.
#[derive(Debug, Clone, Copy)]
pub struct Settings {
    pub verbose: bool,
    pub alerts: aura_ui_terminal::AlertSettings,
    /// How wide chat prose is in the proportional face, in cells: asked
    /// of the fonts of whichever renderer draws the window
    pub measure: aura_ui_terminal::ProseMeasure,
}

/// A file the user is editing in an app that doesn't report closing; it
/// counts as done once it's saved.
pub struct Watch {
    pub path: PathBuf,
    pub modified: Option<SystemTime>,
    pub checked: Instant,
}

pub struct Tab {
    /// Stays the same while tabs around it open and close.
    pub id: u64,
    pub tui: aura_ui_terminal::App,
    pub terminal: ratatui::Terminal<GridBackend>,
    pub watch: Option<Watch>,
    /// Hex ids of the root agents heading the projects in this tab. The
    /// agents under a root belong to the tab with it.
    pub projects: Vec<String>,
    /// What the UI asked of the session since the last update.
    events: mpsc::Receiver<UiEvent>,
}

impl Tab {
    pub fn new(id: u64, settings: Settings) -> Self {
        let (sender, events) = mpsc::channel(EVENT_QUEUE);
        let mut tui = aura_ui_terminal::App::new()
            .with_event_sender(sender)
            .with_alert_settings(settings.alerts);
        tui.set_host_chrome(true);
        tui.set_prose_measure(settings.measure);
        tui.set_smooth_scroll(true);
        if settings.verbose {
            tui.set_verbose(true);
        }
        Self {
            id,
            tui,
            terminal: ratatui::Terminal::new(GridBackend::new(1, 1))
                .expect("the grid backend never fails"),
            watch: None,
            projects: Vec::new(),
            events,
        }
    }

    /// Hex id of the agent the tab shows; empty until the session has
    /// shown one in it.
    pub fn agent(&self) -> &str {
        self.tui.active_agent_id()
    }

    /// The next event the UI queued for the session, if any.
    pub fn next_event(&mut self) -> Option<UiEvent> {
        self.events.try_recv().ok()
    }

    /// What the tab is showing, for the title box of the tab bar: the
    /// project in capitals, then the agent when it isn't the one heading
    /// an unnamed project. Empty until the tab shows an agent.
    pub fn title(&self) -> String {
        let agents = self.tui.agents();
        let Some(index) = agents.iter().position(|a| a.id == self.agent()) else {
            return String::new();
        };
        let root = agents[..=index]
            .iter()
            .rfind(|agent| agent.heads_project())
            .unwrap_or(&agents[index]);
        let named = |text: &str| Some(text.trim().to_uppercase()).filter(|text| !text.is_empty());
        let agent = named(&agents[index].name);
        match root.project.as_deref().and_then(named) {
            Some(project) => match agent {
                Some(agent) => format!("{project} / {agent}"),
                None => project,
            },
            None if root.id == agents[index].id => agent.unwrap_or_default(),
            None => [named(&root.name), agent]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" / "),
        }
    }

    /// Whether an agent of the tab is working or waiting for an approval.
    pub fn busy(&self) -> bool {
        let working = |agent: &AgentSummary| agent.running || self.tui.agent_busy(&agent.id);
        self.tui.agents().iter().any(working)
    }
}
