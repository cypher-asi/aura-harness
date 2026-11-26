//! HTML to readable markdown, without a DOM.
//!
//! Pages are tokenized into tags and text and rendered in one pass. Real
//! pages are rarely well-formed, so nothing here relies on nesting being
//! correct: skipped regions (scripts, navigation, page chrome) end at the
//! matching close tag counted per name, and an unclosed inline tag only
//! costs a stray marker. When the page has a `<main>` (or, failing that,
//! `<article>`) element only its content is rendered, which drops most
//! sidebars and footers; if that leaves almost nothing the whole body is
//! rendered instead.

use reqwest::Url;

/// A converted page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    /// Contents of `<title>`, if any.
    pub title: Option<String>,
    /// The page body as markdown.
    pub markdown: String,
}

/// Elements whose content is never shown.
const SKIPPED: &[&str] = &[
    "head", "nav", "footer", "aside", "noscript", "template", "svg", "iframe", "form", "button",
    "select", "dialog", "canvas", "object",
];

/// Elements with no closing tag.
const VOID: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "source", "track",
    "wbr",
];

/// Elements whose text is not HTML (the tokenizer swallows them whole).
const RAW_TEXT: &[&str] = &["script", "style", "title", "textarea"];

/// Below this much text, a `<main>`/`<article>` focus is assumed to have
/// missed the content and the whole page is rendered.
const MIN_FOCUSED_CHARS: usize = 200;

/// Convert an HTML document to markdown. `base` resolves relative links.
#[must_use]
pub fn html_to_markdown(html: &str, base: Option<&Url>) -> Page {
    let tokens = tokenize(html);
    let title = tokens.iter().find_map(|t| match t {
        Token::Raw { tag, text } if tag == "title" => {
            let title = collapse_whitespace(&decode_entities(text));
            let title = title.trim();
            (!title.is_empty()).then(|| title.to_string())
        }
        _ => None,
    });

    let focus = focus_element(&tokens);
    let mut markdown = render(&tokens, base, focus);
    if focus.is_some() && markdown.chars().count() < MIN_FOCUSED_CHARS {
        let full = render(&tokens, base, None);
        if full.len() > markdown.len() {
            markdown = full;
        }
    }
    Page { title, markdown }
}

/// Plain text of an HTML fragment (tags dropped, entities decoded,
/// whitespace collapsed). Used for search-result snippets.
#[must_use]
pub fn html_to_text(fragment: &str) -> String {
    let mut text = String::new();
    for token in tokenize(fragment) {
        if let Token::Text(t) = token {
            text.push_str(&t);
        }
    }
    collapse_whitespace(&decode_entities(&text))
        .trim()
        .to_string()
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    Start {
        name: String,
        attrs: Vec<(String, String)>,
    },
    End(String),
    Text(String),
    /// Content of a raw-text element such as `<script>`.
    Raw {
        tag: String,
        text: String,
    },
}

