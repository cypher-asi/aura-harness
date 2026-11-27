//! Markdown rendering for the terminal renderer.
//!
//! Fenced code blocks are split out by [`parse_content_segments`]; the text
//! between them goes through [`render_markdown`], which handles headings,
//! rules, blockquotes, lists, task lists, tables, and inline styling, and
//! wraps the result to the panel width.
//!
//! A list item that starts with a label, `- Path: src/lib.rs`, is a field:
//! its label is muted, the blank lines between two fields are not drawn,
//! and a blank row sets each record apart where the first label of a run
//! of fields comes back (see [`fields`]).

mod fields;
mod inline;
mod rendered;
mod table;
#[cfg(test)]
mod tests;
mod urls;
mod wrap;

use super::text::Measure;
use crate::Theme;
use inline::{parse_linked, Linked};
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};
pub(crate) use rendered::Rendered;
use std::borrow::Cow;
pub(crate) use wrap::wrap_spans;

/// Bullet glyphs by nesting depth, cycling for deeper lists.
const BULLETS: [&str; 3] = ["•", "◦", "▪"];

/// Render a markdown text segment (no fenced code) into lines that fit
/// `width` display columns. A `width` of 0 disables wrapping.
pub fn render_markdown(text: &str, width: usize, base: Style, theme: &Theme) -> Vec<Line<'static>> {
    render_markdown_flow(text, width, base, theme, Measure::GRID).0
}

/// [`render_markdown`] with flowing text (paragraphs, headings, list
/// items, quotes) as wide as `measure` makes it, and for each line whether
/// it is such text rather than a table or a rule, whose columns have to
/// line up: `Some` of the columns its marker, quote bar or indent takes.
pub(crate) fn render_markdown_flow(
    text: &str,
    width: usize,
    base: Style,
    theme: &Theme,
    measure: Measure,
) -> (Vec<Line<'static>>, Vec<Option<u16>>) {
    let (rendered, flow) = render_markdown_linked(text, width, (base, theme), measure);
    (rendered.lines, flow)
}

/// [`render_markdown_flow`], with where the links a click opens fell.
pub(crate) fn render_markdown_linked(
    text: &str,
    width: usize,
    (base, theme): (Style, &Theme),
    measure: Measure,
) -> (Rendered, Vec<Option<u16>>) {
    let expanded: Vec<Cow<'_, str>> = text.lines().map(expand_tabs).collect();
    let lines: Vec<&str> = expanded.iter().map(AsRef::as_ref).collect();
    let plan = fields::plan(&lines);

    let mut out = Rendered::default();
    let mut flow = Vec::new();
    let mut previous = None;
    let mut i = 0;
    while i < lines.len() {
        if plan[i].skip {
            // As if the line were not there, so the item after it
            // follows the one before like the next line
            i += 1;
            continue;
        }
        let table = table::starts_at(&lines, i);
        let block = if table {
            Some(Block::Table)
        } else {
            Block::of(lines[i], previous)
        };
        let parted = plan[i].gap || Block::parts(previous, block);
        if parted && out.lines.last().is_some_and(|last| last.width() > 0) {
            out.lines.push(Line::default());
            flow.push(Some(0));
        }
        previous = block;
        if table {
            let end = table::end(&lines, i);
            out.lines
                .extend(table::render(&lines[i..end], width, base, theme));
            flow.resize(out.lines.len(), None);
            i = end;
        } else {
            let indent = render_block_line(&mut out, lines[i], width, (base, theme), measure);
            let prose = (!is_rule(lines[i].trim_start()))
                .then(|| u16::try_from(indent).unwrap_or(u16::MAX));
            flow.resize(out.lines.len(), prose);
            i += 1;
        }
    }
    (out, flow)
}

/// The kind of block a line of markdown belongs to. A blank row goes
/// between two blocks of different kinds that the text did not already
/// part with an empty line, and before every heading, so a heading, the
/// paragraph under it and a list after that read as separate blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Block {
    Paragraph,
    List,
    Heading,
    Quote,
    Table,
}

impl Block {
    /// The block `line` is part of, given the block of the line before
    /// it. `None` for an empty line or a rule, which part blocks already.
    /// An indented line after a list item continues the item.
    fn of(line: &str, previous: Option<Self>) -> Option<Self> {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || is_rule(trimmed) {
            None
        } else if heading(trimmed).is_some() {
            Some(Self::Heading)
        } else if trimmed.starts_with('>') {
            Some(Self::Quote)
        } else if list_item(trimmed).is_some()
            || (previous == Some(Self::List) && trimmed.len() < line.len())
        {
            Some(Self::List)
        } else {
            Some(Self::Paragraph)
        }
    }

    /// Whether a blank row goes between a line of block `previous` and
    /// the next line, of block `next`, when the line before has text: a
    /// heading gets one after a rule too.
    fn parts(previous: Option<Self>, next: Option<Self>) -> bool {
        match (previous, next) {
            (_, Some(Self::Heading)) => true,
            (Some(previous), Some(next)) => previous != next,
            _ => false,
        }
    }
}

