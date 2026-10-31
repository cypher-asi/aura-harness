//! Markdown rendering for the terminal renderer.
//!
//! Fenced code blocks are split out by [`parse_content_segments`]; the text
//! between them goes through [`render_markdown`], which handles headings,
//! rules, blockquotes, lists, task lists, tables, and inline styling, and
//! wraps the result to the panel width.

mod inline;
mod table;
#[cfg(test)]
mod tests;
mod wrap;

use crate::Theme;
use inline::parse_inline;
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};
use wrap::wrap_spans;

/// Bullet glyphs by nesting depth, cycling for deeper lists.
const BULLETS: [&str; 3] = ["•", "◦", "▪"];

/// Render a markdown text segment (no fenced code) into lines that fit
/// `width` display columns. A `width` of 0 disables wrapping.
pub fn render_markdown(text: &str, width: usize, base: Style, theme: &Theme) -> Vec<Line<'static>> {
    let expanded: Vec<String> = text
        .lines()
        .map(|line| line.replace('\t', "    "))
        .collect();
    let lines: Vec<&str> = expanded.iter().map(String::as_str).collect();

    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if table::starts_at(&lines, i) {
            let end = table::end(&lines, i);
            out.extend(table::render(&lines[i..end], width, base, theme));
            i = end;
        } else {
            render_block_line(&mut out, lines[i], width, base, theme);
            i += 1;
        }
    }
    out
}

fn render_block_line(
    out: &mut Vec<Line<'static>>,
    line: &str,
    width: usize,
    base: Style,
    theme: &Theme,
) {
    let trimmed = line.trim_start();
    let indent = line.len() - trimmed.len();

    if trimmed.is_empty() {
        out.push(Line::default());
    } else if let Some((level, title)) = heading(trimmed) {
        let spans = parse_inline(title, heading_style(level, base, theme), theme);
        out.extend(wrap_spans(&spans, width, &[], &[]));
    } else if is_rule(trimmed) {
        out.push(Line::from(Span::styled(
            "─".repeat(width.max(3)),
            Style::default().fg(theme.colors.muted),
        )));
    } else if trimmed.starts_with('>') {
        render_quote(out, trimmed, width, base, theme);
    } else if let Some(item) = list_item(trimmed) {
        render_list_item(out, indent, &item, width, base, theme);
    } else {
        let pad = indent_spans(indent);
        let spans = parse_inline(trimmed, base, theme);
        out.extend(wrap_spans(&spans, width, &pad, &pad));
    }
}

fn indent_spans(indent: usize) -> Vec<Span<'static>> {
    if indent == 0 {
        Vec::new()
    } else {
        vec![Span::raw(" ".repeat(indent))]
    }
}

/// `## Title ##` → `(2, "Title")`.
fn heading(line: &str) -> Option<(usize, &str)> {
    let level = line.bytes().take_while(|&b| b == b'#').count();
    if !(1..=6).contains(&level) {
        return None;
    }
    let rest = &line[level..];
    if !rest.is_empty() && !rest.starts_with(' ') {
        return None;
    }
    let title = rest.trim();
    let title = match title.trim_end_matches('#') {
        stripped if stripped.is_empty() || stripped.ends_with(' ') => stripped.trim_end(),
        _ => title,
    };
    Some((level, title))
}

fn heading_style(level: usize, base: Style, theme: &Theme) -> Style {
    let bold = base.add_modifier(Modifier::BOLD);
    match level {
        1 => bold
            .fg(theme.colors.primary)
            .add_modifier(Modifier::UNDERLINED),
        2 => bold.fg(theme.colors.primary),
        _ => bold,
    }
}

/// `---`, `***`, `___`, optionally spaced (`- - -`).
fn is_rule(line: &str) -> bool {
    let mut marks = line.chars().filter(|c| !c.is_whitespace());
    let Some(first) = marks.next() else {
        return false;
    };
    matches!(first, '-' | '*' | '_') && marks.clone().all(|c| c == first) && marks.count() >= 2
}

/// Blockquote lines get a `│ ` bar per nesting level and their content is
/// rendered as markdown in the remaining width.
fn render_quote(
    out: &mut Vec<Line<'static>>,
    line: &str,
    width: usize,
    base: Style,
    theme: &Theme,
) {
    let mut depth = 0;
    let mut rest = line;
    while let Some(inner) = rest.trim_start().strip_prefix('>') {
        depth += 1;
        rest = inner.strip_prefix(' ').unwrap_or(inner);
    }
    let bar = Span::styled("│ ".repeat(depth), Style::default().fg(theme.colors.muted));
    let inner_width = if width == 0 {
        0
    } else {
        width.saturating_sub(bar.width()).max(1)
    };

    let mut inner_lines = Vec::new();
    render_block_line(
        &mut inner_lines,
        rest,
        inner_width,
        base.add_modifier(Modifier::ITALIC),
        theme,
    );
    for mut quoted in inner_lines {
        quoted.spans.insert(0, bar.clone());
        out.push(quoted);
    }
}

