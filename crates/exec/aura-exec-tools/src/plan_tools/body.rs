//! Tidying the plan body the model sent.

/// Whether `line` opens or closes a code fence: the fence character and
/// how many of it start the line.
fn fence_of(line: &str) -> Option<(char, usize)> {
    let trimmed = line.trim_start();
    let first = trimmed.chars().next().filter(|c| matches!(c, '`' | '~'))?;
    let run = trimmed.chars().take_while(|&c| c == first).count();
    (run >= 3).then_some((first, run))
}

/// Whether `line` is the `# ` heading `name`, which the generated header
/// already shows.
fn is_title_heading(line: &str, name: &str) -> bool {
    line.strip_prefix("# ").is_some_and(|text| {
        text.trim_end_matches('#').trim().to_lowercase() == name.trim().to_lowercase()
    })
}

/// Tidy a plan body the model sent, without reformatting its Markdown:
///
/// - CRLF becomes LF. A body with no newline whose literal `\n`
///   sequences clearly encode structure (escaped twice by the model; see
///   [`has_escaped_structure`]) gets real newlines.
/// - Trailing whitespace is stripped from every line.
/// - Runs of blank lines outside code fences collapse to one.
/// - A leading `# ` heading equal to `name` (ignoring case) is dropped,
///   since the generated header shows the title.
/// - Leading and trailing blank lines are trimmed; there is no trailing
///   newline.
pub(super) fn normalize_body(body: &str, name: &str) -> String {
    let mut text = body.replace("\r\n", "\n");
    if !text.contains('\n') && has_escaped_structure(&text) {
        text = unescape_newlines(&text);
    }
    let mut lines = text.lines().map(str::trim_end).skip_while(|l| l.is_empty());
    let mut kept: Vec<&str> = Vec::new();
    match lines.next() {
        Some(first) if is_title_heading(first, name) => {}
        Some(first) => kept.push(first),
        None => return String::new(),
    }
    // The open fence's character and length; it closes on a run at least
    // as long with nothing after it.
    let mut fence: Option<(char, usize)> = None;
    if let Some(&first) = kept.first() {
        fence = fence_of(first);
    }
    for line in lines {
        if kept.is_empty() && line.is_empty() {
            continue;
        }
        match (fence, fence_of(line)) {
            (None, Some(open)) => fence = Some(open),
            (Some((ch, len)), Some((c, run)))
                if c == ch && run >= len && line.trim().chars().all(|x| x == ch) =>
            {
                fence = None;
            }
            (None, None) if line.is_empty() && kept.last().is_some_and(|l| l.is_empty()) => {
                continue;
            }
            _ => {}
        }
        kept.push(line);
    }
    while kept.last().is_some_and(|l| l.is_empty()) {
        kept.pop();
    }
    kept.join("\n")
}

/// Whether a single-line `text` looks like Markdown whose newlines were
/// escaped: some literal `\n` is followed by another one (a blank line)
/// or by a line opener (`#`, `-`, `*`, `>`, a digit or a code fence).
/// A literal `\n` alone, as in "replace `\n` with `\r\n`" or `C:\new`, is
/// prose. An escaped backslash (`\\n`) is never an escape of `n`.
fn has_escaped_structure(text: &str) -> bool {
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        // Consuming the escaped character keeps `\\n` from matching.
        if c != '\\' || chars.next() != Some('n') {
            continue;
        }
        let rest = chars.as_str();
        if rest.starts_with("\\n")
            || rest.starts_with("\\r\\n")
            || rest.starts_with("```")
            || rest.starts_with(|next: char| {
                matches!(next, '#' | '-' | '*' | '>') || next.is_ascii_digit()
            })
        {
            return true;
        }
    }
    false
}

/// `text` with literal `\n` and `\r\n` as newlines; other escapes,
/// including `\\`, are left as they are.
fn unescape_newlines(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') if chars.as_str().starts_with("\\n") => {
                chars.nth(1);
                out.push('\n');
            }
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}
