//! Code block component with syntax highlighting.
//!
//! Renders fenced code blocks with language labels, borders, and syntax
//! highlighting. Highlighting and its palette live in
//! [`super::syntax`]; a block is highlighted top to bottom so multi-line
//! constructs (block comments, strings, embedded CSS) keep their colors.

use super::syntax::{highlight_line, syntax_for_token, LineHighlighter};
use crate::themes::Theme;
use ratatui::{
    style::Style,
    text::{Line, Span},
};
use std::ops::Range;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Get a human-readable label for a language.
fn get_language_label(lang: &str) -> &'static str {
    match lang.to_lowercase().as_str() {
        "rust" | "rs" => "Rust",
        "python" | "py" => "Python",
        "javascript" | "js" => "JavaScript",
        "typescript" | "ts" => "TypeScript",
        "jsx" => "JSX",
        "tsx" => "TSX",
        "go" | "golang" => "Go",
        "bash" | "sh" | "shell" | "zsh" => "Shell",
        "json" => "JSON",
        "yaml" | "yml" => "YAML",
        "toml" => "TOML",
        "html" => "HTML",
        "css" => "CSS",
        "sql" => "SQL",
        "markdown" | "md" => "Markdown",
        "c" => "C",
        "cpp" | "c++" | "cxx" => "C++",
        "java" => "Java",
        "ruby" | "rb" => "Ruby",
        "php" => "PHP",
        "swift" => "Swift",
        "kotlin" | "kt" => "Kotlin",
        "scala" => "Scala",
        "haskell" | "hs" => "Haskell",
        "elixir" | "ex" => "Elixir",
        "erlang" | "erl" => "Erlang",
        "clojure" | "clj" => "Clojure",
        "lua" => "Lua",
        "r" => "R",
        "powershell" | "ps1" => "PowerShell",
        "dockerfile" => "Dockerfile",
        "makefile" | "make" => "Makefile",
        "xml" => "XML",
        "graphql" | "gql" => "GraphQL",
        // Other known syntaxes use their own name; unknown ones are "Code".
        _ => syntax_for_token(lang).map_or("Code", |syntax| syntax.name.as_str()),
    }
}

/// Parsed code block ready for rendering.
#[derive(Debug, Clone)]
pub struct CodeBlock {
    /// Language identifier (e.g., "rust", "python")
    pub language: String,
    /// Code content lines
    pub lines: Vec<String>,
}

impl CodeBlock {
    /// Create a new code block.
    #[must_use]
    pub fn new(language: impl Into<String>, content: impl Into<String>) -> Self {
        let content = content.into();
        // Tabs take a column each in width counts but several on screen,
        // so they are laid out as spaces.
        let lines: Vec<String> = content
            .lines()
            .map(|line| line.replace('\t', "    "))
            .collect();
        Self {
            language: language.into(),
            lines,
        }
    }