fn tokenize(html: &str) -> Vec<Token> {
    let bytes = html.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0;
    let mut text_start = 0;

    while i < bytes.len() {
        if bytes[i] != b'<' {
            i += 1;
            continue;
        }
        let Some(&next) = bytes.get(i + 1) else {
            break;
        };
        let markup_end = if html[i..].starts_with("<!--") {
            Some(
                html[i + 4..]
                    .find("-->")
                    .map_or(bytes.len(), |p| i + 4 + p + 3),
            )
        } else if next == b'!' || next == b'?' {
            Some(html[i..].find('>').map_or(bytes.len(), |p| i + p + 1))
        } else {
            None
        };
        if let Some(end) = markup_end {
            push_text(&mut tokens, &html[text_start..i]);
            i = end;
            text_start = i;
            continue;
        }

        let is_end = next == b'/';
        let name_start = if is_end { i + 2 } else { i + 1 };
        if !bytes.get(name_start).is_some_and(u8::is_ascii_alphabetic) {
            // A bare `<` in text, e.g. `a < b`.
            i += 1;
            continue;
        }
        let Some(tag_len) = find_tag_end(&html[i..]) else {
            // A tag cut off at the end of the document.
            push_text(&mut tokens, &html[text_start..i]);
            text_start = bytes.len();
            break;
        };
        let tag_end = i + tag_len;
        push_text(&mut tokens, &html[text_start..i]);
        let inner = &html[name_start..tag_end - 1];
        let name_len = inner
            .find(|c: char| c.is_ascii_whitespace() || c == '/' || c == '>')
            .unwrap_or(inner.len());
        let name = inner[..name_len].to_ascii_lowercase();
        i = tag_end;
        text_start = i;

        if is_end {
            tokens.push(Token::End(name));
            continue;
        }
        let self_closing = inner.trim_end().ends_with('/');
        let attrs = parse_attrs(&inner[name_len..]);
        if RAW_TEXT.contains(&name.as_str()) && !self_closing {
            let close = format!("</{name}");
            let end = find_ci(&html[i..], &close).map_or(bytes.len(), |p| i + p);
            tokens.push(Token::Raw {
                tag: name.clone(),
                text: html[i..end].to_string(),
            });
            i = html[end..].find('>').map_or(bytes.len(), |p| end + p + 1);
            text_start = i;
            continue;
        }
        tokens.push(Token::Start { name, attrs });
    }
    push_text(&mut tokens, &html[text_start.min(bytes.len())..]);
    tokens
}

fn push_text(tokens: &mut Vec<Token>, text: &str) {
    if !text.is_empty() {
        tokens.push(Token::Text(text.to_string()));
    }
}

/// Length of the tag starting at `s[0] == '<'`, through its `>`, skipping
/// `>` inside quoted attribute values.
fn find_tag_end(s: &str) -> Option<usize> {
    let mut quote: Option<char> = None;
    for (idx, c) in s.char_indices().skip(1) {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(c),
            (None, '>') => return Some(idx + 1),
            (None, _) => {}
        }
    }
    None
}

fn find_ci(haystack: &str, needle: &str) -> Option<usize> {
    let needle = needle.as_bytes();
    haystack
        .as_bytes()
        .windows(needle.len())
        .position(|w| w.eq_ignore_ascii_case(needle))
}

fn parse_attrs(s: &str) -> Vec<(String, String)> {
    let mut attrs = Vec::new();
    let mut rest = s.trim_start_matches(|c: char| c.is_ascii_whitespace() || c == '/');
    while !rest.is_empty() {
        let name_len = rest
            .find(|c: char| c.is_ascii_whitespace() || c == '=' || c == '/')
            .unwrap_or(rest.len());
        let name = rest[..name_len].to_ascii_lowercase();
        rest = rest[name_len..].trim_start();
        let mut value = String::new();
        if let Some(after_eq) = rest.strip_prefix('=') {
            let after_eq = after_eq.trim_start();
            let (raw, remaining) = match after_eq.chars().next() {
                Some(q @ ('"' | '\'')) => {
                    let body = &after_eq[1..];
                    let end = body.find(q).unwrap_or(body.len());
                    (&body[..end], body.get(end + 1..).unwrap_or(""))
                }
                _ => {
                    let end = after_eq
                        .find(|c: char| c.is_ascii_whitespace())
                        .unwrap_or(after_eq.len());
                    (&after_eq[..end], &after_eq[end..])
                }
            };
            value = decode_entities(raw);
            rest = remaining;
        }
        if !name.is_empty() {
            attrs.push((name, value));
        }
        rest = rest.trim_start_matches(|c: char| c.is_ascii_whitespace() || c == '/');
    }
    attrs
}

fn attr<'a>(attrs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    attrs
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, v)| v.as_str())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Main,
    Article,
}

impl Focus {
    fn matches(self, name: &str, attrs: &[(String, String)]) -> bool {
        match self {
            Self::Main => name == "main" || attr(attrs, "role") == Some("main"),
            Self::Article => name == "article",
        }
    }
}

