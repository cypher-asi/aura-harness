//! Reading a plan's generated header, checklist and body.

use super::frontmatter::Frontmatter;
use super::PlanTodo;
use crate::app::plans::{checklist_before, PLAN_HEADER_END};
use crate::events::TodoState;

/// The parts of a plan, before they become a [`PlanDoc`].
pub(super) struct Parts {
    pub(super) title: String,
    pub(super) overview: String,
    pub(super) todos: Vec<PlanTodo>,
    pub(super) body: String,
}

/// The YAML between a leading `---` line and the next one, and the lines
/// after it. No frontmatter without both lines.
pub(super) fn split_frontmatter<'a, 'b>(lines: &'b [&'a str]) -> (&'b [&'a str], &'b [&'a str]) {
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
pub(super) fn header_end(lines: &[&str], meta: &Frontmatter) -> Option<usize> {
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
pub(super) fn from_header(lines: &[&str], marker: usize, meta: &Frontmatter) -> Parts {
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
pub(super) fn from_frontmatter(lines: &[&str], meta: &Frontmatter) -> Parts {
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
pub(super) fn front_todos(meta: &Frontmatter) -> Vec<PlanTodo> {
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
pub(super) fn sanitize(text: &str) -> String {
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

pub(super) fn is_todos_heading(lines: &[&str], index: usize) -> bool {
    lines.get(index).map(|line| line.trim_end()) == Some("## Todos")
}

/// The todo of a checklist item: its first line `- [x] text` and the
/// 2-space continuation lines after it.
pub(super) fn checklist_todo(lines: &[&str]) -> Option<PlanTodo> {
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
pub(super) fn checklist_item(line: &str) -> Option<(TodoState, &str)> {
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
pub(super) fn take_assignee(content: &mut String) -> Option<String> {
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
pub(super) fn body_text(lines: &[&str], title: Option<&str>) -> String {
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
