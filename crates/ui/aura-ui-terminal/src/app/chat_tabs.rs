//! The chat panel's tabs: the conversation, the terminal that runs the
//! session's shell commands, the graph of what the agent and the agents
//! it spawned did, the same work over time, the benchmarks that compare
//! two arms on the same work, or the run log of the agent and the agents
//! below it.

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
    /// Benchmarks: the same work on two arms (Gate or a model), their
    /// cost and how well they did.
    Bench,
    /// The run log of the agent on screen and of every agent below it,
    /// and where it is stored.
    Log,
}

impl ChatTab {
    pub const ALL: [Self; 6] = [
        Self::Chat,
        Self::Terminal,
        Self::Graph,
        Self::Time,
        Self::Bench,
        Self::Log,
    ];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Chat => "CHAT",
            Self::Terminal => "TERMINAL",
            Self::Graph => "GRAPH",
            Self::Time => "TIME",
            Self::Bench => "BENCH",
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
            Self::Bench => "BEN",
            Self::Log => "LOG",
        }
    }

    const fn next(self) -> Self {
        match self {
            Self::Chat => Self::Terminal,
            Self::Terminal => Self::Graph,
            Self::Graph => Self::Time,
            Self::Time => Self::Bench,
            Self::Bench => Self::Log,
            Self::Log => Self::Chat,
        }
    }

    const fn prev(self) -> Self {
        match self {
            Self::Chat => Self::Log,
            Self::Terminal => Self::Chat,
            Self::Graph => Self::Terminal,
            Self::Time => Self::Graph,
            Self::Bench => Self::Time,
            Self::Log => Self::Bench,
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
        } else if self.bench.is_open() {
            ChatTab::Bench
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

    /// Show `tab`. The conversation and the terminal open at their bottom
    /// with their own input history; the graph and the timeline ask for
    /// the flow of the agent on screen, unless the other has it, the
    /// benchmarks for the latest of them, and the log for the agent's.
    pub(super) fn show_chat_tab(&mut self, tab: ChatTab) {
        let flow_was_open = self.flow.is_open();
        self.terminal_mode = tab == ChatTab::Terminal;
        self.flow
            .set_open(matches!(tab, ChatTab::Graph | ChatTab::Time));
        self.bench.set_open(tab == ChatTab::Bench);
        self.log.set_open(tab == ChatTab::Log);
        self.history().reset();
        self.clear_selection();
        self.scroll_chat_to_bottom();
        match tab {
            ChatTab::Graph | ChatTab::Time => {
                self.flow.set_mode(if tab == ChatTab::Time {
                    FlowMode::Timeline
                } else {
                    FlowMode::Canvas
                });
                if !flow_was_open {
                    self.request_flow();
                }
            }
            ChatTab::Bench => self.request_benchmarks(),
            ChatTab::Log => self.request_log(),
            ChatTab::Chat | ChatTab::Terminal => {}
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
        tab_at(self.areas.chat_tabs, column, row, ChatTab::layout)
    }

    /// The chat panel's tab under the pointer.
    pub(super) fn hovered_chat_tab(&self, column: u16, row: u16) -> Option<HoverTarget> {
        self.chat_tab_at(column, row).map(HoverTarget::ChatTab)
    }
}
