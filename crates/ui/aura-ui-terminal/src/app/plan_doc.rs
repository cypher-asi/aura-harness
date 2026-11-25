//! A plan file read as a document: the title, overview and todo checklist
//! of its generated header, then the Markdown body its author wrote, with
//! the YAML frontmatter and the header's marker comment left out.
//!
//! This crate does not depend on the tools crate that writes plans, so the
//! format is read here from its text, without a YAML parser. Reading never
//! fails: a plan from before the generated header takes its title, overview
//! and todos from the frontmatter, and any other Markdown file reads as its
//! own body under a title from its first heading or its file name.

use super::file_view::{FileBody, FileView};
use super::plans::{checklist_before, todo_line, PLAN_HEADER_END};
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
    /// Text width, whether text is on the cell grid, and the theme's name
    pub(crate) key: Option<(usize, bool, String)>,
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
    fn focus_on(&mut self, focus: TodoFocus<'_>) {
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
        let Some(DetailView::Plan(doc)) = self.detail.as_ref() else {
            return;
        };
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

    /// The open plan document and the view's scroll, for drawing it.
    pub(crate) fn plan_view_mut(&mut self) -> Option<(&mut PlanDoc, &mut usize)> {
        match self.detail.as_mut() {
            Some(DetailView::Plan(doc)) => Some((doc.as_mut(), &mut self.detail_scroll)),
            _ => None,
        }
    }
}

/// The parts of a plan, before they become a [`PlanDoc`].
struct Parts {
    title: String,
    overview: String,
    todos: Vec<PlanTodo>,
    body: String,
}

/// The YAML between a leading `---` line and the next one, and the lines
/// after it. No frontmatter without both lines.
fn split_frontmatter<'a, 'b>(lines: &'b [&'a str]) -> (&'b [&'a str], &'b [&'a str]) {
    if lines.first().map(|line| line.trim_end()) != Some("---") {
        return (&[], lines);
    }
    let after = lines.get(1..).unwrap_or_default();
    match after.iter().position(|line| line.trim_end() == "---") {
        Some(close) => (
            after.get(..close).unwrap_or_default(),
            after.get(close + 1..).unwrap_or_default(),
        ),
        None => (&[], lines),
    }
}

/// Line of the marker that ends the generated header: the first one with
/// the `## Todos` checklist just before it, else the one after as many
/// markers as the overview quotes (so a marker quoted in the overview is
/// passed over either way), else the first one.
fn header_end(lines: &[&str], meta: &Frontmatter) -> Option<usize> {
    let markers: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.trim_end() == PLAN_HEADER_END)
        .map(|(index, _)| index)
        .collect();
    let quoted = meta
        .overview
        .lines()
        .filter(|line| line.trim_end() == PLAN_HEADER_END)
        .count();
    markers
        .iter()
        .copied()
        .find(|&marker| !checklist_before(lines, marker).is_empty())
        .or_else(|| markers.get(quoted).copied())
        .or_else(|| markers.first().copied())
}

/// Title, overview and todos from the generated header that ends at line
/// `marker`, frontmatter values filling in what it lacks, and the body
/// after it.
fn from_header(lines: &[&str], marker: usize, meta: &Frontmatter) -> Parts {
    let header = lines.get(..marker).unwrap_or_default();
    let start = header.iter().position(|line| !line.trim().is_empty());
    let heading = start.and_then(|start| {
        let line = header.get(start)?;
        Some((start + 1, line.strip_prefix("# ")?.trim().to_string()))
    });
    let (overview_start, title) = match heading {
        Some((after, title)) => (after, title),
        None => (start.unwrap_or(0), meta.name.clone()),
    };
    let items = checklist_before(lines, marker);
    let overview_end = items
        .first()
        .and_then(|&first| {
            (0..first)
                .rev()
                .find(|&index| is_todos_heading(lines, index))
        })
        .unwrap_or(marker);
    let overview = lines
        .get(overview_start..overview_end)
        .map(|overview| overview.join("\n").trim().to_string())
        .filter(|overview| !overview.is_empty())
        .unwrap_or_else(|| meta.overview.clone());
    let todos = items
        .iter()
        .enumerate()
        .filter_map(|(at, &item)| {
            let end = items.get(at + 1).copied().unwrap_or(marker);
            checklist_todo(lines.get(item..end)?)
        })
        .collect();
    Parts {
        title,
        overview,
        todos,
        body: body_text(lines.get(marker + 1..).unwrap_or_default(), None),
    }
}

