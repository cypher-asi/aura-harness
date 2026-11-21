//! Syntax highlighting for the file viewer and fenced code blocks.
//!
//! Parsing is done by syntect with bat's extended syntax set (two-face),
//! using the pure-Rust `fancy-regex` engine. Colors do not come from a
//! syntect theme: every scope is classified into a [`Role`] and each role
//! maps to the approved palette from the style guide, or to the passed
//! [`Theme`]'s colors:
//!
//! | Role        | Scopes (most specific rule wins)                         | Color            |
//! |-------------|----------------------------------------------------------|------------------|
//! | `Keyword`   | `keyword`, `storage`, `variable.language`                | Purple, bold     |
//! | `Tag`       | `entity.name.tag`                                        | Purple           |
//! | `Operator`  | `keyword.operator`, list bullets                         | Red              |
//! | `String`    | `string`, `markup.raw`                                   | Cyan             |
//! | `Type`      | `entity.name.type`/`class`/..., `support.type`/`class`   | Cyan             |
//! | `Comment`   | `comment`                                                | Gray             |
//! | `Constant`  | `constant`                                               | Blue             |
//! | `Function`  | `entity.name.function`, `support.function`, calls        | Blue             |
//! | `Property`  | attribute names, CSS properties, YAML/TOML keys          | Blue             |
//! | `Key`       | JSON object keys                                         | foreground       |
//! | `Heading`   | `markup.heading`                                         | primary, bold    |
//! | `Bold`      | `markup.bold`                                            | foreground, bold |
//! | `Italic`    | `markup.italic`                                          | foreground, ital |
//! | `Link`      | `markup.underline.link`, `string.other.link`             | Blue             |
//! | `Inserted`  | `markup.inserted`                                        | success          |
//! | `Deleted`   | `markup.deleted`                                         | error            |
//! | `Plain`     | everything else                                          | foreground       |
//!
//! The scope rules are fed to syntect's [`Highlighter`] as a theme whose
//! colors are sentinels encoding the role, so scope matching and
//! specificity follow syntect's (TextMate) rules exactly.
//!
//! Invariant: the text of the spans produced for a line always
//! concatenates to exactly that line, so callers can restyle them by
//! character position (cursor, search matches).

use crate::themes::{Theme, BLUE, CYAN, GRAY, PURPLE, RED};
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use std::ops::Range;
use std::path::Path;
use std::sync::OnceLock;
use syntect::highlighting::{
    Color as SyntectColor, Highlighter, ScopeSelectors, StyleModifier, Theme as SyntectTheme,
    ThemeItem, ThemeSettings,
};
use syntect::parsing::{ParseState, ScopeStack, SyntaxReference, SyntaxSet};

/// Lines longer than this (in bytes) are shown in plain foreground instead
/// of being parsed, so one minified line cannot stall the UI.
const MAX_HIGHLIGHT_LINE_BYTES: usize = 20_000;

/// Lines between saved parser states in a [`HighlightCache`].
pub(crate) const CHECKPOINT_INTERVAL: usize = 128;

/// Extra lines highlighted around a request so small scrolls hit the cache.
const WINDOW_MARGIN: usize = 64;

/// What a piece of source text is, deciding its style.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Role {
    Plain,
    Keyword,
    Tag,
    Operator,
    String,
    Type,
    Comment,
    Constant,
    Function,
    Property,
    Key,
    Heading,
    Bold,
    Italic,
    Link,
    Inserted,
    Deleted,
}

/// Every role, indexed by its discriminant (used to decode sentinels).
const ROLES: [Role; 17] = [
    Role::Plain,
    Role::Keyword,
    Role::Tag,
    Role::Operator,
    Role::String,
    Role::Type,
    Role::Comment,
    Role::Constant,
    Role::Function,
    Role::Property,
    Role::Key,
    Role::Heading,
    Role::Bold,
    Role::Italic,
    Role::Link,
    Role::Inserted,
    Role::Deleted,
];

