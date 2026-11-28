//! A plan file read as a document: the title, overview and todo checklist
//! of its generated header, then the Markdown body its author wrote, with
//! the YAML frontmatter and the header's marker comment left out.
//!
//! This crate does not depend on the tools crate that writes plans, so the
//! format is read here from its text, without a YAML parser. Reading never
//! fails: a plan from before the generated header takes its title, overview
//! and todos from the frontmatter, and any other Markdown file reads as its
//! own body under a title from its first heading or its file name.

mod facts;
mod frontmatter;
mod origin;
mod parse;

pub(crate) use facts::{Fact, FactTone, PlanFacts};
pub use origin::PlanOrigin;

use frontmatter::read_frontmatter;
use parse::{from_frontmatter, from_header, front_todos, header_end, sanitize, split_frontmatter};

use super::file_view::{FileBody, FileView};
use super::plans::todo_line;
use super::{App, DetailView};
use crate::events::TodoState;
use ratatui::text::Line;
use std::path::{Path, PathBuf};

/// Suffix of plan files, which open as documents.
const PLAN_SUFFIX: &str = ".plan.md";

/// One todo, as the plan's checklist shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanTodo {
    /// Where the todo stands
    pub state: TodoState,
    /// What it says; continuation lines are joined with newlines
    pub content: String,
    /// Who works on it, from the item's ` (@assignee)` suffix
    pub assignee: Option<String>,
}

/// A plan file shown as a document over the chat panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanDoc {
    /// Absolute path of the file, for showing its source
    pub file: PathBuf,
    /// Path shown in the header, relative to the sandbox root when inside it
    pub path: String,
    /// The plan's name
    pub title: String,
    /// The paragraph under the title, as Markdown
    pub overview: String,
    /// The todo checklist, in plan order
    pub todos: Vec<PlanTodo>,
    /// When, by whom and where it was written, as far as it says
    pub origin: PlanOrigin,
    /// What the author wrote after the generated header, as Markdown
    pub body: String,
    /// Index in `todos` of the todo the view was opened at
    pub focus: Option<usize>,
    /// Whether the next frame still has to scroll `focus` into view; a
    /// scroll by the user before then wins.
    pub(crate) scroll_to_focus: bool,
    /// The frontmatter's todo ids and contents, in order, for finding a
    /// todo by id
    ids: Vec<(String, String)>,
    /// Rows laid out by the last frame
    pub(crate) layout: PlanLayout,
}

/// Which todo a plan opens at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TodoFocus<'a> {
    /// The todo with this frontmatter id
    Id(&'a str),
    /// Todo number `index` of the `count` the caller knows, saying
    /// `content`; found by its text when the file lists a different number
    Nth {
        index: usize,
        count: usize,
        content: &'a str,
    },
}

/// A plan's rows as laid out for one panel width, face and theme, so a
/// frame that changes none of them draws without laying the plan out
/// again.
#[derive(Debug, Default)]
pub(crate) struct PlanLayout {
    /// Text width, whether text is on the cell grid, the theme's name,
    /// and a hash of the facts shown under the title
    pub(crate) key: Option<(usize, bool, String, u64)>,
    /// One screen row each
    pub(crate) lines: Vec<Line<'static>>,
    /// Per row: `Some` of its indent columns when it is prose
    pub(crate) prose: Vec<Option<u16>>,
    /// Row of each todo's first line, in todo order
    pub(crate) todo_rows: Vec<usize>,
}

/// A copy lays itself out again on its first frame.
impl Clone for PlanLayout {
    fn clone(&self) -> Self {
        Self::default()
    }
}

/// Layouts never make two documents differ.
impl PartialEq for PlanLayout {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Eq for PlanLayout {}

impl PlanDoc {
    /// Read the plan in `text`, from the file at `file` shown as `path`.
    #[must_use]
    pub fn parse(text: &str, file: &Path, path: &str) -> Self {
        let text = sanitize(text);
        let lines: Vec<&str> = text.lines().collect();
        let (front, rest) = split_frontmatter(&lines);
        let meta = read_frontmatter(front);
        let stem = path.strip_suffix(PLAN_SUFFIX).unwrap_or(path);
        let stem = stem.rsplit(['/', '\\']).next().unwrap_or(stem);
        let mut doc = match header_end(rest, &meta) {
            Some(marker) => from_header(rest, marker, &meta),
            None => from_frontmatter(rest, &meta),
        };
        // The frontmatter is what the plan tools keep true; the checklist
        // is only a copy for reading the file raw, and may be stale or
        // hand-edited.
        if !meta.todos.is_empty() {
            doc.todos = front_todos(&meta);
        }
        if doc.title.is_empty() {
            doc.title = stem.to_string();
        }
        Self {
            file: file.to_path_buf(),
            path: path.to_string(),
            title: doc.title,
            overview: doc.overview,
            todos: doc.todos,
            origin: PlanOrigin::from_frontmatter(&meta),
            body: doc.body,
            focus: None,
            scroll_to_focus: false,
            ids: meta
                .todos
                .into_iter()
                .map(|todo| (todo.id, todo.content))
                .collect(),
            layout: PlanLayout::default(),
        }
    }

