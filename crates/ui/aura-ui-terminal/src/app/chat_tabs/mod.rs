//! The chat panel's tabs: the conversation, the terminal that runs the
//! session's shell commands, the graph of what the agent and the agents
//! it spawned did, the same work over time, or the run log of the agent
//! and the agents below it. Each agent keeps the tab it was left on (see
//! `agent_views`).

use super::flow::FlowMode;
use super::{tab_at, tab_bar::TabLayout, App, HoverTarget};
use ratatui::layout::Rect;

/// Views of the chat panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChatTab {
    /// The conversation with the active agent.
    #[default]
    Chat,
    /// Shell commands run this session; the input line runs commands.
    Terminal,
    /// The flow between the active agent and the agents it spawned, from
    /// the kernel record.
    Graph,
    /// The same flow as a timeline: an agent a row, its turns as bars.
    Time,
    /// The run log of the agent on screen and of every agent below it,
    /// and where it is stored.
    Log,
}

impl ChatTab {
    pub const ALL: [Self; 5] = [
        Self::Chat,
        Self::Terminal,
        Self::Graph,
        Self::Time,
        Self::Log,
    ];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Chat => "CHAT",
            Self::Terminal => "TERMINAL",
            Self::Graph => "GRAPH",
            Self::Time => "TIME",
            Self::Log => "LOG",
        }
    }

    /// Label for header bars too narrow for `label`.
    #[must_use]
    pub const fn short_label(self) -> &'static str {
        match self {
            Self::Chat => "CHT",
            Self::Terminal => "TRM",
            Self::Graph => "GRP",
            Self::Time => "TIM",
            Self::Log => "LOG",
        }
    }

    const fn next(self) -> Self {
        match self {
            Self::Chat => Self::Terminal,
            Self::Terminal => Self::Graph,
            Self::Graph => Self::Time,
            Self::Time => Self::Log,
            Self::Log => Self::Chat,
        }
    }

    const fn prev(self) -> Self {
        match self {
            Self::Chat => Self::Log,
            Self::Terminal => Self::Chat,
            Self::Graph => Self::Terminal,
            Self::Time => Self::Graph,
            Self::Log => Self::Time,
        }
    }

    /// The tabs' cells in a header bar `width` columns wide, as drawn and
    /// as clicks are mapped back.
    #[must_use]
    pub fn layout(width: u16) -> TabLayout<Self> {
        TabLayout::new(Self::ALL, Self::label, Self::short_label, width)
    }
}

impl App {
    /// Which view the chat panel shows.
    #[must_use]
    pub const fn chat_tab(&self) -> ChatTab {
        if self.flow.is_open() {
            match self.flow.mode() {
                FlowMode::Canvas => ChatTab::Graph,
                FlowMode::Timeline => ChatTab::Time,
            }
        } else if self.log.is_open() {
            ChatTab::Log
        } else if self.terminal_mode {
            ChatTab::Terminal
        } else {
            ChatTab::Chat
        }
    }

    /// Record where the chat panel's header bar was drawn this frame.
    pub fn set_chat_tabs_area(&mut self, area: Rect) {
        self.areas.chat_tabs = Some(area);
    }

    /// Make `tab` the chat panel's view, with its own input history. The
    /// flow's mode, GRAPH's or TIME's, is the caller's to set: showing
    /// TIME moves its timeline to the selected card, coming back on it
    /// does not.
    fn set_chat_tab(&mut self, tab: ChatTab) {
        self.terminal_mode = tab == ChatTab::Terminal;
        self.flow
            .set_open(matches!(tab, ChatTab::Graph | ChatTab::Time));
        self.log.set_open(tab == ChatTab::Log);
        self.history().reset();
    }