/// Scope selectors and the role they give. Among matching rules, syntect
/// picks the most specific, so `keyword.operator` beats `keyword`.
const RULES: &[(&str, Role)] = &[
    ("comment", Role::Comment),
    ("string", Role::String),
    ("markup.raw", Role::String),
    ("constant", Role::Constant),
    ("support.constant", Role::Type),
    ("keyword", Role::Keyword),
    ("storage", Role::Keyword),
    ("variable.language", Role::Keyword),
    ("keyword.operator", Role::Operator),
    ("punctuation.definition.list_item", Role::Operator),
    ("punctuation.definition.list", Role::Operator),
    ("entity.name.tag", Role::Tag),
    ("entity.other.attribute-name", Role::Property),
    ("support.type.property-name", Role::Property),
    ("meta.property-name", Role::Property),
    ("meta.mapping.key string", Role::Property),
    ("entity.name.tag.yaml", Role::Property),
    ("entity.name.tag.toml", Role::Property),
    ("entity.name.table", Role::Type),
    ("meta.mapping.key.json string", Role::Key),
    ("support.type.property-name.json", Role::Key),
    ("entity.name.function", Role::Function),
    ("support.function", Role::Function),
    ("variable.function", Role::Function),
    ("meta.function-call.identifier", Role::Function),
    ("entity.name.type", Role::Type),
    ("entity.name.class", Role::Type),
    ("entity.name.struct", Role::Type),
    ("entity.name.enum", Role::Type),
    ("entity.name.trait", Role::Type),
    ("entity.name.impl", Role::Type),
    ("entity.name.union", Role::Type),
    ("entity.name.interface", Role::Type),
    ("entity.other.inherited-class", Role::Type),
    ("support.type", Role::Type),
    ("support.class", Role::Type),
    ("markup.heading", Role::Heading),
    ("markup.bold", Role::Bold),
    ("markup.italic", Role::Italic),
    ("markup.underline.link", Role::Link),
    ("string.other.link", Role::Link),
    ("meta.link.inline.description", Role::Link),
    ("markup.inserted", Role::Inserted),
    ("markup.deleted", Role::Deleted),
];

/// Green and blue channels marking a syntect color as a role sentinel;
/// the red channel holds the role's index in [`ROLES`].
const SENTINEL_GB: (u8, u8) = (0x5e, 0x17);

impl Role {
    /// The style of this role under `theme`.
    pub(crate) fn style(self, theme: &Theme) -> Style {
        let fg = |color| Style::default().fg(color);
        match self {
            Self::Plain | Self::Key => fg(theme.colors.foreground),
            Self::Keyword => fg(PURPLE).add_modifier(Modifier::BOLD),
            Self::Tag => fg(PURPLE),
            Self::Operator => fg(RED),
            Self::String | Self::Type => fg(CYAN),
            Self::Comment => fg(GRAY),
            Self::Constant | Self::Function | Self::Property | Self::Link => fg(BLUE),
            Self::Heading => fg(theme.colors.primary).add_modifier(Modifier::BOLD),
            Self::Bold => fg(theme.colors.foreground).add_modifier(Modifier::BOLD),
            Self::Italic => fg(theme.colors.foreground).add_modifier(Modifier::ITALIC),
            Self::Inserted => fg(theme.colors.success),
            Self::Deleted => fg(theme.colors.error),
        }
    }

    fn sentinel(self) -> SyntectColor {
        SyntectColor {
            r: self as u8,
            g: SENTINEL_GB.0,
            b: SENTINEL_GB.1,
            a: 0xff,
        }
    }

    fn from_sentinel(color: SyntectColor) -> Self {
        if (color.g, color.b) != SENTINEL_GB {
            return Self::Plain;
        }
        ROLES
            .get(usize::from(color.r))
            .copied()
            .unwrap_or(Self::Plain)
    }
}

/// A piece of a highlighted line.
pub(crate) type Token = (Role, String);

/// The shared syntax definitions, loaded on first use.
fn syntax_set() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(two_face::syntax::extra_newlines)
}

/// Classifies scope stacks into roles via the sentinel theme.
fn highlighter() -> &'static Highlighter<'static> {
    static THEME: OnceLock<SyntectTheme> = OnceLock::new();
    static HIGHLIGHTER: OnceLock<Highlighter<'static>> = OnceLock::new();
    HIGHLIGHTER.get_or_init(|| Highlighter::new(THEME.get_or_init(role_theme)))
}

