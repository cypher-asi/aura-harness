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
        let lines: Vec<String> = content.lines().map(String::from).collect();
        Self {
            language: language.into(),
            lines,
        }
    }

    /// Render the code block as styled lines.
    #[must_use]
    pub fn render(&self, theme: &Theme, max_width: usize) -> Vec<Line<'static>> {
        let mut highlighter = syntax_for_token(&self.language).map(LineHighlighter::new);
        let label = get_language_label(&self.language);

        // Calculate the box width (content width + padding)
        let content_width = self
            .lines
            .iter()
            .map(String::len)
            .max()
            .unwrap_or(0)
            .max(label.len() + 4);
        let box_width = content_width.min(max_width.saturating_sub(4)) + 4; // 2 padding on each side

        let mut result: Vec<Line<'static>> = Vec::new();

        // Top border with language label
        let label_str = format!(" {label} ");
        let border_after_label = "─".repeat(box_width.saturating_sub(label_str.len() + 3));
        result.push(Line::from(vec![
            Span::styled("╭─", Style::default().fg(theme.colors.faint)),
            Span::styled(label_str, Style::default().fg(theme.colors.muted)),
            Span::styled(border_after_label, Style::default().fg(theme.colors.faint)),
            Span::styled("╮", Style::default().fg(theme.colors.faint)),
        ]));

        // Code lines with syntax highlighting
        for line in &self.lines {
            let highlighted = match &mut highlighter {
                Some(highlighter) => highlighter.spans(line, theme),
                None => vec![Span::styled(
                    line.clone(),
                    Style::default().fg(theme.colors.foreground),
                )],
            };
            let line_len = line.len();
            let padding = " ".repeat(box_width.saturating_sub(line_len + 4));

            let mut spans = vec![Span::styled("│ ", Style::default().fg(theme.colors.faint))];
            spans.extend(highlighted);
            spans.push(Span::styled(padding, Style::default()));
            spans.push(Span::styled(" │", Style::default().fg(theme.colors.faint)));

            result.push(Line::from(spans));
        }

        // Bottom border
        let bottom_border = "─".repeat(box_width.saturating_sub(2));
        result.push(Line::from(vec![
            Span::styled("╰", Style::default().fg(theme.colors.faint)),
            Span::styled(bottom_border, Style::default().fg(theme.colors.faint)),
            Span::styled("╯", Style::default().fg(theme.colors.faint)),
        ]));

        result
    }
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

    #[test]
    fn test_language_labels() {
        assert_eq!(get_language_label("rust"), "Rust");
        assert_eq!(get_language_label("py"), "Python");
        assert_eq!(get_language_label("js"), "JavaScript");
        assert_eq!(get_language_label("vue"), "Vue Component");
        assert_eq!(get_language_label(""), "Code");
    }
}
