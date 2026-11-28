//! Screen areas from the last frame and helpers to hit-test them.

use super::*;

/// Rows of the input box's wrapped text in the last frame.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct InputRows {
    /// Text row drawn on the box's first row
    pub(crate) top: usize,
    /// Rows the text wraps to
    pub(crate) total: usize,
    /// Rows the box had room to show
    pub(crate) visible: usize,
}

impl InputRows {
    /// Highest `top` that still fills the box.
    pub(super) const fn max_top(self) -> usize {
        self.total.saturating_sub(self.visible)
    }
}

/// Where the wheel scrolled the input box's text, and the draft and
/// cursor it was scrolled on. Any edit or cursor move changes one of
/// those, so comparing them tells when the view should follow the cursor
/// again without every key path having to reset it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InputView {
    /// Text row to show on the box's first row
    pub(super) top: usize,
    /// The input when it was scrolled
    pub(super) input: String,
    /// The cursor when it was scrolled
    pub(super) cursor: usize,
}

/// Screen areas of the content panels from the last rendered frame.
#[derive(Debug, Clone, Copy, Default)]
pub struct PanelAreas {
    /// Agents panel (including border)
    pub swarm: Option<Rect>,
    /// Rows of the agents list inside the panel
    pub agent_rows: Option<Rect>,
    /// "+ New project", pinned to the bottom of the agents panel
    pub new_project_row: Option<Rect>,
    /// Header bar of the agents panel, up to its BENCH icon
    pub agent_tabs: Option<Rect>,
    /// BENCH's icon, at the right end of the agents panel's header bar
    pub agent_icon_tab: Option<Rect>,
    /// INTEGRATIONS' icon, left of BENCH's
    pub agent_lead_icon: Option<Rect>,
    /// Rows of the roster inside the agents panel
    pub definition_rows: Option<Rect>,
    /// "+ New agent", pinned to the bottom of the agents panel
    pub new_agent_row: Option<Rect>,
    /// Rows of the Archive inside the agents panel
    pub archived_rows: Option<Rect>,
    /// Chat panel (including border)
    pub chat: Option<Rect>,
    /// Header bar of the chat panel
    pub chat_tabs: Option<Rect>,
    /// Close button in the chat panel's header bar while a view covers it
    pub detail_close: Option<Rect>,
    /// Button in the chat panel's header bar that expands or restores it
    pub chat_expand: Option<Rect>,
    /// Message text area inside the chat panel
    pub chat_text: Option<Rect>,
    /// Rows of the chat panel's body that scroll by rows: the
    /// conversation, the terminal, the benchmarks, or a record or file
    /// shown over them. `None` under the graph or an image.
    pub chat_rows: Option<Rect>,
    /// Content row shown at the top of `chat_rows`
    pub chat_rows_top: usize,
    /// Input box under the chat panel (including border)
    pub input: Option<Rect>,
    /// Stop button at the right end of the input box's first row, drawn
    /// while the agent on screen is busy
    pub input_stop: Option<Rect>,
    /// Columns the input box wrapped its text at, which Up and Down
    /// follow to move between the rows drawn
    pub input_wrap: Option<usize>,
    /// Rows of the input box's text and which of them it showed, which
    /// the wheel scrolls from
    pub(crate) input_rows: Option<InputRows>,
    /// Record panel (including border)
    pub records: Option<Rect>,
    /// Header bar of the record panel
    pub record_tabs: Option<Rect>,
    /// Rows of the record list inside the panel
    pub record_rows: Option<Rect>,
    /// Rows of the MEMBERS tab inside the record panel
    pub member_rows: Option<Rect>,
    /// Member row shown at the top of `member_rows`
    pub members_scroll: usize,
    /// Rows of the INBOX tab inside the record panel
    pub inbox_rows: Option<Rect>,
    /// Inbox row shown at the top of `inbox_rows`
    pub inbox_scroll: usize,
    /// Sandbox panel (including border)
    pub sandbox: Option<Rect>,
    /// Rows of the sandbox tree inside the panel
    pub sandbox_rows: Option<Rect>,
    /// Tree row shown at the top of `sandbox_rows`
    pub sandbox_scroll: usize,
    /// Header bar of the panel below the Record panel, up to its HOST
    /// icon
    pub side_tabs: Option<Rect>,
    /// HOST's icon, left of the bar's `*` button
    pub side_icon_tab: Option<Rect>,
    /// Button at the right end of that bar that shows the whole
    /// project's items in its lists, or the active agent's
    pub side_scope: Option<Rect>,
    /// Button right of that one that maximizes the panel over the Record
    /// panel, or restores it
    pub side_expand: Option<Rect>,
    /// Hamburger button at the right end of the header row that hides
    /// the right column or shows it again
    pub side_toggle: Option<Rect>,
    /// Rows of the Processes tab inside the panel
    pub process_rows: Option<Rect>,
    /// Process row shown at the top of `process_rows`
    pub processes_scroll: usize,
    /// Rows of the Trace tab inside the panel
    pub trace_rows: Option<Rect>,
    /// Feed row shown at the top of `trace_rows`
    pub trace_scroll: usize,
    /// Rows of the Tasks tab inside the panel
    pub task_rows: Option<Rect>,
    /// Tasks row shown at the top of `task_rows`
    pub tasks_scroll: usize,
    /// Rows of the REPO tab inside the panel, below its filters
    pub repo_rows: Option<Rect>,
    /// REPO row shown at the top of `repo_rows`
    pub repo_scroll: usize,
    /// Scrollbar on the chat panel's right border, when its list
    /// overflows
    pub(crate) scrollbar: Option<scrollbar::ScrollbarArea>,
}

/// The tab of the header bar drawn at `bar` under a screen cell, laid out
/// by `layout` for the bar's width.
pub(super) fn tab_at<T: Copy + PartialEq>(
    bar: Option<Rect>,
    column: u16,
    row: u16,
    layout: impl Fn(u16) -> tab_bar::TabLayout<T>,
) -> Option<T> {
    let bar = bar.filter(|&bar| contains(Some(bar), column, row))?;
    layout(bar.width).tab_at(column - bar.x)
}

pub(super) fn contains(area: Option<Rect>, column: u16, row: u16) -> bool {
    area.is_some_and(|a| {
        column >= a.x && column < a.x + a.width && row >= a.y && row < a.y + a.height
    })
}

/// First row to show of a `total`-row list in a `visible`-row view so that
/// row `selected` stays in view, moving as little as possible from `scroll`.
pub(crate) fn follow_selection(
    scroll: usize,
    selected: usize,
    visible: usize,
    total: usize,
) -> usize {
    let visible = visible.max(1);
    let scroll = if selected < scroll {
        selected
    } else if selected >= scroll + visible {
        selected + 1 - visible
    } else {
        scroll
    };
    scroll.min(total.saturating_sub(visible))
}
