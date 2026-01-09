//! Code block component with syntax highlighting.
//!
//! Renders fenced code blocks with language labels, borders, and basic syntax highlighting.

use crate::themes::Theme;
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

/// Language-specific syntax highlighting configuration.
#[derive(Debug, Clone, Copy)]
pub struct LanguageConfig {
    /// Keywords color
    pub keyword_color: Color,
    /// String literals color
    pub string_color: Color,
    /// Comments color
    pub comment_color: Color,
    /// Numbers color
    pub number_color: Color,
    /// Functions/methods color
    pub function_color: Color,
    /// Types color
    pub type_color: Color,
    /// Operators color
    pub operator_color: Color,
}

impl Default for LanguageConfig {
    fn default() -> Self {
        Self {
            keyword_color: Color::Magenta,
            string_color: Color::Green,
            comment_color: Color::DarkGray,
            number_color: Color::Yellow,
            function_color: Color::Blue,
            type_color: Color::Cyan,
            operator_color: Color::Red,
        }
    }
}

/// Keywords for various languages.
const RUST_KEYWORDS: &[&str] = &[
    "fn", "let", "mut", "const", "static", "if", "else", "match", "for", "while", "loop",
    "return", "break", "continue", "struct", "enum", "impl", "trait", "type", "where",
    "pub", "mod", "use", "crate", "super", "self", "Self", "async", "await", "move",
    "dyn", "ref", "in", "as", "unsafe", "extern", "true", "false", "Some", "None", "Ok", "Err",
];

const PYTHON_KEYWORDS: &[&str] = &[
    "def", "class", "if", "elif", "else", "for", "while", "return", "import", "from",
    "as", "try", "except", "finally", "with", "lambda", "yield", "raise", "pass",
    "break", "continue", "and", "or", "not", "in", "is", "True", "False", "None",
    "async", "await", "self", "cls",
];

const JS_KEYWORDS: &[&str] = &[
    "function", "const", "let", "var", "if", "else", "for", "while", "do", "switch",
    "case", "break", "continue", "return", "try", "catch", "finally", "throw",
    "class", "extends", "new", "this", "super", "import", "export", "default", "from",
    "async", "await", "yield", "true", "false", "null", "undefined", "typeof", "instanceof",
];

const GO_KEYWORDS: &[&str] = &[
    "func", "var", "const", "type", "struct", "interface", "map", "chan", "if", "else",
    "for", "range", "switch", "case", "default", "break", "continue", "return", "go",
    "defer", "select", "package", "import", "true", "false", "nil", "make", "new",
];

const SHELL_KEYWORDS: &[&str] = &[
    "if", "then", "else", "elif", "fi", "for", "while", "do", "done", "case", "esac",
    "function", "return", "exit", "echo", "export", "source", "alias", "cd", "pwd",
    "ls", "rm", "cp", "mv", "mkdir", "chmod", "chown", "grep", "sed", "awk", "cat",
];

/// Get keywords for a language.
fn get_keywords(lang: &str) -> &'static [&'static str] {
    match lang.to_lowercase().as_str() {
        "rust" | "rs" => RUST_KEYWORDS,
        "python" | "py" => PYTHON_KEYWORDS,
        "javascript" | "js" | "typescript" | "ts" | "jsx" | "tsx" => JS_KEYWORDS,
        "go" | "golang" => GO_KEYWORDS,
        "bash" | "sh" | "shell" | "zsh" => SHELL_KEYWORDS,
        _ => &[],
    }
}

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
        "" => "Code",
        other => {
            // Return the input as-is for unknown languages
            // We can't return a &'static str here, so return "Code"
            if other.is_empty() { "Code" } else { "Code" }
        }
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
        let config = LanguageConfig::default();
        let keywords = get_keywords(&self.language);
        let label = get_language_label(&self.language);
        
        // Calculate the box width (content width + padding)
        let content_width = self.lines.iter()
            .map(|l| l.len())
            .max()
            .unwrap_or(0)
            .max(label.len() + 4);
        let box_width = content_width.min(max_width.saturating_sub(4)) + 4; // 2 padding on each side
        
        let mut result: Vec<Line<'static>> = Vec::new();
        
        // Top border with language label
        let label_str = format!(" {} ", label);
        let border_after_label = "─".repeat(box_width.saturating_sub(label_str.len() + 3));
        result.push(Line::from(vec![
            Span::styled("╭─", Style::default().fg(theme.colors.muted)),
            Span::styled(label_str, Style::default().fg(theme.colors.secondary).add_modifier(Modifier::BOLD)),
            Span::styled(border_after_label, Style::default().fg(theme.colors.muted)),
            Span::styled("╮", Style::default().fg(theme.colors.muted)),
        ]));
        
        // Code lines with syntax highlighting
        for line in &self.lines {
            let highlighted = highlight_line(line, keywords, &config, theme);
            let line_len = line.len();
            let padding = " ".repeat(box_width.saturating_sub(line_len + 4));
            
            let mut spans = vec![
                Span::styled("│ ", Style::default().fg(theme.colors.muted)),
            ];
            spans.extend(highlighted);
            spans.push(Span::styled(padding, Style::default()));
            spans.push(Span::styled(" │", Style::default().fg(theme.colors.muted)));
            
            result.push(Line::from(spans));
        }
        
        // Bottom border
        let bottom_border = "─".repeat(box_width.saturating_sub(2));
        result.push(Line::from(vec![
            Span::styled("╰", Style::default().fg(theme.colors.muted)),
            Span::styled(bottom_border, Style::default().fg(theme.colors.muted)),
            Span::styled("╯", Style::default().fg(theme.colors.muted)),
        ]));
        
        result
    }
}