fn role_theme() -> SyntectTheme {
    let scopes = RULES
        .iter()
        .filter_map(|(selector, role)| {
            let scope = selector.parse::<ScopeSelectors>().ok()?;
            Some(ThemeItem {
                scope,
                style: StyleModifier {
                    foreground: Some(role.sentinel()),
                    background: None,
                    font_style: None,
                },
            })
        })
        .collect();
    SyntectTheme {
        settings: ThemeSettings {
            foreground: Some(Role::Plain.sentinel()),
            ..ThemeSettings::default()
        },
        scopes,
        ..SyntectTheme::default()
    }
}

fn role_for(stack: &ScopeStack) -> Role {
    Role::from_sentinel(highlighter().style_for_stack(stack.as_slice()).foreground)
}

/// Language names that are not a syntax's name or extension.
fn alias(token: &str) -> &str {
    match token {
        "shell" | "console" | "shellscript" => "sh",
        "golang" => "go",
        "c++" => "cpp",
        "docker" => "Dockerfile",
        "make" => "Makefile",
        "yml" => "yaml",
        "text" | "txt" | "plain" | "plaintext" => "txt",
        other => other,
    }
}

/// The syntax for a language name or file extension (`rust`, `rs`,
/// `typescript`, `tsx`, `yml`, `zsh`, ...), if one is known.
pub(crate) fn syntax_for_token(token: &str) -> Option<&'static SyntaxReference> {
    let token = token.trim();
    if token.is_empty() {
        return None;
    }
    let lower = token.to_ascii_lowercase();
    let set = syntax_set();
    set.find_syntax_by_token(alias(&lower))
        .or_else(|| set.find_syntax_by_token(token))
}

/// The syntax for a file: by its name (`Dockerfile`, `Makefile`,
/// `.bashrc`), then its extension, then the part of the name before the
/// first dot (`Dockerfile.dev`), then its first line (shebang, `<?xml`).
/// Unknown files get plain text.
pub(crate) fn syntax_for_path(path: &Path, first_line: &str) -> &'static SyntaxReference {
    let set = syntax_set();
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_default();
    let extension = path
        .extension()
        .map(|ext| ext.to_string_lossy())
        .unwrap_or_default();
    let stem = name.split('.').next().unwrap_or_default();
    let found = [name.as_ref(), extension.as_ref(), stem]
        .into_iter()
        .filter(|candidate| !candidate.is_empty())
        .find_map(|candidate| set.find_syntax_by_extension(candidate));
    found
        .or_else(|| set.find_syntax_by_first_line(first_line))
        .unwrap_or_else(|| set.find_syntax_plain_text())
}

/// Whether `syntax` is plain text, which needs no parsing.
fn is_plain(syntax: &SyntaxReference) -> bool {
    syntax.name == syntax_set().find_syntax_plain_text().name
}

/// Parser position between two lines: enough to resume highlighting.
#[derive(Debug, Clone)]
struct LineState {
    parse: ParseState,
    stack: ScopeStack,
}

impl LineState {
    fn new(syntax: &SyntaxReference) -> Self {
        Self {
            parse: ParseState::new(syntax),
            stack: ScopeStack::new(),
        }
    }

    /// Parse `line` (without its newline), returning its tokens when
    /// `emit` is set. A line that is too long or fails to parse comes
    /// back as one plain token and leaves the state as it was.
    fn advance(&mut self, line: &str, emit: bool) -> Vec<Token> {
        let plain = || vec![(Role::Plain, line.to_string())];
        if line.len() > MAX_HIGHLIGHT_LINE_BYTES {
            return if emit { plain() } else { Vec::new() };
        }
        let text = format!("{line}\n");
        let Ok(ops) = self.parse.parse_line(&text, syntax_set()) else {
            return if emit { plain() } else { Vec::new() };
        };
        let mut tokens: Vec<Token> = Vec::new();
        let mut start = 0;
        for (at, op) in &ops {
            let at = (*at).min(line.len());
            if emit && at > start {
                if let Some(piece) = line.get(start..at) {
                    push_token(&mut tokens, role_for(&self.stack), piece);
                    start = at;
                }
            }
            if self.stack.apply(op).is_err() {
                return if emit { plain() } else { Vec::new() };
            }
        }
        if emit {
            match line.get(start..) {
                Some(rest) if !rest.is_empty() => {
                    push_token(&mut tokens, role_for(&self.stack), rest);
                }
                Some(_) => {}
                None => return plain(),
            }
        }
        tokens
    }
}

