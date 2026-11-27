//! What sits under the pointer: hover targets, chat hits and project rows.

use super::*;

/// Clickable item under the pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HoverTarget {
    /// A label in the agents tab bar.
    AgentTab(AgentsTab),
    /// A label in the chat panel's tab bar.
    ChatTab(ChatTab),
    /// The close button of the view over the chat panel.
    CloseDetail,
    /// The button that expands the chat panel or restores it.
    ExpandChat,
    /// The stop button in the input box, shown while a turn runs.
    StopTurn,
    /// The placement switch at the right end of the footer.
    PlacementSwitch,
    /// A row in the projects list, including "+ New project".
    ProjectRow(usize),
    /// The "+" at the right end of a project's row in the projects list.
    AddToProject(usize),
    /// The archive button left of the time on an agent's row in the
    /// projects list, shown while the row is hovered.
    ArchiveAgent(usize),
    /// A row in the roster, including "+ New agent".
    DefinitionRow(usize),
    /// A selectable row of the CHANNELS tab: a channel or "+ New channel".
    ChannelRow(usize),
    /// A row of BENCH's list, by its index in `App::bench_rows`: a
    /// benchmark, one of its agents, or "+ New bench".
    BenchRow(usize),
    /// A member row of an open channel's MEMBERS side tab.
    ChannelMemberRow(usize),
    /// A row in the Archive.
    ArchivedRow(usize),
    /// A row in the sandbox tree.
    SandboxRow(usize),
    /// A row in the Record panel.
    RecordRow(usize),
    /// A label in the Record panel's tab bar.
    RecordTab(RecordsTab),
    /// A row in the Record panel's MEMBERS tab.
    MemberRow(usize),
    /// A row in the Record panel's INBOX tab.
    InboxRow(usize),
    /// A label or icon in the side panel's tab bar.
    SideTab(SidePanelTab),
    /// The `*` button at the right end of the side panel's tab bar.
    SideScope,
    /// The button right of it that maximizes the side panel or restores
    /// it.
    ExpandSide,
    /// The hamburger button at the right end of the header row, which
    /// hides the right column or shows it again.
    SideToggle,
    /// A row in the Processes tab.
    ProcessRow(usize),
    /// A row in the Trace tab.
    TraceRow(usize),
    /// A row in the Tasks tab.
    TaskRow(usize),
    /// A filter at the top of the REPO tab.
    RepoFilter(repo_tab::RepoFilter),
    /// A row in the REPO tab.
    RepoRow(usize),
    /// A chat row that expands or collapses agent activity.
    Activity(ToggleTarget),
    /// An image thumbnail in the chat: message id, which of its images.
    ChatImage(u64, usize),
    /// A thumbnail above the input, by its `[image N]` label.
    PendingImage(usize),
    /// The copy button of a code block in the chat, by the chat line of
    /// the block's top border.
    CopyCode(usize),
    /// A link in the chat, by its number (see [`LinkHit::link`]), so
    /// every row of a link that wraps lights up with the one pointed at.
    Link(usize),
}

/// What clicking an activity row in the chat expands or collapses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ToggleTarget {
    /// A group of steps, by the id of its first message
    Group(u64),
    /// One step, by message id
    Step(u64),
    /// The kernel records of a turn, by when its prompt was sent (Unix ms)
    TurnRecords(u64),
    /// The `… more lines` row of a part of a message cut short, by
    /// message id
    More(u64, MoreSection),
}

/// A thumbnail in the chat that opens its image when clicked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ImageHit {
    /// Chat line of the thumbnail's first row
    pub line: usize,
    /// Rows the thumbnail takes
    pub rows: usize,
    /// First column, counted from the chat text's left edge
    pub x: u16,
    /// Columns the thumbnail takes
    pub width: u16,
    /// Id of the message holding the image
    pub message: u64,
    /// Which of the message's images
    pub index: usize,
}

/// A copy button in the chat: in the top border of a code block, or on a
/// boot console's header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CodeCopyHit {
    /// Chat line of the button
    pub line: usize,
    /// First column of the button, counted from the chat text's left edge
    pub x: u16,
    /// Columns the button takes
    pub width: u16,
    /// What a click copies
    pub text: CopyText,
}

/// What a chat copy button copies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CopyText {
    /// The code as written in the message, shared with the line cache so
    /// handing hits out each frame does not copy it
    Code(std::sync::Arc<str>),
    /// The boot console with this id, its whole log read when clicked, as
    /// a live one is drawn every frame
    BootLog(String),
}

/// One row of a link in the chat's text. A link that wraps is a piece
/// per row, all with the same `link` and `url`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LinkHit {
    /// Chat line of the piece
    pub line: usize,
    /// First column of the piece, counted from the chat text's left edge
    pub x: u16,
    /// Columns the piece takes
    pub width: u16,
    /// Which link the piece is part of, numbered from 0 in the order the
    /// links are drawn
    pub link: usize,
    /// Where the link goes, shared by its pieces and the line cache
    pub url: std::sync::Arc<str>,
}

impl LinkHit {
    /// The piece `rows` further down and `columns` further right, its
    /// link numbered after the first `links`: where it falls once the
    /// lines it is on are put after others or behind a prompt.
    pub(crate) fn moved(self, (rows, columns, links): (usize, u16, usize)) -> Self {
        Self {
            line: self.line + rows,
            x: self.x.saturating_add(columns),
            link: self.link + links,
            ..self
        }
    }

    /// How many numbers the links of `hits` take, so links put after
    /// them are numbered on from there.
    pub(crate) fn numbers(hits: &[Self]) -> usize {
        hits.iter().map(|hit| hit.link + 1).max().unwrap_or(0)
    }
}

/// A chat row that expands or collapses `target` when clicked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ToggleRow {
    /// Chat line of the row
    pub line: usize,
    /// What the row toggles
    pub target: ToggleTarget,
    /// Whether `target` is shown open
    pub open: bool,
}

impl HoverTarget {
    /// The hover target of `button` on projects-list row `index`.
    pub(crate) const fn agent_button(button: AgentButton, index: usize) -> Self {
        match button {
            AgentButton::Archive => Self::ArchiveAgent(index),
        }
    }
}

/// One row of the projects list. A project is its own row; its first
/// agent, and any agents that agent spawned, hang underneath while the
/// project is expanded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProjectListRow {
    /// Foldable header. `root` is the index of the project's first agent.
    Project { root: usize, expanded: bool },
    /// Header of a project with every agent archived, which stays listed
    /// until it is removed. `project` is its index in
    /// [`App::projects`].
    EmptyProject { project: usize },
    /// An agent under a project.
    Agent { index: usize },
    /// The "+ New project" row.
    NewProject,
    /// Blank row between two projects. Never selected.
    Gap,
}
