//! The chat panel's tabs: the conversation, or the terminal that runs the
//! session's shell commands.

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
}

impl ChatTab {
    pub const ALL: [Self; 2] = [Self::Chat, Self::Terminal];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Chat => "CHAT",
            Self::Terminal => "TERMINAL",
        }
    }

    /// Label for header bars too narrow for `label`.
    #[must_use]
    pub const fn short_label(self) -> &'static str {
        match self {
            Self::Chat => "CHT",
            Self::Terminal => "TRM",
        }
    }

    const fn other(self) -> Self {
        match self {
            Self::Chat => Self::Terminal,
            Self::Terminal => Self::Chat,
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
        if self.terminal_mode {
            ChatTab::Terminal
        } else {
            ChatTab::Chat
        }
    }

    /// Record where the chat panel's header bar was drawn this frame.
    pub fn set_chat_tabs_area(&mut self, area: Rect) {
        self.areas.chat_tabs = Some(area);
    }

    /// Show `tab`, opening it at its bottom with its own input history.
    pub(super) fn show_chat_tab(&mut self, tab: ChatTab) {
        self.terminal_mode = tab == ChatTab::Terminal;
        self.history().reset();
        self.scroll_chat_to_bottom();
    }

    pub(super) fn toggle_chat_tab(&mut self) {
        self.show_chat_tab(self.chat_tab().other());
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