/// Append `text` as `role`, merging with the previous token of that role.
fn push_token(tokens: &mut Vec<Token>, role: Role, text: &str) {
    match tokens.last_mut() {
        Some((last, existing)) if *last == role => existing.push_str(text),
        _ => tokens.push((role, text.to_string())),
    }
}

/// Spans for `tokens` under `theme`; an empty line is one empty span.
pub(crate) fn token_spans(tokens: &[Token], theme: &Theme) -> Vec<Span<'static>> {
    if tokens.is_empty() {
        return vec![Span::styled(String::new(), Role::Plain.style(theme))];
    }
    tokens
        .iter()
        .map(|(role, text)| Span::styled(text.clone(), role.style(theme)))
        .collect()
}

/// Highlights consecutive lines of one source, carrying state (block
/// comments, multi-line strings, embedded languages) from line to line.
#[derive(Debug, Clone)]
pub(crate) struct LineHighlighter {
    /// `None` for plain text, which skips parsing.
    state: Option<LineState>,
}

impl LineHighlighter {
    /// A highlighter at the start of a `syntax` source.
    pub(crate) fn new(syntax: &SyntaxReference) -> Self {
        Self {
            state: (!is_plain(syntax)).then(|| LineState::new(syntax)),
        }
    }

    /// The tokens of the next line.
    pub(crate) fn tokens(&mut self, line: &str) -> Vec<Token> {
        match &mut self.state {
            Some(state) => state.advance(line, true),
            None if line.is_empty() => Vec::new(),
            None => vec![(Role::Plain, line.to_string())],
        }
    }

    /// The spans of the next line under `theme`.
    pub(crate) fn spans(&mut self, line: &str, theme: &Theme) -> Vec<Span<'static>> {
        token_spans(&self.tokens(line), theme)
    }
}

/// Highlight one line of `syntax` on its own, without earlier context.
pub(crate) fn highlight_line(
    syntax: &SyntaxReference,
    line: &str,
    theme: &Theme,
) -> Vec<Span<'static>> {
    LineHighlighter::new(syntax).spans(line, theme)
}

/// Lazily highlighted lines of one file, resumable anywhere.
///
/// Holds the parser state before every [`CHECKPOINT_INTERVAL`]th line
/// reached so far, and the tokens of the last window shown. Highlighting
/// line `n` parses from the nearest checkpoint at or before `n` (filling
/// in checkpoints on the way, without computing styles for skipped
/// lines), so the result always equals highlighting from the top.
///
/// The cache is keyed by the syntax and line count it was built for and
/// resets itself when asked about anything else.
#[derive(Default)]
pub(crate) struct HighlightCache {
    inner: Option<CacheState>,
}

struct CacheState {
    syntax: &'static SyntaxReference,
    line_count: usize,
    /// `checkpoints[k]` is the state before line `k * CHECKPOINT_INTERVAL`.
    checkpoints: Vec<LineState>,
    /// First line of `window`.
    window_start: usize,
    window: Vec<Vec<Token>>,
}

