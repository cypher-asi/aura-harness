//! The generated header: title, overview and todo checklist.

use super::{PlanMeta, PlanTodo, HEADER_END};

/// `# name`, the overview, the `## Todos` checklist and the
/// [`HEADER_END`] line, each block followed by a blank line except the
/// marker.
pub(super) fn generated_header(meta: &PlanMeta) -> String {
    // A heading is one line; a name with newlines would spill into the body.
    let title = meta.name.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out = format!("# {title}\n\n");
    let overview = meta.overview.trim();
    if !overview.is_empty() {
        out.push_str(overview);
        out.push_str("\n\n");
    }
    if !meta.todos.is_empty() {
        out.push_str("## Todos\n\n");
        for todo in &meta.todos {
            out.push_str(&checklist_item(todo));
            out.push('\n');
        }
        out.push('\n');
    }
    out.push_str(HEADER_END);
    out.push('\n');
    out
}

/// `- [x] content (@assignee)`, with continuation lines indented so a
/// multi-line todo stays one list item.
fn checklist_item(todo: &PlanTodo) -> String {
    let mut out = format!("- {} ", todo.status.checkbox());
    for (index, line) in todo.content.trim().lines().enumerate() {
        let line = line.trim_end();
        if index > 0 {
            out.push('\n');
            if !line.is_empty() {
                out.push_str("  ");
            }
        }
        out.push_str(line);
    }
    if let Some(assignee) = todo.assignee.as_deref().filter(|a| !a.trim().is_empty()) {
        out.push_str(&format!(" (@{})", assignee.trim()));
    }
    out
}

/// The text after the generated header in `text` (what follows the
/// frontmatter), or `None` when there is no header.
///
/// The header `meta` renders to is matched first, so a marker line inside
/// the overview or a todo can't cut the header short; failing that (the
/// header was edited by hand) the first marker line ends it.
pub(super) fn after_header<'a>(meta: &PlanMeta, text: &'a str) -> Option<&'a str> {
    if let Some(rest) = text.strip_prefix(generated_header(meta).as_str()) {
        return Some(rest);
    }
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        offset += line.len();
        if line.trim_end() == HEADER_END {
            return text.get(offset..);
        }
    }
    None
}