    /// Render the code block as styled lines, at most `max_width` display
    /// columns wide. A line too long for the box wraps onto the rows
    /// below it, so text never runs into or past the right border.
    #[must_use]
    pub fn render(&self, theme: &Theme, max_width: usize) -> Vec<Line<'static>> {
        self.layout(theme, max_width, false).0
    }

    /// [`Self::render`] with a [`COPY_BUTTON`] at the right end of the top
    /// border, for a caller that acts on a click there. A short block is
    /// widened to make room for it, as far as `max_width` allows; when the
    /// button still does not fit it is left out.
    ///
    /// Returns the lines and the columns the button takes on the first of
    /// them, counted from the box's left edge, when it was drawn.
    #[must_use]
    pub fn render_copyable(
        &self,
        theme: &Theme,
        max_width: usize,
    ) -> (Vec<Line<'static>>, Option<Range<usize>>) {
        self.layout(theme, max_width, true)
    }

    fn layout(
        &self,
        theme: &Theme,
        max_width: usize,
        copyable: bool,
    ) -> (Vec<Line<'static>>, Option<Range<usize>>) {
        let mut highlighter = syntax_for_token(&self.language).map(LineHighlighter::new);
        let label = get_language_label(&self.language);
        let border = Style::default().fg(theme.colors.faint);

        // The box is its text plus a border and a space on each side, and
        // wide enough for what its top border carries.
        let label_width = display_width(label) + 2;
        let top_width = if copyable {
            label_width + display_width(COPY_BUTTON) + 1
        } else {
            label_width + 2
        };
        let content_width = self
            .lines
            .iter()
            .map(|line| display_width(line))
            .max()
            .unwrap_or(0)
            .max(top_width);
        let inner = content_width.min(max_width.saturating_sub(4)).max(1);
        let box_width = inner + 4;

        let (top, button) = top_border(label, box_width, copyable, theme);
        let mut result: Vec<Line<'static>> = vec![top];

        // Code lines with syntax highlighting, each row padded to the box
        for line in &self.lines {
            let highlighted = match &mut highlighter {
                Some(highlighter) => highlighter.spans(line, theme),
                None => vec![Span::styled(
                    line.clone(),
                    Style::default().fg(theme.colors.foreground),
                )],
            };
            let rows = split_columns(highlighted, inner);
            let last = rows.len().saturating_sub(1);
            for (i, (row, used)) in rows.into_iter().enumerate() {
                // A row whose line goes on below pads with the wrap mark,
                // so a copy can tell it from a line that ends there.
                let pad = if i < last { WRAP_PAD } else { ' ' };
                let mut spans = vec![Span::styled("│ ", border)];
                spans.extend(row);
                spans.push(Span::raw(
                    pad.to_string().repeat(inner.saturating_sub(used)),
                ));
                spans.push(Span::styled(format!("{pad}│"), border));
                result.push(Line::from(spans));
            }
        }

        // Bottom border
        let bottom_border = "─".repeat(box_width.saturating_sub(2));
        result.push(Line::from(vec![
            Span::styled("╰", Style::default().fg(theme.colors.faint)),
            Span::styled(bottom_border, Style::default().fg(theme.colors.faint)),
            Span::styled("╯", Style::default().fg(theme.colors.faint)),
        ]));

        (result, button)
    }
}

/// The button a copyable code block draws in its top border.
pub const COPY_BUTTON: &str = " ⧉ Copy ";

/// A `box_width` wide top border: `╭─ Label ───╮`, with the
/// [`COPY_BUTTON`] before its right corner when `copyable` and there is
/// room (`╭─ Label ── ⧉ Copy ─╮`), and plain when even the label does not
/// fit. Returns the columns the button takes, when drawn.
fn top_border(
    label: &str,
    box_width: usize,
    copyable: bool,
    theme: &Theme,
) -> (Line<'static>, Option<Range<usize>>) {
    let border = Style::default().fg(theme.colors.faint);
    let muted = Style::default().fg(theme.colors.muted);
    let label_str = format!(" {label} ");
    let label_width = display_width(&label_str);
    let button_width = display_width(COPY_BUTTON);
    // `╭─`, the label, at least one `─`, the button, then `─╮`
    if copyable && label_width + button_width + 5 <= box_width {
        let fill = box_width - label_width - button_width - 4;
        let start = 2 + label_width + fill;
        let line = Line::from(vec![
            Span::styled("╭─", border),
            Span::styled(label_str, muted),
            Span::styled("─".repeat(fill), border),
            Span::styled(COPY_BUTTON, muted),
            Span::styled("─╮", border),
        ]);
        return (line, Some(start..start + button_width));
    }
    let line = if label_width + 3 <= box_width {
        Line::from(vec![
            Span::styled("╭─", border),
            Span::styled(label_str, muted),
            Span::styled("─".repeat(box_width - label_width - 3), border),
            Span::styled("╮", border),
        ])
    } else {
        Line::from(Span::styled(
            format!("╭{}╮", "─".repeat(box_width.saturating_sub(2))),
            border,
        ))
    };
    (line, None)
}

