//! The chat panel's tabs: the conversation, the terminal that runs the
//! session's shell commands, the graph of what the agent and the agents
//! it spawned did, the same work over time, the benchmarks that compare
//! two arms on the same work, or the run log of the agent and the agents
//! below it. Each agent keeps the tab it was left on (see `agent_views`).

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

    /// Make `tab` the chat panel's view, with its own input history. The
    /// flow's mode, GRAPH's or TIME's, is the caller's to set: showing
    /// TIME moves its timeline to the selected card, coming back on it
    /// does not.
    fn set_chat_tab(&mut self, tab: ChatTab) {
        self.terminal_mode = tab == ChatTab::Terminal;
        self.flow
            .set_open(matches!(tab, ChatTab::Graph | ChatTab::Time));
        self.bench.set_open(tab == ChatTab::Bench);
        self.log.set_open(tab == ChatTab::Log);
        self.history().reset();
    }

    /// Show `tab`. The conversation and the terminal open at their bottom
    /// with their own input history; the graph and the timeline ask for
    /// the flow of the agent on screen, unless the other has it, the
    /// benchmarks for the latest of them, and the log for the agent's.
    pub(super) fn show_chat_tab(&mut self, tab: ChatTab) {
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
            ChatTab::Bench => self.request_benchmarks(),
            ChatTab::Log => self.request_log(),
            ChatTab::Chat | ChatTab::Terminal => {}
        }
    }

    /// Put the chat panel back on `tab`, the one the agent coming on
    /// screen was left on. Unlike [`Self::show_chat_tab`], it neither
    /// scrolls the chat to its bottom nor moves the timeline's selection,
    /// and it asks for nothing: the switch asks for what the tab shows of
    /// the new agent. Text typed on the input line, which every agent
    /// shares, keeps its meaning across the switch: a message is not run
    /// as a shell command, nor a command sent to the model.
    pub(super) fn restore_chat_tab(&mut self, tab: ChatTab) {
        // Text typed keeps what Enter does with it: under a typed message an
        // agent left on TERMINAL comes back on CHAT, and under a typed command
        // the terminal stays.
        let tab = if self.input.is_empty() || (tab == ChatTab::Terminal) == self.terminal_mode {
            tab
        } else if self.terminal_mode {
            ChatTab::Terminal
        } else {
            ChatTab::Chat
        };
        if tab == self.chat_tab() {
            return;
        }
        self.set_chat_tab(tab);
        match tab {
            ChatTab::Graph => self.flow.restore_mode(FlowMode::Canvas),
            ChatTab::Time => self.flow.restore_mode(FlowMode::Timeline),
            ChatTab::Chat | ChatTab::Terminal | ChatTab::Bench | ChatTab::Log => {}
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{AgentSummary, UiCommand, UiEvent};
    use crossterm::event::{KeyCode, KeyEvent};
    use tokio::sync::mpsc;

    /// Agents `aa` and `bb` with `aa` on screen, and what the app sends.
    fn two_agents() -> (App, mpsc::Receiver<UiEvent>) {
        // Room for every request a test makes: a full channel drops them.
        let (tx, rx) = mpsc::channel(64);
        let mut app = App::new().with_event_sender(tx);
        let agent = |id: &str| AgentSummary {
            id: id.to_string(),
            name: id.to_uppercase(),
            ..AgentSummary::default()
        };
        app.process_command(UiCommand::SetAgents(vec![agent("aa"), agent("bb")]));
        app.process_command(UiCommand::SetActiveAgent("aa".to_string()));
        (app, rx)
    }

    /// Put `agent_id` on screen, as the session does once a switch to it
    /// is asked for.
    fn switch_to(app: &mut App, agent_id: &str) {
        app.process_command(UiCommand::SetActiveAgent(agent_id.to_string()));
    }

    /// What the tabs asked the session for since the last call: a flow,
    /// the benchmarks or a log, and of which agent.
    fn requests(rx: &mut mpsc::Receiver<UiEvent>) -> Vec<String> {
        std::iter::from_fn(|| rx.try_recv().ok())
            .filter_map(|event| match event {
                UiEvent::OpenFlow(agent_id) => Some(format!("flow {agent_id}")),
                UiEvent::Bench { agent_id, request } => {
                    Some(format!("bench {agent_id} {request:?}"))
                }
                UiEvent::Log { agent_id, .. } => Some(format!("log {agent_id}")),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn each_agent_comes_back_on_the_tab_it_was_left_on() {
        let (mut app, _rx) = two_agents();
        for tab in ChatTab::ALL {
            app.show_chat_tab(tab);
            switch_to(&mut app, "bb");
            assert_eq!(
                app.chat_tab(),
                ChatTab::Chat,
                "aa's {tab:?} does not carry over to bb"
            );
            switch_to(&mut app, "aa");
            assert_eq!(app.chat_tab(), tab, "aa was left on {tab:?}");
        }

        app.show_chat_tab(ChatTab::Terminal);
        switch_to(&mut app, "bb");
        app.show_chat_tab(ChatTab::Log);
        switch_to(&mut app, "aa");
        assert_eq!(app.chat_tab(), ChatTab::Terminal);
        assert!(app.terminal_pane());

        switch_to(&mut app, "aa");
        assert_eq!(
            app.chat_tab(),
            ChatTab::Terminal,
            "the agent on screen sent again stays on its tab"
        );

        switch_to(&mut app, "bb");
        assert_eq!(app.chat_tab(), ChatTab::Log, "bb was left on LOG");
    }

    #[test]
    fn an_agent_coming_back_asks_once_for_what_its_tab_shows() {
        let cases: [(ChatTab, &[&str]); 6] = [
            (ChatTab::Graph, &["flow aa"]),
            (ChatTab::Time, &["flow aa"]),
            (ChatTab::Bench, &["bench aa List", "bench aa Sessions"]),
            (ChatTab::Log, &["log aa"]),
            (ChatTab::Terminal, &[]),
            (ChatTab::Chat, &[]),
        ];
        for (tab, expected) in cases {
            let (mut app, mut rx) = two_agents();
            app.show_chat_tab(tab);
            // Drop what showing the tab asked for.
            requests(&mut rx);

            switch_to(&mut app, "bb");
            assert!(
                requests(&mut rx).is_empty(),
                "bb on CHAT asks for nothing, aa left on {tab:?}"
            );

            switch_to(&mut app, "aa");
            assert_eq!(app.chat_tab(), tab);
            assert_eq!(requests(&mut rx), expected, "aa back on {tab:?}");
            if tab == ChatTab::Time {
                assert_eq!(app.flow().mode(), FlowMode::Timeline);
            }
        }

        // Both on GRAPH: the switch asks for the flow of the agent coming
        // on screen.
        let (mut app, mut rx) = two_agents();
        app.show_chat_tab(ChatTab::Graph);
        switch_to(&mut app, "bb");
        app.show_chat_tab(ChatTab::Graph);
        switch_to(&mut app, "aa");
        requests(&mut rx);
        switch_to(&mut app, "bb");
        assert_eq!(app.chat_tab(), ChatTab::Graph);
        assert_eq!(requests(&mut rx), ["flow bb"]);
    }

    /// Coming back on a tab leaves the chat's scroll alone, where showing
    /// a tab scrolls it to its bottom. (The session reloads an idle
    /// agent's conversation after a switch, and that starts at its bottom.)
    #[test]
    fn coming_back_on_a_tab_does_not_scroll_the_chat() {
        let (mut app, _rx) = two_agents();
        app.show_chat_tab(ChatTab::Terminal);
        app.scroll_up(7);
        assert_eq!(app.scroll_offset(), 7);

        switch_to(&mut app, "bb");
        assert_eq!(app.scroll_offset(), 0, "bb's chat is at its bottom");

        switch_to(&mut app, "aa");
        assert!(app.terminal_pane());
        assert_eq!(app.scroll_offset(), 7, "where aa was left");

        // Showing a tab, unlike coming back on it, opens it at its bottom.
        app.show_chat_tab(ChatTab::Chat);
        assert_eq!(app.scroll_offset(), 0);
    }

    #[test]
    fn background_output_leaves_the_tab_on_screen_alone() {
        let (mut app, mut rx) = two_agents();
        app.show_chat_tab(ChatTab::Graph);
        requests(&mut rx);

        for command in [
            UiCommand::TurnStarted {
                prompt: Some("ask bb".to_string()),
            },
            UiCommand::StartStreaming,
            UiCommand::AppendText("x".into()),
            UiCommand::FinishStreaming,
            UiCommand::Complete,
        ] {
            app.process_command(UiCommand::ForAgent {
                agent_id: "bb".into(),
                command: Box::new(command),
            });
        }
        assert!(app.messages().is_empty(), "the output went to bb's chat");
        assert_eq!(app.chat_tab(), ChatTab::Graph);
        assert!(requests(&mut rx).is_empty(), "bb's output asks for nothing");

        switch_to(&mut app, "bb");
        assert_eq!(app.chat_tab(), ChatTab::Chat, "bb was never shown");
        switch_to(&mut app, "aa");
        assert_eq!(app.chat_tab(), ChatTab::Graph);
    }

    #[test]
    fn asking_for_an_agents_log_wins_over_the_tab_it_was_left_on() {
        let (mut app, mut rx) = two_agents();
        switch_to(&mut app, "bb");
        app.show_chat_tab(ChatTab::Terminal);
        switch_to(&mut app, "aa");
        assert_eq!(app.chat_tab(), ChatTab::Chat);
        requests(&mut rx);

        app.open_log_of("bb".to_string());
        let sent: Vec<UiEvent> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        assert!(
            matches!(sent.as_slice(), [UiEvent::SelectAgent(id)] if id == "bb"),
            "asks for the switch to bb, sent {sent:?}"
        );
        assert_eq!(app.chat_tab(), ChatTab::Chat, "until the switch lands");

        switch_to(&mut app, "bb");
        assert_eq!(
            app.chat_tab(),
            ChatTab::Log,
            "not the TERMINAL bb was left on"
        );
        assert_eq!(requests(&mut rx), ["log bb"]);

        switch_to(&mut app, "aa");
        assert_eq!(app.chat_tab(), ChatTab::Chat);
        switch_to(&mut app, "bb");
        assert_eq!(app.chat_tab(), ChatTab::Log, "bb was left on LOG this time");
    }

    /// Type `text` on the input line, a key at a time.
    fn type_text(app: &mut App, text: &str) {
        for c in text.chars() {
            app.handle_key(KeyEvent::from(KeyCode::Char(c)));
        }
    }

    /// Leave `text` on the input line, as typing it does, on tabs whose
    /// keys would take the first letters typed.
    fn draft(app: &mut App, text: &str) {
        app.input = text.to_string();
        app.cursor_pos = text.len();
    }

    #[test]
    fn typed_text_keeps_what_enter_does_across_a_switch() {
        // A message typed is not run as a shell command: bb, left on
        // TERMINAL, comes back on CHAT.
        let (mut app, _rx) = two_agents();
        switch_to(&mut app, "bb");
        app.show_chat_tab(ChatTab::Terminal);
        switch_to(&mut app, "aa");
        assert_eq!(app.chat_tab(), ChatTab::Chat);
        type_text(&mut app, "hi");
        switch_to(&mut app, "bb");
        assert_eq!(
            app.chat_tab(),
            ChatTab::Chat,
            "not the TERMINAL bb was left on"
        );
        assert!(!app.input_is_command(), "Enter sends the line as a message");
        assert_eq!(app.input(), "hi");

        // A command typed is not sent to the model: the terminal stays.
        let (mut app, _rx) = two_agents();
        switch_to(&mut app, "bb");
        app.show_chat_tab(ChatTab::Graph);
        switch_to(&mut app, "aa");
        app.show_chat_tab(ChatTab::Terminal);
        type_text(&mut app, "ls");
        switch_to(&mut app, "bb");
        assert_eq!(
            app.chat_tab(),
            ChatTab::Terminal,
            "not the GRAPH bb was left on"
        );
        assert!(app.input_is_command(), "Enter runs the line as a command");
        assert_eq!(app.input(), "ls");

        // With nothing typed, the agent comes back on the tab it was left on.
        let (mut app, _rx) = two_agents();
        switch_to(&mut app, "bb");
        app.show_chat_tab(ChatTab::Terminal);
        switch_to(&mut app, "aa");
        assert_eq!(app.chat_tab(), ChatTab::Chat);
        assert!(app.input().is_empty());
        switch_to(&mut app, "bb");
        assert_eq!(app.chat_tab(), ChatTab::Terminal);

        // A typed message keeps any tab where Enter still sends it.
        for saved in [ChatTab::Graph, ChatTab::Bench, ChatTab::Log] {
            let (mut app, _rx) = two_agents();
            switch_to(&mut app, "bb");
            app.show_chat_tab(saved);
            switch_to(&mut app, "aa");
            draft(&mut app, "hi");
            switch_to(&mut app, "bb");
            assert_eq!(app.chat_tab(), saved, "a message, bb left on {saved:?}");
            assert!(!app.input_is_command());
        }

        // A message typed on GRAPH or LOG is not run as a command either.
        for typed_on in [ChatTab::Graph, ChatTab::Log] {
            let (mut app, _rx) = two_agents();
            switch_to(&mut app, "bb");
            app.show_chat_tab(ChatTab::Terminal);
            switch_to(&mut app, "aa");
            app.show_chat_tab(typed_on);
            draft(&mut app, "hi");
            switch_to(&mut app, "bb");
            assert_eq!(app.chat_tab(), ChatTab::Chat, "typed on {typed_on:?}");
            assert!(!app.input_is_command());
        }
    }
}
