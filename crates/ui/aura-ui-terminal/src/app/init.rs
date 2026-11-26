//! Building a new [`App`] with its initial state.

use super::*;

impl App {
    /// Create a new application instance.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: AppState::Idle,
            messages: VecDeque::new(),
            input: InputState::default(),
            status: "Ready".to_string(),
            pending: Pending::default(),
            scroll_offset: 0,
            chat: ChatView::default(),
            event_tx: None,
            command_rx: None,
            notifications: Notifications::default(),
            focus: PanelFocus::default(),
            panels: PanelLayout::default(),
            sandbox: SandboxTree::default(),
            ticks: Ticks::default(),
            records: VecDeque::new(),
            selected_record: 0,
            records_scroll: 0,
            records_tab: RecordsTab::default(),
            members: members::Members::default(),
            inbox: inbox::Inbox::default(),
            policies_scroll: scroll::InfoScroll::default(),
            detail: None,
            detail_scroll: 0,
            detail_shown: 1,
            agents: Vec::new(),
            project_list: ProjectList::default(),
            active_agent_id: String::new(),
            switching: AgentSwitch::default(),
            account: Account::default(),
            sandbox_scroll: 0,
            areas: PanelAreas::default(),
            host_chrome: false,
            host_canvas: false,
            pane_chrome: Vec::new(),
            pane_splits: PaneSplits::default(),
            hover: None,
            shell_prompt: shell_prompt::detect(),
            terminal_mode: false,
            select: SelectionState::default(),
            agents_tab: AgentsTab::default(),
            apps: apps::AppMemory::default(),
            channels: apps::ChannelState::default(),
            definitions: ListState::default(),
            archived: ListState::default(),
            channel_form: None,
            context_menu: None,
            workspaces: context_menu::Workspaces::default(),
            hud: Hud::default(),
            cloud: host_tab::CloudUi::default(),
            queue_panel: queue_panel::QueuePanel::default(),
            bench_panel: bench_progress::BenchPanel::default(),
            todos_open: false,
            todos_expanded: false,
            side_tab: SidePanelTab::default(),
            side_scope: SideScope::default(),
            all_processes: Vec::new(),
            process_filter: None,
            processes: ListState::default(),
            graph: graph::GraphState::default(),
            trace_feed: trace_feed::TraceFeed::default(),
            trace_scroll: 0,
            task_list: tasks::TaskList::default(),
            tasks_scroll: 0,
            flow: flow::FlowState::default(),
            bench: BenchState::default(),
            log: run_log::LogState::default(),
        }
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}