/// A plan without a generated header: its frontmatter's name, overview
/// and todos, and the rest of the file as the body.
fn from_frontmatter(lines: &[&str], meta: &Frontmatter) -> Parts {
    let title = if meta.name.is_empty() {
        lines
            .iter()
            .find_map(|line| line.strip_prefix("# "))
            .map(|title| title.trim().to_string())
            .unwrap_or_default()
    } else {
        meta.name.clone()
    };
    Parts {
        body: body_text(lines, Some(&title)),
        title,
        overview: meta.overview.clone(),
        todos: front_todos(meta),
    }
}

/// The frontmatter's todos, as the document shows them.
fn front_todos(meta: &Frontmatter) -> Vec<PlanTodo> {
    meta.todos
        .iter()
        .map(|todo| PlanTodo {
            state: todo.state,
            content: todo.content.clone(),
            assignee: todo.assignee.clone(),
        })
        .collect()
}

/// `text` safe to draw: no carriage returns, and other control
/// characters but newlines and tabs (which the Markdown renderer expands)
/// shown as a replacement character instead of reaching the terminal.
fn sanitize(text: &str) -> String {
    text.chars()
        .filter(|&c| c != '\r')
        .map(|c| {
            if c.is_control() && c != '\n' && c != '\t' {
                '\u{fffd}'
            } else {
                c
            }
        })
        .collect()
}

fn is_todos_heading(lines: &[&str], index: usize) -> bool {
    lines.get(index).map(|line| line.trim_end()) == Some("## Todos")
}

/// The todo of a checklist item: its first line `- [x] text` and the
/// 2-space continuation lines after it.
fn checklist_todo(lines: &[&str]) -> Option<PlanTodo> {
    let (first, rest) = lines.split_first()?;
    let (state, text) = checklist_item(first)?;
    let mut content: Vec<&str> = vec![text.trim_end()];
    content.extend(
        rest.iter()
            .map(|line| line.strip_prefix("  ").unwrap_or(line).trim_end()),
    );
    while content.last().is_some_and(|line| line.is_empty()) {
        content.pop();
    }
    let mut content = content.join("\n");
    let assignee = take_assignee(&mut content);
    Some(PlanTodo {
        state,
        content,
        assignee,
    })
}

/// State and text of a checklist item `- [x] text`.
fn checklist_item(line: &str) -> Option<(TodoState, &str)> {
    let mut rest = line.strip_prefix("- [")?.chars();
    let state = match rest.next()? {
        'x' | 'X' => TodoState::Completed,
        '~' => TodoState::InProgress,
        '-' => TodoState::Cancelled,
        _ => TodoState::Pending,
    };
    Some((state, rest.as_str().strip_prefix("] ")?))
}

/// Remove a trailing ` (@assignee)` from `content` and return the name.
fn take_assignee(content: &mut String) -> Option<String> {
    let (text, name) = content.strip_suffix(')')?.rsplit_once(" (@")?;
    let name = name.trim();
    if name.is_empty() || name.contains(['(', ')']) {
        return None;
    }
    let name = name.to_string();
    content.truncate(text.len());
    Some(name)
}

/// `lines` as body text: without blank lines around it, marker lines or,
/// when `title` is given, a leading `# ` heading saying it.
fn body_text(lines: &[&str], title: Option<&str>) -> String {
    let mut kept: Vec<&str> = lines
        .iter()
        .copied()
        .filter(|line| line.trim_end() != PLAN_HEADER_END)
        .skip_while(|line| line.trim().is_empty())
        .collect();
    let repeats_title = kept
        .first()
        .and_then(|line| line.strip_prefix("# "))
        .is_some_and(|text| {
            title.is_some_and(|title| text.trim().eq_ignore_ascii_case(title.trim()))
        });
    if repeats_title {
        kept.remove(0);
    }
    kept.join("\n").trim_matches('\n').trim_end().to_string()
}

/// What a plan's frontmatter says, read without a YAML parser: enough of
/// YAML for what the plan tools write (plain, quoted and block scalars).
#[derive(Debug, Default)]
struct Frontmatter {
    name: String,
    overview: String,
    todos: Vec<FrontTodo>,
}

