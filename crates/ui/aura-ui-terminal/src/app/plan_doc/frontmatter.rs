//! Reading a plan's YAML frontmatter without a YAML parser.

use crate::events::TodoState;

/// What a plan's frontmatter says, read without a YAML parser: enough of
/// YAML for what the plan tools write (plain, quoted and block scalars).
#[derive(Debug, Default)]
pub(super) struct Frontmatter {
    pub(super) name: String,
    pub(super) overview: String,
    pub(super) todos: Vec<FrontTodo>,
}

#[derive(Debug, Default)]
pub(super) struct FrontTodo {
    pub(super) id: String,
    pub(super) content: String,
    pub(super) state: TodoState,
    pub(super) assignee: Option<String>,
}

pub(super) fn read_frontmatter(lines: &[&str]) -> Frontmatter {
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
pub(super) fn read_todos(lines: &[&str]) -> Vec<FrontTodo> {
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

pub(super) fn todo_state(status: &str) -> TodoState {
    match status {
        "completed" => TodoState::Completed,
        "in_progress" => TodoState::InProgress,
        "cancelled" => TodoState::Cancelled,
        _ => TodoState::Pending,
    }
}

pub(super) fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start_matches(' ').len()
}

/// End of the value whose key is on line `index`: the next line, past
/// blank ones, indented no deeper than `indent`. A `- ` item at exactly
/// `indent` still belongs to the value, since YAML lets a list sit at its
/// key's indent (`todos:` then `- id: x`, as the plan tools write it).
pub(super) fn value_end(lines: &[&str], index: usize, indent: usize) -> usize {
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
pub(super) fn scalar(first: &str, more: &[&str]) -> String {
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
pub(super) fn block_scalar(folded: bool, lines: &[&str]) -> String {
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
pub(super) fn unescape(quoted: &str) -> String {
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