/// Whether a span is inline code, which stays monospace in flowing text.
pub(crate) fn is_inline_code(style: Style, theme: &Theme) -> bool {
    style.bg == Some(theme.colors.surface)
}

/// Render one line of markdown, returning how many columns of the lines
/// it added are a marker, quote bar or indent rather than text.
fn render_block_line(
    out: &mut Rendered,
    line: &str,
    width: usize,
    (base, theme): (Style, &Theme),
    measure: Measure,
) -> usize {
    let trimmed = line.trim_start();
    let indent = line.len() - trimmed.len();

    if trimmed.is_empty() {
        out.lines.push(Line::default());
        0
    } else if let Some((level, title)) = heading(trimmed) {
        let linked = parse_linked(title, heading_style(level, base, theme), theme);
        out.wrap(linked, width, (&[], &[]), measure);
        0
    } else if is_rule(trimmed) {
        out.lines.push(Line::from(Span::styled(
            "─".repeat(width.max(3)),
            Style::default().fg(theme.colors.muted),
        )));
        0
    } else if trimmed.starts_with('>') {
        render_quote(out, trimmed, width, (base, theme), measure)
    } else if let Some(item) = list_item(trimmed) {
        render_list_item(out, indent, &item, width, (base, theme), measure)
    } else {
        let pad = indent_spans(indent);
        let linked = parse_linked(trimmed, base, theme);
        out.wrap(linked, width, (&pad, &pad), measure);
        indent
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
        _ => bold.fg(theme.colors.foreground),
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
    out: &mut Rendered,
    line: &str,
    width: usize,
    (base, theme): (Style, &Theme),
    measure: Measure,
) -> usize {
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

    let mut inner = Rendered::default();
    let inner_indent = render_block_line(
        &mut inner,
        rest,
        inner_width,
        (base.add_modifier(Modifier::ITALIC), theme),
        measure,
    );
    let bar_width = bar.width();
    out.quote(inner, &bar);
    bar_width + inner_indent
}

struct ListItem<'a> {
    /// The marker as written: `-`, `*`, `+`, `1.`, `2)`.
    marker: &'a str,
    ordered: bool,
    /// `Some(done)` for `[ ]` / `[x]` task items.
    task: Option<bool>,
    body: &'a str,
}

/// Whether `trimmed`, a line without its indent, is a list item. `- - -`
/// is a rule.
fn is_list_item(trimmed: &str) -> bool {
    !is_rule(trimmed) && list_item(trimmed).is_some()
}

/// Whether `line`, the line before a gap, may belong to a list: it is an
/// item, or it is indented, so it may be part of one.
pub(crate) fn may_continue_list(line: &str) -> bool {
    let line = expand_tabs(line);
    let trimmed = line.trim_start();
    trimmed.len() < line.len() || is_list_item(trimmed)
}

/// Whether how `line`, the line after a gap, is drawn may depend on the
/// list before the gap: it is indented, so it may continue an item, or it
/// is a field, whose blank lines and record depend on the items above it
/// (see [`fields`]). A list is laid out as a whole across such gaps.
pub(crate) fn may_join_list(line: &str) -> bool {
    let line = expand_tabs(line);
    let trimmed = line.trim_start();
    trimmed.len() < line.len() || fields::is_field(trimmed)
}

/// Columns of indent before a line's text.
fn indent(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// `line` with each tab as four spaces, as it is laid out.
fn expand_tabs(line: &str) -> Cow<'_, str> {
    if line.contains('\t') {
        Cow::Owned(line.replace('\t', "    "))
    } else {
        Cow::Borrowed(line)
    }
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
    out: &mut Rendered,
    indent: usize,
    item: &ListItem<'_>,
    width: usize,
    (base, theme): (Style, &Theme),
    measure: Measure,
) -> usize {
    let marker_style = base.fg(theme.colors.muted);
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

    let linked = item_spans(item, base, theme);
    out.wrap(linked, width, (&prefix, &hang), measure);
    hang_width
}

/// An item's text. A field's plain label is muted, so the values read
/// apart from the names; a bold one keeps the emphasis its author gave
/// it. Whether it is muted depends on the item alone, so a label does not
/// change color while the items after it stream in.
fn item_spans(item: &ListItem<'_>, base: Style, theme: &Theme) -> Linked {
    let plain = item
        .task
        .is_none()
        .then(|| fields::label(item.body))
        .flatten()
        .filter(|label| !label.bold);
    let Some(label) = plain else {
        return parse_linked(item.body, base, theme);
    };
    let mut linked = Linked::plain(vec![Span::styled(
        format!("{}:", label.text),
        base.fg(theme.colors.muted),
    )]);
    linked.append(parse_linked(label.rest, base, theme));
    linked
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

/// Parse message content into segments of text and code blocks. A fence
/// left open runs to the end, so code still streaming in is drawn as code.
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

    if in_code_block {
        segments.push(ContentSegment::CodeBlock {
            language: code_language,
            code: code_content,
        });
    } else if !current_text.is_empty() {
        segments.push(ContentSegment::Text(current_text));
    }

    segments
}