#[derive(Debug, Default)]
struct FrontTodo {
    id: String,
    content: String,
    state: TodoState,
    assignee: Option<String>,
}

fn read_frontmatter(lines: &[&str]) -> Frontmatter {
    let mut meta = Frontmatter::default();
    let mut index = 0;
    while let Some(line) = lines.get(index) {
        let end = value_end(lines, index, 0);
        if let Some((key, value)) = line.split_once(':').filter(|_| indent_of(line) == 0) {
            let more = lines.get(index + 1..end).unwrap_or_default();
            match key.trim() {
                "name" => meta.name = scalar(value, more),
                "overview" => meta.overview = scalar(value, more),
                "todos" => meta.todos = read_todos(more),
                _ => {}
            }
        }
        index = end.max(index + 1);
    }
    meta
}

/// The items of a `todos:` list: `- key: value` lines, each followed by
/// the item's other keys at the indent of its first.
fn read_todos(lines: &[&str]) -> Vec<FrontTodo> {
    let mut todos = Vec::new();
    let mut key_indent = 0;
    let mut index = 0;
    while let Some(line) = lines.get(index) {
        let trimmed = line.trim_start();
        let entry = if let Some(item) = trimmed.strip_prefix("- ") {
            todos.push(FrontTodo::default());
            key_indent = indent_of(line) + 2;
            item
        } else {
            trimmed
        };
        let end = value_end(lines, index, key_indent);
        if let (Some(todo), Some((key, value))) = (todos.last_mut(), entry.split_once(':')) {
            let value = scalar(value, lines.get(index + 1..end).unwrap_or_default());
            match key.trim() {
                "id" => todo.id = value,
                "content" => todo.content = value,
                "status" => todo.state = todo_state(&value),
                "assignee" if !value.is_empty() && value != "null" && value != "~" => {
                    todo.assignee = Some(value);
                }
                _ => {}
            }
        }
        index = end.max(index + 1);
    }
    todos
}

fn todo_state(status: &str) -> TodoState {
    match status {
        "completed" => TodoState::Completed,
        "in_progress" => TodoState::InProgress,
        "cancelled" => TodoState::Cancelled,
        _ => TodoState::Pending,
    }
}

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start_matches(' ').len()
}

/// End of the value whose key is on line `index`: the next line, past
/// blank ones, indented no deeper than `indent`. A `- ` item at exactly
/// `indent` still belongs to the value, since YAML lets a list sit at its
/// key's indent (`todos:` then `- id: x`, as the plan tools write it).
fn value_end(lines: &[&str], index: usize, indent: usize) -> usize {
    let more = lines.get(index + 1..).unwrap_or_default();
    let ends = |line: &&str| {
        let at = indent_of(line);
        !line.trim().is_empty()
            && (at < indent || (at == indent && !line.trim_start().starts_with("- ")))
    };
    let end = more.iter().position(ends).unwrap_or(more.len());
    // Blank lines before the next key belong to neither.
    let used = more
        .get(..end)
        .and_then(|lines| lines.iter().rposition(|line| !line.trim().is_empty()))
        .map_or(0, |last| last + 1);
    index + 1 + used
}

/// A YAML scalar that starts as `first` after its key and goes on over
/// the more indented lines `more`.
fn scalar(first: &str, more: &[&str]) -> String {
    let first = first.trim();
    if first.starts_with('|') || first.starts_with('>') {
        return block_scalar(first.starts_with('>'), more);
    }
    let mut text = first.to_string();
    for line in more {
        let line = line.trim();
        text.push(if line.is_empty() { '\n' } else { ' ' });
        text.push_str(line);
    }
    if let Some(quoted) = text
        .strip_prefix('\'')
        .and_then(|rest| rest.strip_suffix('\''))
    {
        return quoted.replace("''", "'");
    }
    if let Some(quoted) = text
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
    {
        return unescape(quoted);
    }
    text.trim().to_string()
}

/// A `|` (literal) or `>` (folded) block: its lines without their common
/// indent, joined with newlines or folded into paragraphs.
fn block_scalar(folded: bool, lines: &[&str]) -> String {
    let indent = lines
        .iter()
        .filter(|line| !line.trim().is_empty())
        .map(|line| indent_of(line))
        .min()
        .unwrap_or(0);
    let lines = lines
        .iter()
        .map(|line| line.get(indent..).unwrap_or_default().trim_end());
    if !folded {
        return lines.collect::<Vec<_>>().join("\n");
    }
    let mut text = String::new();
    for line in lines {
        if line.is_empty() {
            text.push('\n');
        } else {
            if !text.is_empty() && !text.ends_with('\n') {
                text.push(' ');
            }
            text.push_str(line);
        }
    }
    text
}