fn focus_element(tokens: &[Token]) -> Option<Focus> {
    let starts = || {
        tokens.iter().filter_map(|t| match t {
            Token::Start { name, attrs } => Some((name.as_str(), attrs.as_slice())),
            _ => None,
        })
    };
    if starts().any(|(n, a)| Focus::Main.matches(n, a)) {
        Some(Focus::Main)
    } else if starts().any(|(n, a)| Focus::Article.matches(n, a)) {
        Some(Focus::Article)
    } else {
        None
    }
}

/// An open inline element whose rendered text gets rewritten on close.
struct Inline {
    tag: String,
    start: usize,
    kind: InlineKind,
}

enum InlineKind {
    Link(Option<String>),
    Wrap(&'static str),
    Code,
}

struct ListLevel {
    ordered: bool,
    next: usize,
}

#[derive(Default)]
struct Table {
    row_cells: usize,
    rows: usize,
}

/// Counter for a region that ends at the matching close tag of one name.
struct Region {
    tag: String,
    depth: usize,
}

impl Region {
    /// Track `name` opening (`true`) or closing; returns whether the
    /// region is still open.
    fn track(&mut self, name: &str, open: bool) -> bool {
        if name == self.tag {
            if open {
                self.depth += 1;
            } else {
                self.depth -= 1;
            }
        }
        self.depth > 0
    }
}

struct Renderer<'a> {
    out: String,
    base: Option<&'a Url>,
    skip: Option<Region>,
    /// `None` renders everything; otherwise only inside a focus element.
    focus: Option<Focus>,
    focus_region: Option<Region>,
    inline: Vec<Inline>,
    lists: Vec<ListLevel>,
    pre: Option<(usize, Option<String>)>,
    quotes: Vec<usize>,
    table: Option<Table>,
    in_cell: bool,
}

fn render(tokens: &[Token], base: Option<&Url>, focus: Option<Focus>) -> String {
    let mut r = Renderer {
        out: String::new(),
        base,
        skip: None,
        focus,
        focus_region: None,
        inline: Vec::new(),
        lists: Vec::new(),
        pre: None,
        quotes: Vec::new(),
        table: None,
        in_cell: false,
    };
    for token in tokens {
        match token {
            Token::Start { name, attrs } => r.start(name, attrs),
            Token::End(name) => r.end(name),
            Token::Text(text) => r.text(text),
            Token::Raw { .. } => {}
        }
    }
    tidy(&r.out)
}

