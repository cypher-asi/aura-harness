//! The view shown over the chat panel.

use super::*;

/// What is shown over the chat panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DetailView {
    /// The record highlighted in the Record panel.
    Record,
    /// A file from the sandbox.
    File(Box<FileView>),
    /// A process from the Processes tab, refreshed while it stays listed.
    Process(Box<ProcessSummary>),
    /// The run graph (`/graph`).
    Graph,
    /// A pasted image, as large as the panel allows.
    Image(Box<image_preview::ImageView>),
    /// A thread of mail between agents, from the INBOX tab, kept up to
    /// date as its messages arrive.
    Thread(Box<ThreadView>),
    /// A plan file read as a document: title, overview, todo checklist
    /// and body, without its frontmatter. `s` shows its source.
    Plan(Box<PlanDoc>),
}

impl App {
    /// Record or file shown over the chat panel, if any.
    #[must_use]
    pub const fn detail_view(&self) -> Option<&DetailView> {
        self.detail.view.as_ref()
    }

    /// Show `view` over the chat panel, starting at its top.
    pub(crate) fn open_detail(&mut self, view: DetailView) {
        self.detail.view = Some(view);
        self.detail.scroll = 0;
        self.graph.behind = false;
        self.clear_selection();
    }

    /// Close the view over the chat panel.
    pub(crate) fn close_detail(&mut self) {
        self.detail.view = None;
        self.detail.scroll = 0;
        self.graph.behind = false;
        self.clear_selection();
    }

    /// Whether a screen cell is on the chat panel's close button, drawn
    /// while a view covers the chat or a member's chat is on screen.
    pub(super) fn detail_close_at(&self, column: u16, row: u16) -> bool {
        (self.detail.view.is_some() || self.member_chat().is_some())
            && contains(self.areas.detail_close, column, row)
    }

    /// Open the highlighted sandbox row: a file over the chat panel, a
    /// plan file as a document; a directory, which has nothing to show,
    /// folds or unfolds.
    pub(crate) fn activate_selected_sandbox_row(&mut self) {
        let Some(row) = self.sandbox.rows().get(self.sandbox.selected()) else {
            return;
        };
        match row.kind {
            SandboxEntryKind::File | SandboxEntryKind::Symlink
                if plan_doc::is_plan_file(&row.path) =>
            {
                let path = row.path.clone();
                self.open_plan_doc(&path, None);
            }
            SandboxEntryKind::File | SandboxEntryKind::Symlink => {
                let path = row.path.clone();
                self.open_file(&path);
            }
            SandboxEntryKind::Dir => self.sandbox.toggle_selected(),
            SandboxEntryKind::More(_) => {}
        }
    }

    /// Unfold the highlighted directory; on an open directory or a file,
    /// do nothing.
    pub(crate) fn expand_selected_sandbox_dir(&mut self) {
        let closed_dir = self
            .sandbox
            .rows()
            .get(self.sandbox.selected())
            .is_some_and(|row| row.kind == SandboxEntryKind::Dir && !row.expanded);
        if closed_dir {
            self.sandbox.toggle_selected();
        }
    }

    pub(crate) fn scroll_detail_up(&mut self, lines: usize) {
        self.detail.scroll = self.detail.scroll.saturating_sub(lines);
        self.detail_scrolled();
    }

    pub(crate) fn scroll_detail_down(&mut self, lines: usize) {
        self.detail.scroll = self.detail.scroll.saturating_add(lines);
        self.detail_scrolled();
    }

    /// An open file's cursor follows a scroll instead of pulling it back;
    /// an open plan stops scrolling to its focused todo, so a later frame
    /// does not snap back to it.
    pub(super) fn detail_scrolled(&mut self) {
        match self.detail.view.as_mut() {
            Some(DetailView::File(file)) => file.vim.scrolled(),
            Some(DetailView::Plan(doc)) => doc.scroll_to_focus = false,
            _ => {}
        }
    }

    /// Lines a page key moves the detail view.
    pub(crate) fn detail_page_rows(&self) -> usize {
        self.detail.shown.saturating_sub(1).max(1)
    }

    /// The open file and its scroll, for keys and drawing.
    pub(crate) fn file_view_mut(&mut self) -> Option<(&mut FileView, &mut usize)> {
        match self.detail.view.as_mut() {
            Some(DetailView::File(file)) => Some((file.as_mut(), &mut self.detail.scroll)),
            _ => None,
        }
    }

    /// The open image, to draw it.
    pub(crate) fn image_view_mut(&mut self) -> Option<&mut image_preview::ImageView> {
        match self.detail.view.as_mut() {
            Some(DetailView::Image(view)) => Some(view.as_mut()),
            _ => None,
        }
    }

    /// What the detail view showed in the last frame.
    pub(crate) const fn detail_screen(&self) -> vim::Screen {
        vim::Screen {
            first: self.detail.scroll,
            shown: self.detail.shown,
        }
    }

    /// First line of the detail view to show.
    #[must_use]
    pub const fn detail_scroll(&self) -> usize {
        self.detail.scroll
    }

    /// Clamp the detail view's scroll to `max_scroll` and record that
    /// `shown` source lines fit, as measured by the last frame.
    pub fn set_detail_bounds(&mut self, max_scroll: usize, shown: usize) {
        self.detail.scroll = self.detail.scroll.min(max_scroll);
        self.detail.shown = shown.max(1);
    }

    /// Get the currently selected record (if any).
    #[must_use]
    pub fn selected_record_data(&self) -> Option<&RecordSummary> {
        self.records.items.get(self.records.selected)
    }
}
