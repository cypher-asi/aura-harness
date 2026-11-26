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
use std::path::Path;
use std::sync::OnceLock;
use syntect::highlighting::{
    Color as SyntectColor, Highlighter, ScopeSelectors, StyleModifier, Theme as SyntectTheme,
    ThemeItem, ThemeSettings,
};
use syntect::parsing::{ParseState, ScopeStack, SyntaxReference, SyntaxSet};

mod cache;

pub use cache::HighlightCache;

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
pub fn syntax_for_token(token: &str) -> Option<&'static SyntaxReference> {
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
pub fn syntax_for_path(path: &Path, first_line: &str) -> &'static SyntaxReference {
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
pub struct LineHighlighter {
    /// `None` for plain text, which skips parsing.
    state: Option<LineState>,
}

impl LineHighlighter {
    /// A highlighter at the start of a `syntax` source.
    pub fn new(syntax: &SyntaxReference) -> Self {
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
    pub fn spans(&mut self, line: &str, theme: &Theme) -> Vec<Span<'static>> {
        token_spans(&self.tokens(line), theme)
    }
}

/// Highlight one line of `syntax` on its own, without earlier context.
pub fn highlight_line(syntax: &SyntaxReference, line: &str, theme: &Theme) -> Vec<Span<'static>> {
    LineHighlighter::new(syntax).spans(line, theme)
}

#[cfg(test)]
mod tests;