/// Pads the right edge of a code row whose line wraps onto the row below.
/// It shows as a blank, but marks the row on screen so copied text can
/// join the line back up.
const WRAP_PAD: char = '\u{a0}';

/// Where a code block's frame sits in selected rows.
#[derive(Debug, Clone, Copy)]
enum Frame<'a> {
    /// Its rows start with this (an indent, or the gutter it is drawn in)
    /// before the left border
    At(&'a str),
    /// The selection holds no corner to place it by: the left border is
    /// a row's first non-blank
    Loose,
}

/// A code block's top or bottom border, and the frame it places.
#[derive(Debug, Clone, Copy)]
enum Edge<'a> {
    Top(Frame<'a>),
    Bottom(Frame<'a>),
}

/// Selected screen rows with code block frames taken off: border rows
/// dropped, the `│ ` and ` │` around each code row cut, and a code line
/// wrapped over several rows joined back into one.
///
/// Only rows inside a frame are cut: below a top border `╭─`, above a
/// bottom border `╰─`, or, when the selection holds neither, rows that
/// show a frame's right side `│`. Each is cut where its frame's corner
/// sits, so a gutter the block is drawn in goes too. Tables (`┌`, `├`)
/// and quote bars are left as they are.
///
/// The first and last rows may be cut short by where the selection starts
/// and ends, so either may miss one side of its frame.
#[must_use]
pub fn unframe_code(rows: &[String]) -> Vec<String> {
    let last = rows.len().saturating_sub(1);
    let edges: Vec<Option<Edge<'_>>> = rows
        .iter()
        .enumerate()
        .map(|(i, row)| frame_edge(row, i == 0, i == last))
        .collect();
    let frames = frames_of(rows, &edges);
    let mut lines: Vec<String> = Vec::with_capacity(rows.len());
    let mut joining = false;
    for (i, (row, edge)) in rows.iter().zip(&edges).enumerate() {
        if edge.is_some() {
            joining = false;
            continue;
        }
        let code = frames[i].and_then(|frame| code_row(row, frame, i == 0, i == last));
        match code {
            Some((text, wraps)) => {
                match lines.last_mut().filter(|_| joining) {
                    Some(open) => open.push_str(text),
                    None => lines.push(text.to_owned()),
                }
                joining = wraps;
            }
            None => {
                lines.push(row.clone());
                joining = false;
            }
        }
    }
    lines
}

/// The frame each of `rows` is inside, given the borders among them.
fn frames_of<'a>(rows: &[String], edges: &[Option<Edge<'a>>]) -> Vec<Option<Frame<'a>>> {
    let mut frames = vec![None; edges.len()];
    if edges.iter().all(Option::is_none) {
        // Selected inside one block, as its right border shows; a table's
        // rows end in `│` too, but cross its junctions.
        let framed = rows.iter().any(|row| row.ends_with('│'))
            && !rows
                .iter()
                .any(|row| row.contains(['┼', '├', '┤', '┬', '┴', '┌', '└']));
        if framed {
            frames.fill(Some(Frame::Loose));
        }
        return frames;
    }
    // Below a top border until its bottom one.
    let mut open = None;
    for (frame, edge) in frames.iter_mut().zip(edges) {
        match edge {
            Some(Edge::Top(at)) => open = Some(*at),
            Some(Edge::Bottom(_)) => open = None,
            None => *frame = open,
        }
    }
    // Above a bottom border whose top is not selected.
    let mut closing = None;
    for (frame, edge) in frames.iter_mut().zip(edges).rev() {
        match edge {
            Some(Edge::Bottom(at)) => closing = Some(*at),
            Some(Edge::Top(_)) => closing = None,
            None => *frame = frame.or(closing),
        }
    }
    frames
}

