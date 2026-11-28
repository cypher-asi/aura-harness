//! Switching which agent is on screen: the chats and drafts of the agents
//! not shown, and how far the chat on screen is in getting its history.

use std::collections::{HashMap, VecDeque};
use std::time::Instant;

use super::{agent_views, queue_panel, side_scope};

/// Agents most recently on screen whose chats are kept whole off screen,
/// so going back and forth between a few costs no reload (see
/// `agent_views::shed`).
pub(crate) const KEPT_VIEWS: usize = 4;

/// The state kept while switching which agent is on screen.
pub(crate) struct AgentSwitch {
    /// Chats of the agents not on screen, by agent ID
    pub(crate) views: HashMap<String, agent_views::AgentView>,
    /// Whether the chat on screen holds its agent's history, so showing
    /// it again needs no reload (see `agent_views`)
    pub(crate) chat_loaded: bool,
    /// When the chat on screen started waiting for its history
    pub(crate) chat_loading_since: Option<Instant>,
    /// When the chat on screen got its history after waiting for it, to
    /// fade it in
    pub(crate) chat_revealed_at: Option<Instant>,
    /// The agent the UI switched to on screen and asked the session for,
    /// until the session confirms it; echoes of earlier switches are
    /// ignored meanwhile
    pub(crate) switch_sent: Option<String>,
    /// The agent the session last confirmed as active, to go back to when
    /// a switch fails
    pub(crate) confirmed_agent: String,
    /// What was typed for the agents not on screen, by agent ID; each
    /// agent gets its draft back when it comes on screen again
    pub(crate) drafts: HashMap<String, queue_panel::Draft>,
    /// Where the side panel's lists were when the agent just put on
    /// screen was last shown, put back once they are filtered for it
    pub(super) list_places_due: Option<side_scope::ListPlaces>,
    /// The agents most recently on screen, newest first, whose views are
    /// kept whole off screen (see `agent_views::shed`)
    pub(super) recent: VecDeque<String>,
    /// The agent on screen whose chat was shed and shows only the turn
    /// running, to ask for its history when the turn ends
    pub(super) reload_after_turn: Option<String>,
}

impl Default for AgentSwitch {
    /// No other agent's chat or draft kept, the chat on screen loaded.
    fn default() -> Self {
        Self {
            views: HashMap::new(),
            // The chat at startup gets its history without a switch; it
            // shows at once instead of fading in.
            chat_loaded: true,
            chat_loading_since: None,
            chat_revealed_at: None,
            switch_sent: None,
            confirmed_agent: String::new(),
            drafts: HashMap::new(),
            list_places_due: None,
            recent: VecDeque::new(),
            reload_after_turn: None,
        }
    }
}