impl HighlightCache {
    /// Spans of `lines[range]`, highlighted as `syntax` in the context of
    /// every line above them. `prepare` turns a raw line into the text to
    /// highlight (e.g. tab expansion); the spans concatenate to it.
    pub(crate) fn spans(
        &mut self,
        syntax: &'static SyntaxReference,
        lines: &[&str],
        range: Range<usize>,
        prepare: impl Fn(&str) -> String,
        theme: &Theme,
    ) -> Vec<Vec<Span<'static>>> {
        let end = range.end.min(lines.len());
        let start = range.start.min(end);
        if is_plain(syntax) {
            return lines
                .iter()
                .take(end)
                .skip(start)
                .map(|line| {
                    let text = prepare(line);
                    LineHighlighter { state: None }.spans(&text, theme)
                })
                .collect();
        }
        let state = self.state_for(syntax, lines.len());
        state.fill(lines, start..end, &prepare);
        state
            .window
            .iter()
            .skip(start.saturating_sub(state.window_start))
            .take(end - start)
            .map(|tokens| token_spans(tokens, theme))
            .collect()
    }

    fn state_for(
        &mut self,
        syntax: &'static SyntaxReference,
        line_count: usize,
    ) -> &mut CacheState {
        let stale = self
            .inner
            .as_ref()
            .is_none_or(|state| state.syntax.name != syntax.name || state.line_count != line_count);
        if stale {
            self.inner = None;
        }
        self.inner.get_or_insert_with(|| CacheState {
            syntax,
            line_count,
            checkpoints: vec![LineState::new(syntax)],
            window_start: 0,
            window: Vec::new(),
        })
    }
}

impl CacheState {
    /// Make `window` cover `range`.
    fn fill(&mut self, lines: &[&str], range: Range<usize>, prepare: &impl Fn(&str) -> String) {
        let window_end = self.window_start + self.window.len();
        if range.start >= self.window_start && range.end <= window_end {
            return;
        }
        let start = range.start.saturating_sub(WINDOW_MARGIN);
        let end = (range.end + WINDOW_MARGIN).min(lines.len());
        let mut state = self.resume_at(lines, start, prepare);
        self.window = lines
            .iter()
            .enumerate()
            .take(end)
            .skip(start)
            .map(|(index, line)| {
                if index % CHECKPOINT_INTERVAL == 0 {
                    self.save_checkpoint(index, &state);
                }
                state.advance(&prepare(line), true)
            })
            .collect();
        self.window_start = start;
    }

    /// The state before line `target`, parsing forward from the nearest
    /// checkpoint and saving new checkpoints on the way.
    fn resume_at(
        &mut self,
        lines: &[&str],
        target: usize,
        prepare: &impl Fn(&str) -> String,
    ) -> LineState {
        let wanted = target / CHECKPOINT_INTERVAL;
        let known = wanted.min(self.checkpoints.len().saturating_sub(1));
        let Some(checkpoint) = self.checkpoints.get(known) else {
            return LineState::new(self.syntax);
        };
        let mut state = checkpoint.clone();
        let skipped = lines
            .iter()
            .enumerate()
            .take(target)
            .skip(known * CHECKPOINT_INTERVAL);
        for (index, line) in skipped {
            if index % CHECKPOINT_INTERVAL == 0 {
                self.save_checkpoint(index, &state);
            }
            state.advance(&prepare(line), false);
        }
        state
    }

    /// Record `state` as the state before line `index` (a multiple of the
    /// interval) if it extends the known checkpoints.
    fn save_checkpoint(&mut self, index: usize, state: &LineState) {
        if index / CHECKPOINT_INTERVAL == self.checkpoints.len() {
            self.checkpoints.push(state.clone());
        }
    }
}

/// Clones start empty: the cache is derived data, rebuilt on demand.
impl Clone for HighlightCache {
    fn clone(&self) -> Self {
        Self::default()
    }
}