impl Renderer<'_> {
    fn visible(&self) -> bool {
        self.skip.is_none() && (self.focus.is_none() || self.focus_region.is_some())
    }

    fn start(&mut self, name: &str, attrs: &[(String, String)]) {
        let void = VOID.contains(&name);
        if let Some(skip) = &mut self.skip {
            if !void {
                skip.track(name, true);
            }
            return;
        }
        if let Some(region) = &mut self.focus_region {
            if !void {
                region.track(name, true);
            }
        } else if let Some(focus) = self.focus {
            if focus.matches(name, attrs) {
                self.focus_region = Some(Region {
                    tag: name.to_string(),
                    depth: 1,
                });
                self.block_break();
            }
            return;
        }
        let is_page_header = name == "header" && self.focus.is_none();
        if (SKIPPED.contains(&name) || is_page_header || is_hidden(attrs)) && !void {
            self.skip = Some(Region {
                tag: name.to_string(),
                depth: 1,
            });
            return;
        }

        if self.pre.is_some() {
            match name {
                "br" => self.out.push('\n'),
                "code" => {
                    if let Some((_, lang @ None)) = &mut self.pre {
                        *lang = code_language(attrs);
                    }
                }
                _ => {}
            }
            return;
        }

        match name {
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                self.block_break();
                let level = usize::from(name.as_bytes()[1] - b'0');
                self.out.push_str(&"#".repeat(level));
                self.out.push(' ');
            }
            "p" => {
                if self.lists.is_empty() {
                    self.block_break();
                } else {
                    self.line_break();
                }
            }
            "div" | "section" | "article" | "main" | "header" | "figure" | "figcaption"
            | "details" | "summary" | "address" | "dl" | "dt" => self.line_break(),
            "dd" => {
                self.line_break();
                self.out.push_str("  ");
            }
            "br" => {
                // A line break inside a link or emphasis would close it early.
                if self.inline.is_empty() {
                    self.line_break();
                } else {
                    self.out.push(' ');
                }
            }
            "hr" => {
                self.block_break();
                self.out.push_str("---");
                self.block_break();
            }
            "ul" | "ol" => {
                self.line_break();
                if self.lists.is_empty() {
                    self.block_break();
                }
                let start = attr(attrs, "start")
                    .and_then(|s| s.trim().parse().ok())
                    .unwrap_or(1);
                self.lists.push(ListLevel {
                    ordered: name == "ol",
                    next: start,
                });
            }
            "li" => {
                self.line_break();
                let depth = self.lists.len().saturating_sub(1);
                self.out.push_str(&"  ".repeat(depth));
                match self.lists.last_mut() {
                    Some(level) if level.ordered => {
                        self.out.push_str(&format!("{}. ", level.next));
                        level.next += 1;
                    }
                    _ => self.out.push_str("- "),
                }
            }
            "pre" => {
                self.block_break();
                self.pre = Some((self.out.len(), code_language(attrs)));
            }
            "blockquote" => {
                self.block_break();
                self.quotes.push(self.out.len());
            }
            "table" => {
                self.block_break();
                self.table = Some(Table::default());
            }
            "tr" => {
                if let Some(table) = &mut self.table {
                    table.row_cells = 0;
                    self.out.push('\n');
                    self.out.push('|');
                }
            }
            "td" | "th" => {
                if let Some(table) = &mut self.table {
                    table.row_cells += 1;
                    self.out.push(' ');
                    self.in_cell = true;
                }
            }
            "a" => {
                let href = attr(attrs, "href").and_then(|h| self.resolve(h));
                self.open_inline(name, InlineKind::Link(href));
            }
            "strong" | "b" => self.open_inline(name, InlineKind::Wrap("**")),
            "em" | "i" => self.open_inline(name, InlineKind::Wrap("*")),
            "del" | "s" => self.open_inline(name, InlineKind::Wrap("~~")),
            "code" | "kbd" | "samp" => self.open_inline(name, InlineKind::Code),
            "img" => {
                let alt = attr(attrs, "alt").map(collapse_whitespace);
                let src = attr(attrs, "src").and_then(|s| self.resolve(s));
                if let (Some(alt), Some(src)) = (alt, src) {
                    let alt = alt.trim();
                    if !alt.is_empty() {
                        self.out.push_str(&format!("![{alt}]({src})"));
                    }
                }
            }
            _ => {}
        }
    }

    fn end(&mut self, name: &str) {
        if let Some(skip) = &mut self.skip {
            if !skip.track(name, false) {
                self.skip = None;
            }
            return;
        }
        if let Some(region) = &mut self.focus_region {
            if !region.track(name, false) {
                self.focus_region = None;
                self.block_break();
                return;
            }
        } else if self.focus.is_some() {
            return;
        }

        if let Some((start, lang)) = &self.pre {
            if name != "pre" {
                return;
            }
            let start = *start;
            let lang = lang.clone().unwrap_or_default();
            let body = self.out.split_off(start);
            let body = body.strip_prefix('\n').unwrap_or(&body).trim_end();
            let fence = if body.contains("```") { "````" } else { "```" };
            self.out
                .push_str(&format!("{fence}{lang}\n{body}\n{fence}"));
            self.pre = None;
            self.block_break();
            return;
        }

        match name {
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "p" | "table" => {
                if name == "table" {
                    self.table = None;
                }
                if name == "p" && !self.lists.is_empty() {
                    self.line_break();
                } else {
                    self.block_break();
                }
            }
            "div" | "section" | "article" | "main" | "header" | "figure" | "figcaption"
            | "details" | "summary" | "address" | "dl" | "dt" | "dd" | "li" => self.line_break(),
            "ul" | "ol" => {
                self.lists.pop();
                self.line_break();
                if self.lists.is_empty() {
                    self.block_break();
                }
            }
            "blockquote" => {
                if let Some(start) = self.quotes.pop() {
                    let body = self.out.split_off(start.min(self.out.len()));
                    let quoted: Vec<String> = tidy(&body)
                        .lines()
                        .map(|line| {
                            if line.is_empty() {
                                ">".to_string()
                            } else {
                                format!("> {line}")
                            }
                        })
                        .collect();
                    self.out.push_str(&quoted.join("\n"));
                    self.block_break();
                }
            }
            "td" | "th" => {
                self.in_cell = false;
                if self.table.is_some() {
                    while self.out.ends_with(' ') {
                        self.out.pop();
                    }
                    self.out.push_str(" |");
                }
            }
            "tr" => {
                if let Some(table) = &mut self.table {
                    table.rows += 1;
                    if table.rows == 1 && table.row_cells > 0 {
                        self.out.push('\n');
                        self.out.push('|');
                        self.out.push_str(&" --- |".repeat(table.row_cells));
                    }
                }
            }
            _ => self.close_inline(name),
        }
    }

    fn text(&mut self, raw: &str) {
        if !self.visible() {
            return;
        }
        let decoded = decode_entities(raw);
        if self.pre.is_some() {
            self.out.push_str(&decoded.replace('\u{a0}', " "));
            return;
        }
        let collapsed = collapse_whitespace(&decoded);
        let at_line_start = self.out.is_empty()
            || self.out.ends_with('\n')
            || self.out.ends_with(' ')
            || self.out.ends_with('|');
        let text = if at_line_start {
            collapsed.trim_start()
        } else {
            collapsed.as_str()
        };
        self.out.push_str(text);
    }

    fn open_inline(&mut self, tag: &str, kind: InlineKind) {
        self.inline.push(Inline {
            tag: tag.to_string(),
            start: self.out.len(),
            kind,
        });
    }

    fn close_inline(&mut self, tag: &str) {
        let Some(pos) = self.inline.iter().rposition(|i| i.tag == tag) else {
            return;
        };
        // Close anything left open inside it too (unclosed tags are common).
        let mut closing = self.inline.split_off(pos);
        while let Some(inline) = closing.pop() {
            self.rewrite_inline(inline);
        }
    }

    fn rewrite_inline(&mut self, inline: Inline) {
        let start = inline.start.min(self.out.len());
        let body = self.out.split_off(start);
        let trimmed = body.trim();
        if trimmed.is_empty() {
            self.out.push_str(&body);
            return;
        }
        let lead = &body[..body.len() - body.trim_start().len()];
        let trail = &body[body.trim_end().len()..];
        let rendered = match inline.kind {
            InlineKind::Link(Some(href)) => format!("[{trimmed}]({href})"),
            InlineKind::Link(None) => trimmed.to_string(),
            InlineKind::Wrap(marker) => format!("{marker}{trimmed}{marker}"),
            InlineKind::Code => {
                if trimmed.contains('`') {
                    format!("`` {trimmed} ``")
                } else {
                    format!("`{trimmed}`")
                }
            }
        };
        self.out.push_str(lead);
        self.out.push_str(&rendered);
        self.out.push_str(trail);
    }

    fn resolve(&self, href: &str) -> Option<String> {
        let href = href.trim();
        if href.is_empty() || href.starts_with('#') {
            return None;
        }
        let lower = href.to_ascii_lowercase();
        if lower.starts_with("javascript:") || lower.starts_with("data:") {
            return None;
        }
        match self.base {
            Some(base) => base.join(href).ok().map(String::from),
            None => Some(href.to_string()),
        }
    }

    fn line_break(&mut self) {
        if self.in_cell {
            if !self.out.ends_with(' ') {
                self.out.push(' ');
            }
            return;
        }
        // Emphasis and links never span blocks; an unclosed one ends here.
        let open = std::mem::take(&mut self.inline);
        for inline in open.into_iter().rev() {
            self.rewrite_inline(inline);
        }
        // `<li><p>text` keeps the text on the marker's line.
        if self.line_is_list_marker() {
            return;
        }
        while self.out.ends_with(' ') {
            self.out.pop();
        }
        if !self.out.is_empty() && !self.out.ends_with('\n') {
            self.out.push('\n');
        }
    }

    fn line_is_list_marker(&self) -> bool {
        let line = self
            .out
            .rsplit('\n')
            .next()
            .unwrap_or_default()
            .trim_start();
        line == "- "
            || line
                .strip_suffix(". ")
                .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
    }

    fn block_break(&mut self) {
        if self.in_cell {
            self.line_break();
            return;
        }
        self.line_break();
        if !self.out.is_empty() && !self.out.ends_with("\n\n") {
            self.out.push('\n');
        }
    }
}

