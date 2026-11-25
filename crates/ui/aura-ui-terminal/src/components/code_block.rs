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
    pub(crate) fn render_copyable(
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
pub(crate) const COPY_BUTTON: &str = " ⧉ Copy ";

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
pub(crate) fn unframe_code(rows: &[String]) -> Vec<String> {
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
mod tests {
    use super::*;
    use crate::themes::CYAN;

    #[test]
    fn test_code_block_creation() {
        let block = CodeBlock::new("rust", "fn main() {\n    println!(\"Hello\");\n}");
        assert_eq!(block.language, "rust");
        assert_eq!(block.lines.len(), 3);
    }

    #[test]
    fn json_keys_are_plain_and_values_are_strings() {
        let theme = Theme::cyber();
        // Keys are only keys inside an object, so highlight with context.
        let syntax = super::syntax_for_token("json").unwrap();
        let mut highlighter = crate::components::syntax::LineHighlighter::new(syntax);
        highlighter.spans("{", &theme);
        let spans = highlighter.spans(r#"  "tool": "run_command","#, &theme);
        let color_of = |text: &str| {
            spans
                .iter()
                .find(|s| s.content == text)
                .and_then(|s| s.style.fg)
        };
        assert_eq!(color_of(r#""tool""#), Some(theme.colors.foreground));
        assert_eq!(color_of(r#""run_command""#), Some(CYAN));
    }

    #[test]
    fn code_block_carries_state_across_lines() {
        let theme = Theme::cyber();
        let block = CodeBlock::new("rust", "/* open\n   inside */\nfn x() {}");
        let lines = block.render(&theme, 80);
        let inside = lines[2]
            .spans
            .iter()
            .find(|s| s.content.contains("inside"))
            .unwrap();
        assert_eq!(inside.style.fg, Some(crate::themes::GRAY));
    }

    fn text_of(line: &Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    /// Every row is as wide as the top border and no wider than `max`,
    /// and every body row ends on the right border.
    fn assert_boxed(lines: &[Line<'_>], max: usize) {
        let top = display_width(&text_of(&lines[0]));
        assert!(top <= max, "box {top} wider than {max}");
        for line in lines {
            let text = text_of(line);
            assert_eq!(display_width(&text), top, "{text:?}");
        }
        for line in &lines[1..lines.len() - 1] {
            let text = text_of(line);
            assert!(
                text.starts_with("│ ") && (text.ends_with(" │") || text.ends_with("\u{a0}│")),
                "{text:?}"
            );
        }
    }

    #[test]
    fn long_lines_wrap_inside_the_box() {
        let theme = Theme::cyber();
        let code = "DAYTONA_API_KEY=dtn_x\n\
                    # DAYTONA_ORGANIZATION_ID=     # only if the key can reach several organizations\n\
                    # AURA_CLOUD_CPU=4";
        for language in ["dotenv", "bash", ""] {
            let lines = CodeBlock::new(language, code).render(&theme, 40);
            assert_boxed(&lines, 40);
            let body: String = lines[1..lines.len() - 1]
                .iter()
                .map(|line| {
                    let text = text_of(line);
                    text.strip_prefix("│ ")
                        .and_then(|t| t.strip_suffix('│'))
                        .unwrap()
                        .trim_end_matches([' ', '\u{a0}'])
                        .to_owned()
                })
                .collect();
            assert!(body.contains("several organizations"), "{body:?}");
            assert!(lines.len() > 5, "the long line wrapped");
        }
    }

    #[test]
    fn wide_characters_and_tabs_stay_inside_the_box() {
        let theme = Theme::cyber();
        let lines = CodeBlock::new("", "\tlet s = \"日本語のテキストです\";").render(&theme, 16);
        assert_boxed(&lines, 16);
    }

    #[test]
    fn short_blocks_fit_their_text_and_narrow_panes_drop_the_label() {
        let theme = Theme::cyber();
        let lines = CodeBlock::new("rust", "let x = 1;").render(&theme, 80);
        assert_boxed(&lines, 80);
        assert_eq!(text_of(&lines[1]), "│ let x = 1; │");
        let lines = CodeBlock::new("javascript", "x").render(&theme, 8);
        assert_boxed(&lines, 8);
    }

    /// `lines` as screen text, each indented by `indent` like chat rows.
    fn screen_rows(lines: &[Line<'_>], indent: &str) -> Vec<String> {
        lines
            .iter()
            .map(|line| format!("{indent}{}", text_of(line)).trim_end().to_owned())
            .collect()
    }

    #[test]
    fn copying_a_wrapped_command_drops_the_frame_and_joins_the_line() {
        let theme = Theme::cyber();
        let command = "cd ~/code && rm -rf wilder-simm/target textcat/target zrm/backend/target \
                       the-grid/target machina-2/backend/target zero-os/target";
        let code = format!("{command}\necho done  \n\n    indented");
        let lines = CodeBlock::new("bash", &code).render(&theme, 40);
        assert!(lines.len() > 6, "the command wrapped");
        let copied = unframe_code(&screen_rows(&lines, "  "));
        assert_eq!(copied, [command, "echo done", "", "    indented"]);
    }

    #[test]
    fn a_selection_cut_short_still_loses_the_frame() {
        let theme = Theme::cyber();
        let lines =
            CodeBlock::new("bash", "first line\nsecond line\nthird line").render(&theme, 80);
        let mut rows = screen_rows(&lines[1..4], "");
        // Started inside the first code row, ended inside the last.
        rows[0] = rows[0]["│ fir".len()..].to_owned();
        rows[2] = rows[2][.."│ third".len()].to_owned();
        assert_eq!(unframe_code(&rows), ["st line", "second line", "third"]);
    }

    #[test]
    fn rows_outside_a_frame_are_kept_as_they_are() {
        let rows: Vec<String> = ["│ a quote", "│ another │ thing", "plain", "│ tree"]
            .map(str::to_owned)
            .to_vec();
        let copied = unframe_code(&rows);
        assert_eq!(copied[0], "│ a quote");
        assert_eq!(copied[2], "plain");
        assert_eq!(copied[3], "│ tree", "a last row needs the frame above it");
    }

    #[test]
    fn one_or_two_rows_cut_short_lose_the_frame_and_join() {
        let theme = Theme::cyber();
        let command = "cd ~/code && rm -rf wilder-simm/target textcat/target";
        let lines = CodeBlock::new("bash", command).render(&theme, 40);
        assert_eq!(lines.len(), 4, "the command takes two rows");
        let mut rows = screen_rows(&lines[1..3], "  ");
        // Dragged from its first character to its last.
        rows[0] = rows[0].trim_start()["│ ".len()..].to_owned();
        let end = rows[1].find("target").unwrap() + "target".len();
        rows[1].truncate(end);
        assert_eq!(unframe_code(&rows), [command]);

        // One row, dragged from the code past its end.
        let lines = CodeBlock::new("bash", "echo hi").render(&theme, 40);
        let row = text_of(&lines[1])["│ ".len()..].to_owned();
        assert_eq!(unframe_code(&[row]), ["echo hi"]);
    }

    #[test]
    fn tables_keep_their_borders() {
        let table: Vec<String> = [
            "┌──────┬─────┐",
            "│ Name │ Qty │",
            "├──────┼─────┤",
            "│ nuts │ 4   │",
            "└──────┴─────┘",
        ]
        .map(str::to_owned)
        .to_vec();
        assert_eq!(unframe_code(&table), table);
        assert_eq!(unframe_code(&table[1..4]), table[1..4]);
        // Next to a code block, still the table's own.
        let theme = Theme::cyber();
        let mut rows = screen_rows(&CodeBlock::new("bash", "ls").render(&theme, 40), "");
        rows.extend(table.iter().cloned());
        assert_eq!(unframe_code(&rows)[1..], table[..]);
    }

    #[test]
    fn a_block_in_a_gutter_loses_the_gutter_and_the_frame() {
        let theme = Theme::cyber();
        let lines = CodeBlock::new("rust", "let x = 1;\nlet y = 2;").render(&theme, 40);
        let rows = screen_rows(&lines, "  │ ");
        assert_eq!(unframe_code(&rows), ["let x = 1;", "let y = 2;"]);
        // Started below the top border.
        assert_eq!(unframe_code(&rows[2..]), ["let y = 2;"]);
    }

    /// The columns `range` of `line`'s text, for rows of one-column
    /// characters.
    fn columns(line: &Line<'_>, range: Range<usize>) -> String {
        text_of(line)
            .chars()
            .skip(range.start)
            .take(range.len())
            .collect()
    }

    #[test]
    fn copy_button_sits_in_the_top_border_of_a_widened_short_block() {
        let theme = Theme::cyber();
        let block = CodeBlock::new("bash", "ls");
        let plain = block.render(&theme, 80);
        let (lines, button) = block.render_copyable(&theme, 80);
        assert_boxed(&lines, 80);
        let button = button.expect("the button fits in 80 columns");
        assert_eq!(columns(&lines[0], button.clone()), COPY_BUTTON);
        let top = text_of(&lines[0]);
        assert!(top.starts_with("╭─ Shell ─"), "{top:?}");
        assert!(top.ends_with(" ⧉ Copy ─╮"), "{top:?}");
        assert_eq!(button.end + 2, display_width(&top), "the corner follows it");
        assert!(
            display_width(&top) > display_width(&text_of(&plain[0])),
            "the block widened to fit the button"
        );
        assert_eq!(text_of(&lines[1]).trim_end_matches(['│', ' ']), "│ ls");
    }

    #[test]
    fn copy_button_is_left_out_when_the_box_is_too_narrow() {
        let theme = Theme::cyber();
        let (lines, button) = CodeBlock::new("bash", "ls").render_copyable(&theme, 16);
        assert_boxed(&lines, 16);
        assert_eq!(button, None);
        assert!(
            !text_of(&lines[0]).contains("Copy"),
            "{:?}",
            text_of(&lines[0])
        );
    }

    #[test]
    fn plain_render_never_shows_the_copy_button() {
        let theme = Theme::cyber();
        for width in [16, 40, 80, 200] {
            let lines = CodeBlock::new("bash", "echo hi\nls").render(&theme, width);
            assert!(!text_of(&lines[0]).contains("Copy"), "{width}");
        }
    }

    #[test]
    fn selecting_a_copyable_block_copies_just_its_code() {
        let theme = Theme::cyber();
        let (lines, button) = CodeBlock::new("bash", "echo hi\nls").render_copyable(&theme, 80);
        assert!(button.is_some());
        let copied = unframe_code(&screen_rows(&lines, "  "));
        assert_eq!(copied, ["echo hi", "ls"]);
    }

    #[test]
    fn test_language_labels() {
        assert_eq!(get_language_label("rust"), "Rust");
        assert_eq!(get_language_label("py"), "Python");
        assert_eq!(get_language_label("js"), "JavaScript");
        assert_eq!(get_language_label("vue"), "Vue Component");
        assert_eq!(get_language_label(""), "Code");
    }
}