    /// Read the plan file at `file`, titled relative to `root`, the way
    /// the file view reads files (with its size cap). The file view itself
    /// when the file cannot be read as text, so it shows why.
    pub(super) fn load(file: &Path, root: Option<&Path>) -> Result<Self, Box<FileView>> {
        let view = FileView::load(file, root);
        match &view.body {
            FileBody::Text(text) => Ok(Self::parse(text, file, &view.title)),
            FileBody::Note(_) => Err(Box::new(view)),
        }
    }

    /// Open at `focus`, scrolling to it on the next frame.
    pub(super) fn focus_on(&mut self, focus: TodoFocus<'_>) {
        self.focus = self.todo_index(focus);
        self.scroll_to_focus = self.focus.is_some();
    }

    /// Index in `todos` of the todo `focus` names.
    fn todo_index(&self, focus: TodoFocus<'_>) -> Option<usize> {
        let (index, count, content) = match focus {
            TodoFocus::Id(id) => {
                let index = self.ids.iter().position(|(own, _)| own == id)?;
                let (_, content) = self.ids.get(index)?;
                (index, self.ids.len(), content.as_str())
            }
            TodoFocus::Nth {
                index,
                count,
                content,
            } => (index, count, content),
        };
        if count == self.todos.len() && index < count {
            return Some(index);
        }
        let needle = content.lines().next()?.trim();
        if needle.is_empty() {
            return None;
        }
        self.todos
            .iter()
            .position(|todo| todo.content.lines().next().map(str::trim) == Some(needle))
    }

    /// `(closed, total)` todo counts; cancelled todos count as closed.
    #[must_use]
    pub fn progress(&self) -> (usize, usize) {
        let closed = self
            .todos
            .iter()
            .filter(|todo| matches!(todo.state, TodoState::Completed | TodoState::Cancelled))
            .count();
        (closed, self.todos.len())
    }
}

/// Whether `path` names a plan file, which opens as a document.
pub(super) fn is_plan_file(path: &Path) -> bool {
    path.to_str()
        .is_some_and(|path| path.ends_with(PLAN_SUFFIX))
}

impl App {
    /// Show the plan at `file` as a document over the chat panel, at
    /// `todo` when given; its raw text when it cannot be read as text.
    pub(super) fn open_plan_doc(&mut self, file: &Path, todo: Option<TodoFocus<'_>>) {
        if self.sandbox.is_remote() {
            self.read_remote_file(file, Some(todo));
            return;
        }
        match PlanDoc::load(file, self.sandbox.root()) {
            Ok(mut doc) => {
                if let Some(todo) = todo {
                    doc.focus_on(todo);
                }
                self.open_detail(DetailView::Plan(Box::new(doc)));
            }
            Err(view) => self.open_detail(DetailView::File(view)),
        }
    }

    /// Swap the open plan document for its source in the file view, with
    /// the cursor on the focused todo's checklist item.
    pub(super) fn show_plan_source(&mut self) {
        let Some(DetailView::Plan(doc)) = self.detail.view.as_ref() else {
            return;
        };
        if self.sandbox.is_remote() {
            let file = doc.file.clone();
            self.read_remote_file(&file, None);
            return;
        }
        let mut view = FileView::load(&doc.file, self.sandbox.root());
        let focused = doc
            .focus
            .and_then(|index| Some((index, doc.todos.get(index)?)));
        if let Some(line) = focused
            .and_then(|(index, todo)| todo_line(&view, index, doc.todos.len(), &todo.content))
        {
            view.jump_to(line);
        }
        self.open_detail(DetailView::File(Box::new(view)));
    }

    /// The open plan document, if one is open.
    #[must_use]
    pub(crate) fn plan_view(&self) -> Option<&PlanDoc> {
        match self.detail.view.as_ref() {
            Some(DetailView::Plan(doc)) => Some(doc.as_ref()),
            _ => None,
        }
    }

    /// The open plan document and the view's scroll, for drawing it.
    pub(crate) fn plan_view_mut(&mut self) -> Option<(&mut PlanDoc, &mut usize)> {
        match self.detail.view.as_mut() {
            Some(DetailView::Plan(doc)) => Some((doc.as_mut(), &mut self.detail.scroll)),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests;