fn is_hidden(attrs: &[(String, String)]) -> bool {
    attr(attrs, "hidden").is_some()
        || attr(attrs, "aria-hidden") == Some("true")
        || attr(attrs, "style").is_some_and(|style| {
            let style: String = style.chars().filter(|c| !c.is_whitespace()).collect();
            style.to_ascii_lowercase().contains("display:none")
        })
}

fn code_language(attrs: &[(String, String)]) -> Option<String> {
    attr(attrs, "class")?.split_whitespace().find_map(|class| {
        class
            .strip_prefix("language-")
            .or_else(|| class.strip_prefix("lang-"))
            .filter(|lang| !lang.is_empty())
            .map(str::to_string)
    })
}

/// Trim trailing spaces, drop whitespace-only lines and collapse runs of
/// blank lines, leaving fenced code untouched.
fn tidy(markdown: &str) -> String {
    let mut out = String::with_capacity(markdown.len());
    let mut blank_run = 0;
    let mut in_fence = false;
    for line in markdown.lines() {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
        }
        let line = if in_fence { line } else { line.trim_end() };
        if line.trim().is_empty() && !in_fence {
            blank_run += 1;
            if blank_run > 1 || out.is_empty() {
                continue;
            }
            out.push('\n');
            continue;
        }
        blank_run = 0;
        out.push_str(line);
        out.push('\n');
    }
    out.trim_end().to_string()
}