/// `row` as a code block's top or bottom border, whole or cut short by
/// the selection when it is the `first` or `last` row.
fn frame_edge(row: &str, first: bool, last: bool) -> Option<Edge<'_>> {
    if let Some(at) = row.find('╭') {
        let rest = &row[at..];
        if rest.starts_with("╭─") && (rest.ends_with('╮') || last) {
            return Some(Edge::Top(Frame::At(&row[..at])));
        }
    }
    if let Some(at) = row.find('╰') {
        let rest = &row[at..];
        if rest.chars().all(|c| matches!(c, '╰' | '─' | '╯')) && (rest.ends_with('╯') || last)
        {
            return Some(Edge::Bottom(Frame::At(&row[..at])));
        }
    }
    // Selected from inside a border, past its left corner.
    let text = row.trim_start();
    if first && text.contains('─') {
        if text.ends_with('╮') {
            return Some(Edge::Top(Frame::Loose));
        }
        if text.ends_with('╯') && text.chars().all(|c| matches!(c, '─' | '╯')) {
            return Some(Edge::Bottom(Frame::Loose));
        }
    }
    None
}

/// The code in `row` of `frame`, and whether its line wraps onto the next
/// row. A `first` row may miss its left side and a `last` its right.
fn code_row<'a>(
    row: &'a str,
    frame: Frame<'_>,
    first: bool,
    last: bool,
) -> Option<(&'a str, bool)> {
    let after_indent = match frame {
        Frame::At(indent) => row.strip_prefix(indent),
        Frame::Loose => Some(row.trim_start()),
    };
    let left = after_indent.and_then(|text| text.strip_prefix("│ "));
    let inner = left.unwrap_or(row);
    let right = inner.strip_suffix('│').and_then(|rest| {
        let text = rest.trim_end_matches(WRAP_PAD);
        if text.len() < rest.len() {
            Some((text, true))
        } else {
            rest.strip_suffix(' ').map(|text| (text.trim_end(), false))
        }
    });
    match (left.is_some(), right) {
        (true, Some(code)) => Some(code),
        (false, Some(code)) if first => Some(code),
        (true, None) if last => Some((inner.trim_end_matches(WRAP_PAD), false)),
        _ => None,
    }
}

fn display_width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

/// `spans` cut into rows of at most `width` display columns (at least
/// one), each with the columns it takes. A character never straddles two
/// rows: a wide one that does not fit starts the next. An empty line is
/// one empty row.
fn split_columns(spans: Vec<Span<'static>>, width: usize) -> Vec<(Vec<Span<'static>>, usize)> {
    let width = width.max(1);
    let mut rows = Vec::new();
    let mut row: Vec<Span<'static>> = Vec::new();
    let mut used = 0;
    for span in spans {
        let mut piece = String::new();
        for c in span.content.chars() {
            let w = UnicodeWidthChar::width(c).unwrap_or(0);
            if used + w > width && used > 0 {
                if !piece.is_empty() {
                    row.push(Span::styled(std::mem::take(&mut piece), span.style));
                }
                rows.push((std::mem::take(&mut row), used));
                used = 0;
            }
            piece.push(c);
            used += w;
        }
        if !piece.is_empty() {
            row.push(Span::styled(piece, span.style));
        }
    }
    rows.push((row, used.min(width)));
    rows
}

/// Syntax-highlight one line of `language` source (a name such as
/// `rust` or an extension such as `rs`), without the code block frame or
/// any earlier lines for context. Unknown languages come back in plain
/// foreground.
#[must_use]
pub fn highlight_code_line(language: &str, line: &str, theme: &Theme) -> Vec<Span<'static>> {
    match syntax_for_token(language) {
        Some(syntax) => highlight_line(syntax, line, theme),
        None => vec![Span::styled(
            line.to_string(),
            Style::default().fg(theme.colors.foreground),
        )],
    }
}

#[cfg(test)]
mod tests;