    /// Show `tab`. The conversation and the terminal open at their bottom
    /// with their own input history; the graph and the timeline ask for
    /// the flow of the agent on screen, unless the other has it, and the
    /// log for the agent's. CHANNELS has only CHAT, as does BENCH but on
    /// an agent of a benchmark ([`Self::chat_only`]): any other tab asked
    /// for there, by key, click or command, is not shown.
    pub(super) fn show_chat_tab(&mut self, tab: ChatTab) {
        if self.chat_only() && tab != ChatTab::Chat {
            return;
        }
        let flow_was_open = self.flow.is_open();
        self.set_chat_tab(tab);
        self.clear_selection();
        self.scroll_chat_to_bottom();
        match tab {
            ChatTab::Graph | ChatTab::Time => {
                let mode = if tab == ChatTab::Time {
                    FlowMode::Timeline
                } else {
                    FlowMode::Canvas
                };
                if self.flow.agent_id() == self.active_agent_id {
                    self.flow.set_mode(mode);
                } else {
                    // The flow on hand is another agent's, put away just below: its
                    // timeline keeps its own selection for when that agent comes back.
                    self.flow.restore_mode(mode);
                }
                if !flow_was_open {
                    self.request_flow();
                }
            }
            ChatTab::Log => self.request_log(),
            ChatTab::Chat | ChatTab::Terminal => {}
        }
    }

    /// Put the chat panel back on `tab`, the one the agent coming on
    /// screen was left on. Unlike [`Self::show_chat_tab`], it neither
    /// scrolls the chat to its bottom nor moves the timeline's selection,
    /// and it asks for nothing: the switch asks for what the tab shows of
    /// the new agent. The input holds the draft the agent was left with,
    /// typed under this tab, so Enter does with it what it did then. Where
    /// there is only CHAT ([`Self::chat_only`]), any other tab stays put
    /// away.
    pub(super) fn restore_chat_tab(&mut self, tab: ChatTab) {
        if tab == self.chat_tab() || (self.chat_only() && tab != ChatTab::Chat) {
            return;
        }
        self.set_chat_tab(tab);
        match tab {
            ChatTab::Graph => self.flow.restore_mode(FlowMode::Canvas),
            ChatTab::Time => self.flow.restore_mode(FlowMode::Timeline),
            ChatTab::Chat | ChatTab::Terminal | ChatTab::Log => {}
        }
    }

    /// Show the tab after the open one, or the one before it, wrapping
    /// around.
    pub(super) fn shift_chat_tab(&mut self, forward: bool) {
        let tab = self.chat_tab();
        self.show_chat_tab(if forward { tab.next() } else { tab.prev() });
    }

    /// Clicking a tab of the chat panel shows it. Returns whether the click
    /// was on the tab bar.
    pub(super) fn click_chat_tabs(&mut self, column: u16, row: u16) -> bool {
        let Some(tab) = self.chat_tab_at(column, row) else {
            return false;
        };
        if tab != self.chat_tab() {
            self.show_chat_tab(tab);
        }
        true
    }

    fn chat_tab_at(&self, column: u16, row: u16) -> Option<ChatTab> {
        tab_at(self.areas.chat_tabs, column, row, |width| {
            self.chat_tab_layout(width)
        })
    }

    /// The chat panel's tabs in a header bar `width` columns wide.
    /// Where there is only CHAT ([`Self::chat_only`]), the bar has no tabs
    /// and shows a title instead ([`Self::app_chat_title`]). An app
    /// showing nothing ([`Self::app_empty`]) has no tabs either.
    #[must_use]
    pub(crate) fn chat_tab_layout(&self, width: u16) -> TabLayout<ChatTab> {
        if self.chat_only() || self.app_empty() {
            TabLayout::new([], ChatTab::label, ChatTab::short_label, width)
        } else {
            ChatTab::layout(width)
        }
    }

    /// The chat panel's tab under the pointer.
    pub(super) fn hovered_chat_tab(&self, column: u16, row: u16) -> Option<HoverTarget> {
        self.chat_tab_at(column, row).map(HoverTarget::ChatTab)
    }
}

#[cfg(test)]
mod tests;