fn collapse_whitespace(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_space = false;
    for c in s.chars() {
        if c.is_whitespace() {
            if !in_space {
                out.push(' ');
            }
            in_space = true;
        } else {
            out.push(c);
            in_space = false;
        }
    }
    out
}

/// Decode character references. Unknown named entities are left as-is.
pub(crate) fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        let decoded = rest
            .get(1..rest.len().min(12))
            .and_then(|window| window.find(';'))
            .and_then(|semi| {
                let entity = &rest[1..=semi];
                decode_entity(entity).map(|c| (c, semi + 2))
            });
        match decoded {
            Some((c, len)) => {
                out.push(c);
                rest = &rest[len..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn decode_entity(entity: &str) -> Option<char> {
    if let Some(num) = entity.strip_prefix('#') {
        let code = match num.strip_prefix(['x', 'X']) {
            Some(hex) => u32::from_str_radix(hex, 16).ok()?,
            None => num.parse().ok()?,
        };
        return char::from_u32(code).filter(|c| *c != '\0');
    }
    let c = match entity {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => '\u{a0}',
        "mdash" => '—',
        "ndash" => '–',
        "hellip" => '…',
        "lsquo" => '‘',
        "rsquo" => '’',
        "ldquo" => '“',
        "rdquo" => '”',
        "laquo" => '«',
        "raquo" => '»',
        "bull" => '•',
        "middot" => '·',
        "copy" => '©',
        "reg" => '®',
        "trade" => '™',
        "times" => '×',
        "deg" => '°',
        "rarr" => '→',
        "larr" => '←',
        _ => return None,
    };
    Some(c)
}

#[cfg(test)]
mod tests;
