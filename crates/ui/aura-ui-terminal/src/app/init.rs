//! Building a new [`App`] with its initial state.

use super::*;

impl App {
    /// Create a new application instance.
    #[must_use]
    pub fn new() -> Self {
        Self {
            chat_state: AppState::Idle,
            modals: modal::ModalLayer::default(),
            messages: VecDeque::new(),
            input: InputState::default(),
            status: "Ready".to_string(),
            pending: Pending::default(),
            scroll_offset: 0,
            chat: ChatView::default(),
            link: KernelLink::default(),
            notifications: Notifications::default(),
            focus: PanelFocus::default(),
            panels: PanelLayout::default(),
            sandbox: SandboxTree::default(),
            ticks: Ticks::default(),
            records: RecordList::default(),
            members: members::Members::default(),
            inbox: inbox::Inbox::default(),
            detail: DetailState::default(),
            agents: Vec::new(),
            project_list: ProjectList::default(),
            active_agent_id: String::new(),
            switching: AgentSwitch::default(),
            account: Account::default(),
            areas: PanelAreas::default(),
            host: HostChrome::default(),
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
            hud: Hud::default(),
            queue_panel: queue_panel::QueuePanel::default(),
            bench_panel: bench_progress::BenchPanel::default(),
            todo_drawer: todo_drawer::TodoDrawer::default(),
            side: SidePanel::default(),
            graph: graph::GraphState::default(),
            trace_feed: trace_feed::TraceFeed::default(),
            task_list: tasks::TaskList::default(),
            repo: repo_tab::RepoTab::default(),
            flow: flow::FlowState::default(),
            bench: BenchState::default(),
            integrations: crate::app::apps::integrations::IntegrationsState::default(),
            log: run_log::LogState::default(),
        }
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}
