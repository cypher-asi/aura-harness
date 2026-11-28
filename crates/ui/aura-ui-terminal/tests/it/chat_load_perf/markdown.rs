//! Markdown edits that take parts of a reply out, or pick them out, so their
//! costs can be told apart.

/// `text` with every fenced code block replaced by a blank line.
pub(crate) fn strip_fences(text: &str) -> String {
    let mut out = String::new();
    let mut in_fence = false;
    for line in text.lines() {
        if line.starts_with("```") {
            in_fence = !in_fence;
            out.push('\n');
            continue;
        }
        if !in_fence {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// `text` with the rows of every markdown table taken out.
pub(crate) fn drop_tables(text: &str) -> String {
    text.lines()
        .filter(|line| !line.starts_with('|'))
        .map(|line| format!("{line}\n"))
        .collect()
}

/// The code of every fenced block in `text`.
pub(crate) fn fenced_code(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current: Option<String> = None;
    for line in text.lines() {
        if line.starts_with("```") {
            match current.take() {
                Some(code) => out.push(code),
                None => current = Some(String::new()),
            }
            continue;
        }
        if let Some(code) = &mut current {
            code.push_str(line);
            code.push('\n');
        }
    }
    out
}