/// Highlight a single line of code.
fn highlight_line(line: &str, keywords: &[&str], config: &LanguageConfig, theme: &Theme) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    
    while i < chars.len() {
        let c = chars[i];
        
        // Check for string literals
        if c == '"' || c == '\'' || c == '`' {
            let quote = c;
            let mut string_content = String::from(c);
            i += 1;
            while i < chars.len() {
                let ch = chars[i];
                string_content.push(ch);
                i += 1;
                if ch == quote && (string_content.len() < 2 || string_content.chars().nth(string_content.len() - 2) != Some('\\')) {
                    break;
                }
            }
            spans.push(Span::styled(string_content, Style::default().fg(config.string_color)));
            continue;
        }
        
        // Check for comments (// or #)
        if c == '/' && i + 1 < chars.len() && chars[i + 1] == '/' {
            let comment: String = chars[i..].iter().collect();
            spans.push(Span::styled(comment, Style::default().fg(config.comment_color)));
            break;
        }
        if c == '#' {
            let comment: String = chars[i..].iter().collect();
            spans.push(Span::styled(comment, Style::default().fg(config.comment_color)));
            break;
        }
        
        // Check for numbers
        if c.is_ascii_digit() {
            let mut num = String::from(c);
            i += 1;
            while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.' || chars[i] == 'x' || chars[i] == 'b' || chars[i].is_ascii_hexdigit()) {
                num.push(chars[i]);
                i += 1;
            }
            spans.push(Span::styled(num, Style::default().fg(config.number_color)));
            continue;
        }
        
        // Check for identifiers and keywords
        if c.is_alphabetic() || c == '_' {
            let mut word = String::from(c);
            i += 1;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                word.push(chars[i]);
                i += 1;
            }
            
            // Check if it's a keyword
            if keywords.contains(&word.as_str()) {
                spans.push(Span::styled(word, Style::default().fg(config.keyword_color).add_modifier(Modifier::BOLD)));
            }
            // Check if followed by ( - likely a function
            else if i < chars.len() && chars[i] == '(' {
                spans.push(Span::styled(word, Style::default().fg(config.function_color)));
            }
            // Check if it looks like a type (starts with uppercase)
            else if word.chars().next().map_or(false, |c| c.is_uppercase()) {
                spans.push(Span::styled(word, Style::default().fg(config.type_color)));
            }
            else {
                spans.push(Span::styled(word, Style::default().fg(theme.colors.foreground)));
            }
            continue;
        }
        
        // Check for operators
        if "+-*/%=<>!&|^~?:".contains(c) {
            let mut op = String::from(c);
            i += 1;
            // Check for multi-char operators
            while i < chars.len() && "+-*/%=<>!&|^~?:".contains(chars[i]) {
                op.push(chars[i]);
                i += 1;
            }
            spans.push(Span::styled(op, Style::default().fg(config.operator_color)));
            continue;
        }
        
        // Default: just add the character
        spans.push(Span::styled(c.to_string(), Style::default().fg(theme.colors.foreground)));
        i += 1;
    }
    
    if spans.is_empty() {
        spans.push(Span::styled(String::new(), Style::default()));
    }
    
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_code_block_creation() {
        let block = CodeBlock::new("rust", "fn main() {\n    println!(\"Hello\");\n}");
        assert_eq!(block.language, "rust");
        assert_eq!(block.lines.len(), 3);
    }

    #[test]
    fn test_language_labels() {
        assert_eq!(get_language_label("rust"), "Rust");
        assert_eq!(get_language_label("py"), "Python");
        assert_eq!(get_language_label("js"), "JavaScript");
        assert_eq!(get_language_label(""), "Code");
    }
}