/// A double-quoted YAML string's text with its common escapes resolved.
fn unescape(quoted: &str) -> String {
    let mut out = String::with_capacity(quoted.len());
    let mut chars = quoted.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> PlanDoc {
        PlanDoc::parse(
            text,
            Path::new("/w/.aura/plans/p_00000001.plan.md"),
            ".aura/plans/p_00000001.plan.md",
        )
    }

    fn todo(state: TodoState, content: &str, assignee: Option<&str>) -> PlanTodo {
        PlanTodo {
            state,
            content: content.to_string(),
            assignee: assignee.map(str::to_string),
        }
    }

    #[test]
    fn a_generated_plan_reads_as_title_overview_todos_and_body() {
        let text = format!(
            "---\nname: Plans that read like a document\n\
             overview: Change the guidance so a plan reads as a document.\n\
             todos:\n\
             - id: doc-view\n  content: Add a rendered plan view\n  status: in_progress\n  \
             assignee: rust-engineer\n\
             - id: review\n  content: Review the change\n  status: pending\n---\n\n\
             # Plans that read like a document\n\n\
             Change the guidance so a plan reads as a document.\n\n\
             ## Todos\n\n\
             - [~] Add a rendered plan view (@rust-engineer)\n\
             - [ ] Review the change\n\n\
             {PLAN_HEADER_END}\n\n\
             ## Why plans read badly today\n\nThe agent writes plans.\n\n\
             ```rust\nfn main() {{}}\n```\n"
        );
        let doc = parse(&text);
        assert_eq!(doc.title, "Plans that read like a document");
        assert_eq!(
            doc.overview,
            "Change the guidance so a plan reads as a document."
        );
        assert_eq!(
            doc.todos,
            [
                todo(
                    TodoState::InProgress,
                    "Add a rendered plan view",
                    Some("rust-engineer")
                ),
                todo(TodoState::Pending, "Review the change", None),
            ]
        );
        assert_eq!(
            doc.body,
            "## Why plans read badly today\n\nThe agent writes plans.\n\n\
             ```rust\nfn main() {}\n```"
        );
        assert_eq!(doc.progress(), (0, 2));
        assert_eq!(doc.path, ".aura/plans/p_00000001.plan.md");
    }

    #[test]
    fn a_plan_without_a_generated_header_reads_its_frontmatter() {
        let doc = parse(
            "---\nname: 'It''s old'\noverview: |-\n  First line.\n  Second line.\n\
             todos:\n- id: a\n  content: \"Do \\\"it\\\"\"\n  status: completed\n\
             - id: b\n  content: >-\n    Folded\n    text\n  status: cancelled\n---\n\n\
             # It's old\n\nBody text.\n",
        );
        assert_eq!(doc.title, "It's old");
        assert_eq!(doc.overview, "First line.\nSecond line.");
        assert_eq!(
            doc.todos,
            [
                todo(TodoState::Completed, "Do \"it\"", None),
                todo(TodoState::Cancelled, "Folded text", None),
            ]
        );
        assert_eq!(doc.body, "Body text.", "the title heading is not repeated");
        assert_eq!(doc.progress(), (2, 2));
    }

    #[test]
    fn a_file_with_neither_takes_its_title_from_a_heading_or_its_name() {
        let doc = parse("Intro.\n\n# Heading\n\nMore.\n");
        assert_eq!(doc.title, "Heading");
        assert!(doc.overview.is_empty() && doc.todos.is_empty());
        assert_eq!(doc.body, "Intro.\n\n# Heading\n\nMore.");

        let doc = parse("just text\n");
        assert_eq!(doc.title, "p_00000001");
        assert_eq!(doc.body, "just text");
    }

    #[test]
    fn multi_line_and_cancelled_todos_join_their_continuation_lines() {
        let text = format!(
            "---\nname: P\ntodos: []\n---\n\n# P\n\nOverview one.\n\nOverview two.\n\n\
             ## Todos\n\n\
             - [x] done\n\
             - [~] second\n  with details (@rust-engineer)\n\
             - [ ] third\n\n  after a blank\n\
             - [-] dropped\n\n\
             {PLAN_HEADER_END}\n\nBody.\n{PLAN_HEADER_END}\nMore.\n"
        );
        let doc = parse(&text);
        assert_eq!(doc.overview, "Overview one.\n\nOverview two.");
        assert_eq!(
            doc.todos,
            [
                todo(TodoState::Completed, "done", None),
                todo(
                    TodoState::InProgress,
                    "second\nwith details",
                    Some("rust-engineer")
                ),
                todo(TodoState::Pending, "third\n\nafter a blank", None),
                todo(TodoState::Cancelled, "dropped", None),
            ]
        );
        assert_eq!(doc.body, "Body.\nMore.", "marker lines never show");
        assert_eq!(doc.progress(), (2, 4));
    }

    #[test]
    fn a_marker_quoted_in_the_overview_does_not_end_the_header() {
        let text = format!(
            "---\nname: P\n---\n\n# P\n\nSee {PLAN_HEADER_END}\n{PLAN_HEADER_END}\n\n\
             ## Todos\n\n- [ ] one\n\n{PLAN_HEADER_END}\n\nBody.\n"
        );
        let doc = parse(&text);
        assert_eq!(doc.todos.len(), 1);
        assert_eq!(doc.body, "Body.");
    }

    #[test]
    fn focus_finds_todos_by_id_and_by_position_or_text() {
        let text = format!(
            "---\nname: P\ntodos:\n- id: x\n  content: first\n- id: y\n  content: second\n\
             ---\n\n# P\n\n## Todos\n\n- [ ] first\n- [ ] second\n\n{PLAN_HEADER_END}\n"
        );
        let doc = parse(&text);
        assert_eq!(doc.todo_index(TodoFocus::Id("y")), Some(1));
        assert_eq!(doc.todo_index(TodoFocus::Id("z")), None);
        let nth = |index, count, content| TodoFocus::Nth {
            index,
            count,
            content,
        };
        assert_eq!(doc.todo_index(nth(0, 2, "whatever")), Some(0));
        assert_eq!(
            doc.todo_index(nth(2, 3, "second")),
            Some(1),
            "a count that disagrees falls back to the text"
        );
        assert_eq!(doc.todo_index(nth(2, 3, "gone")), None);
    }

    #[test]
    fn a_marker_quoted_in_the_overview_of_a_plan_without_todos_is_passed_over() {
        // As `plan_tools` writes a plan whose overview quotes the marker.
        let text = format!(
            "---\nname: Tricky\noverview: \"mentions\\n{PLAN_HEADER_END}\\ninside\"\ntodos: []\n---\n\n\
             # Tricky\n\nmentions\n{PLAN_HEADER_END}\ninside\n\n{PLAN_HEADER_END}\n\nBody.\n"
        );
        let doc = parse(&text);
        assert_eq!(doc.title, "Tricky");
        assert_eq!(doc.overview, format!("mentions\n{PLAN_HEADER_END}\ninside"));
        assert_eq!(doc.body, "Body.");
    }

    #[test]
    fn the_frontmatter_todos_win_over_a_stale_checklist() {
        let text = format!(
            "---\nname: P\ntodos:\n\
             - id: x\n  content: first\n  status: completed\n\
             - id: y\n  content: second\n  status: in_progress\n---\n\n\
             # P\n\n## Todos\n\n- [ ] second\n- [ ] first\n\n{PLAN_HEADER_END}\n"
        );
        let doc = parse(&text);
        assert_eq!(
            doc.todos,
            [
                todo(TodoState::Completed, "first", None),
                todo(TodoState::InProgress, "second", None),
            ]
        );
        assert_eq!(doc.todo_index(TodoFocus::Id("y")), Some(1));
    }

    #[test]
    fn control_characters_never_reach_the_terminal() {
        let doc = parse("---\nname: P\n---\r\n\nBody \u{1b}[31mred\u{7}\r\n");
        assert_eq!(doc.body, "Body \u{fffd}[31mred\u{fffd}");
    }

    #[test]
    fn plan_files_are_recognized_by_their_suffix() {
        assert!(is_plan_file(Path::new(".aura/plans/a_1.plan.md")));
        assert!(!is_plan_file(Path::new("README.md")));
    }
}