/// Caches never make two views differ.
impl PartialEq for HighlightCache {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Eq for HighlightCache {}

impl std::fmt::Debug for HighlightCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.inner {
            Some(state) => f
                .debug_struct("HighlightCache")
                .field("syntax", &state.syntax.name)
                .field("checkpoints", &state.checkpoints.len())
                .field(
                    "window",
                    &(state.window_start..state.window_start + state.window.len()),
                )
                .finish(),
            None => f.write_str("HighlightCache(empty)"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_of(spans: &[Span<'_>]) -> String {
        spans.iter().map(|s| s.content.as_ref()).collect()
    }

    fn highlight_all(syntax: &SyntaxReference, source: &str) -> Vec<Vec<Span<'static>>> {
        let theme = Theme::cyber();
        let mut highlighter = LineHighlighter::new(syntax);
        source
            .lines()
            .map(|line| highlighter.spans(line, &theme))
            .collect()
    }

    fn color_of(spans: &[Span<'_>], text: &str) -> Option<ratatui::style::Color> {
        spans
            .iter()
            .find(|s| s.content.trim() == text)
            .and_then(|s| s.style.fg)
    }

    fn file_syntax(name: &str) -> &'static SyntaxReference {
        syntax_for_path(Path::new(name), "")
    }

    #[test]
    fn every_rule_is_a_valid_selector() {
        for (selector, _) in RULES {
            assert!(selector.parse::<ScopeSelectors>().is_ok(), "{selector}");
        }
    }

    #[test]
    fn sentinels_round_trip() {
        for role in ROLES {
            assert_eq!(Role::from_sentinel(role.sentinel()), role);
        }
    }

    #[test]
    fn html_style_block_highlights_css() {
        let html =
            "<html>\n<head>\n<style>\n  body {\n    color: red;\n  }\n</style>\n</head>\n</html>\n";
        let lines = highlight_all(file_syntax("index.html"), html);
        let css = &lines[4];
        assert_eq!(text_of(css), "    color: red;");
        assert_eq!(color_of(css, "color"), Some(BLUE), "{css:?}");
        assert_eq!(color_of(css, "red"), Some(CYAN), "{css:?}");
        assert_eq!(color_of(&lines[2], "style"), Some(PURPLE), "{:?}", lines[2]);
    }

    #[test]
    fn rust_block_comment_spans_lines() {
        let source = "/* start\n   still a comment\n*/\nfn main() {}\n";
        let lines = highlight_all(file_syntax("lib.rs"), source);
        assert!(
            lines[1].iter().all(|s| s.style.fg == Some(GRAY)),
            "{:?}",
            lines[1]
        );
        assert_eq!(color_of(&lines[3], "fn"), Some(PURPLE));
        assert_eq!(color_of(&lines[3], "main"), Some(BLUE));
    }

    #[test]
    fn toml_and_yaml_keys_and_strings() {
        let lines = highlight_all(file_syntax("Cargo.toml"), "[package]\nname = \"aura\"\n");
        assert_eq!(color_of(&lines[0], "package"), Some(CYAN), "{:?}", lines[0]);
        assert_eq!(color_of(&lines[1], "name"), Some(BLUE), "{:?}", lines[1]);
        assert_eq!(
            color_of(&lines[1], "\"aura\""),
            Some(CYAN),
            "{:?}",
            lines[1]
        );
        let lines = highlight_all(file_syntax("ci.yml"), "key: value\n");
        assert_eq!(color_of(&lines[0], "key"), Some(BLUE), "{:?}", lines[0]);
        assert_eq!(color_of(&lines[0], "value"), Some(CYAN), "{:?}", lines[0]);
    }

    #[test]
    fn markdown_heading_is_bold_primary() {
        let theme = Theme::cyber();
        let lines = highlight_all(file_syntax("README.md"), "# Title\n\nbody\n");
        let heading = lines[0]
            .iter()
            .find(|s| s.content.contains("Title"))
            .unwrap();
        assert_eq!(heading.style.fg, Some(theme.colors.primary));
        assert!(heading.style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn unknown_extension_is_plain_and_exact() {
        let theme = Theme::cyber();
        let syntax = syntax_for_path(Path::new("notes.zzunknown"), "just words");
        let spans = highlight_line(syntax, "  let x = \"y\"; // z", &theme);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].content, "  let x = \"y\"; // z");
        assert_eq!(spans[0].style.fg, Some(theme.colors.foreground));
    }

    #[test]
    fn spans_always_concatenate_to_the_line() {
        let samples = [
            ("a.rs", "fn main() { let s = \"x\\ty\"; } // é"),
            ("a.py", "def f(x): return f'{x!r}'  # ok"),
            ("a.ts", "const f = (a: number): string => `${a}`;"),
            ("a.html", "<a href=\"x\">link</a> &amp;"),
            ("a.css", ".x > a:hover { margin: 0 auto; }"),
            ("a.json", "{\"k\": [1, true, null]}"),
            ("a.yaml", "key: - value # c"),
            ("a.sql", "SELECT * FROM t WHERE a = 'b';"),
            ("a.c", "#include <stdio.h> int x = 0x1f;"),
            ("a.md", "**bold** _it_ `code` [l](u)"),
            ("a.sh", "echo \"$HOME\" | grep -v x"),
            ("Dockerfile", "FROM rust:1.85 AS build"),
            ("a.go", "func main() { fmt.Println(\"hi\") }"),
            ("a.java", "public static void main(String[] a) {}"),
            ("a.diff", "+added line"),
            ("a.rs", ""),
        ];
        let theme = Theme::cyber();
        for (name, line) in samples {
            let syntax = file_syntax(name);
            assert_eq!(
                text_of(&highlight_line(syntax, line, &theme)),
                line,
                "{name}"
            );
        }
    }

    #[test]
    fn files_are_detected_by_name_extension_and_first_line() {
        assert_eq!(file_syntax("Dockerfile").name, "Dockerfile");
        assert_eq!(file_syntax("Dockerfile.render").name, "Dockerfile");
        assert_eq!(file_syntax("Makefile").name, "Makefile");
        assert_eq!(file_syntax("CMakeLists.txt").name, "CMake");
        assert_eq!(file_syntax("x.toml").name, "TOML");
        assert_eq!(file_syntax("x.tsx").name, "TypeScriptReact");
        assert_eq!(
            syntax_for_path(Path::new("script"), "#!/bin/bash").name,
            "Bourne Again Shell (bash)"
        );
    }

    #[test]
    fn tokens_resolve_names_and_extensions() {
        for (token, name) in [
            ("rust", "Rust"),
            ("rs", "Rust"),
            ("python", "Python"),
            ("typescript", "TypeScript"),
            ("tsx", "TypeScriptReact"),
            ("yml", "YAML"),
            ("sh", "Bourne Again Shell (bash)"),
            ("zsh", "Bourne Again Shell (bash)"),
            ("json", "JSON"),
        ] {
            assert_eq!(
                syntax_for_token(token).map(|s| s.name.as_str()),
                Some(name),
                "{token}"
            );
        }
        assert!(syntax_for_token("").is_none());
        assert!(syntax_for_token("definitely-not-a-language").is_none());
    }

    #[test]
    fn cache_far_down_matches_highlighting_from_scratch() {
        let source: String = (0..6000)
            .map(|n| match n % 7 {
                0 => "/* a comment\n".to_string(),
                1 => format!("   spanning {n} lines */\n"),
                2 => format!("fn f{n}(x: u32) -> u32 {{ x + {n} }}\n"),
                3 => "let s = \"multi\n".to_string(),
                4 => "line string\";\n".to_string(),
                _ => format!("// note {n}\n"),
            })
            .collect();
        let lines: Vec<&str> = source.lines().collect();
        let syntax = file_syntax("big.rs");
        let theme = Theme::cyber();
        let expected = highlight_all(syntax, &source);
        let mut cache = HighlightCache::default();
        for range in [5000..5040, 10..50, 4990..5010, 5900..5999, 0..3] {
            let got = cache.spans(syntax, &lines, range.clone(), str::to_string, &theme);
            assert_eq!(got, expected[range.clone()], "{range:?}");
        }
    }

    #[test]
    fn cache_resets_for_a_different_source() {
        let theme = Theme::cyber();
        let mut cache = HighlightCache::default();
        let rust = file_syntax("a.rs");
        cache.spans(rust, &["/* open", "x"], 0..2, str::to_string, &theme);
        let got = cache.spans(rust, &["x", "y", "z"], 1..2, str::to_string, &theme);
        assert_eq!(got, vec![highlight_line(rust, "y", &theme)]);
    }

    #[test]
    fn overlong_lines_are_plain() {
        let theme = Theme::cyber();
        let line = format!("let s = \"{}\";", "x".repeat(MAX_HIGHLIGHT_LINE_BYTES));
        let spans = highlight_line(file_syntax("a.rs"), &line, &theme);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].content, line);
    }
}