struct ListItem<'a> {
    /// The marker as written: `-`, `*`, `+`, `1.`, `2)`.
    marker: &'a str,
    ordered: bool,
    /// `Some(done)` for `[ ]` / `[x]` task items.
    task: Option<bool>,
    body: &'a str,
}

fn list_item(line: &str) -> Option<ListItem<'_>> {
    let digits = line.bytes().take_while(u8::is_ascii_digit).count();
    let marker_len =
        if (1..=9).contains(&digits) && matches!(line.as_bytes().get(digits), Some(b'.' | b')')) {
            digits + 1
        } else if matches!(line.as_bytes().first(), Some(b'-' | b'*' | b'+')) {
            1
        } else {
            return None;
        };
    let after = &line[marker_len..];
    if !after.is_empty() && !after.starts_with(' ') {
        return None;
    }
    let body = after.trim_start();
    let (task, body) = match body.get(..3) {
        Some("[ ]") if body.len() == 3 || body[3..].starts_with(' ') => {
            (Some(false), body[3..].trim_start())
        }
        Some("[x]" | "[X]") if body.len() == 3 || body[3..].starts_with(' ') => {
            (Some(true), body[3..].trim_start())
        }
        _ => (None, body),
    };
    Some(ListItem {
        marker: &line[..marker_len],
        ordered: digits > 0,
        task,
        body,
    })
}

/// A list item with its marker; wrapped lines hang under the item text.
fn render_list_item(
    out: &mut Vec<Line<'static>>,
    indent: usize,
    item: &ListItem<'_>,
    width: usize,
    base: Style,
    theme: &Theme,
) {
    let marker_style = base.fg(theme.colors.primary);
    let mut prefix = indent_spans(indent);
    prefix.push(if item.ordered {
        Span::styled(format!("{} ", item.marker), marker_style)
    } else {
        Span::styled(
            format!("{} ", BULLETS[(indent / 2) % BULLETS.len()]),
            marker_style,
        )
    });
    match item.task {
        Some(true) => prefix.push(Span::styled("[✓] ", base.fg(theme.colors.success))),
        Some(false) => prefix.push(Span::styled("[ ] ", base.fg(theme.colors.muted))),
        None => {}
    }
    let hang_width: usize = prefix.iter().map(Span::width).sum();
    let hang = vec![Span::raw(" ".repeat(hang_width))];

    let spans = parse_inline(item.body, base, theme);
    out.extend(wrap_spans(&spans, width, &prefix, &hang));
}

// ============================================================================
// Code Block Parsing
// ============================================================================

/// Content segment - either regular text or a code block.
#[derive(Debug)]
pub enum ContentSegment {
    Text(String),
    CodeBlock { language: String, code: String },
}

/// Parse message content into segments of text and code blocks.
pub fn parse_content_segments(content: &str) -> Vec<ContentSegment> {
    let mut segments = Vec::new();
    let mut current_text = String::new();
    let mut in_code_block = false;
    let mut code_language = String::new();
    let mut code_content = String::new();

    for line in content.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with("```") {
            if in_code_block {
                segments.push(ContentSegment::CodeBlock {
                    language: std::mem::take(&mut code_language),
                    code: std::mem::take(&mut code_content),
                });
                in_code_block = false;
            } else {
                if !current_text.is_empty() {
                    segments.push(ContentSegment::Text(std::mem::take(&mut current_text)));
                }
                code_language = trimmed.trim_start_matches('`').to_string();
                in_code_block = true;
            }
        } else if in_code_block {
            if !code_content.is_empty() {
                code_content.push('\n');
            }
            code_content.push_str(line);
        } else {
            if !current_text.is_empty() {
                current_text.push('\n');
            }
            current_text.push_str(line);
        }
    }

    // Flush remaining content
    if in_code_block {
        if current_text.is_empty() {
            current_text = format!("```{code_language}");
        } else {
            current_text.push_str("\n```");
            current_text.push_str(&code_language);
        }
        if !code_content.is_empty() {
            current_text.push('\n');
            current_text.push_str(&code_content);
        }
    }

    if !current_text.is_empty() {
        segments.push(ContentSegment::Text(current_text));
    }

    segments
}
