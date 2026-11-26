//! Rendering tokens as markdown in one pass.
//!
//! Skipped regions and the focus element are tracked here; what each
//! element renders as lives in [`blocks`] and [`inline`].

mod blocks;
mod inline;

use super::text::{collapse_whitespace, decode_entities, tidy};
use super::tokenize::{attr, Token};
use inline::Inline;
use reqwest::Url;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Focus {
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

pub(super) fn focus_element(tokens: &[Token]) -> Option<Focus> {
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
    fn new(tag: &str) -> Self {
        Self {
            tag: tag.to_string(),
            depth: 1,
        }
    }

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

pub(super) fn render(tokens: &[Token], base: Option<&Url>, focus: Option<Focus>) -> String {
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
        if !self.enter(name, attrs) {
            return;
        }
        if self.pre.is_some() {
            self.start_in_pre(name, attrs);
            return;
        }
        self.start_element(name, attrs);
    }

    /// Track skipped and focus regions for an opening tag; returns whether
    /// the tag itself is rendered.
    fn enter(&mut self, name: &str, attrs: &[(String, String)]) -> bool {
        let void = VOID.contains(&name);
        if let Some(skip) = &mut self.skip {
            if !void {
                skip.track(name, true);
            }
            return false;
        }
        if let Some(region) = &mut self.focus_region {
            if !void {
                region.track(name, true);
            }
        } else if let Some(focus) = self.focus {
            if focus.matches(name, attrs) {
                self.focus_region = Some(Region::new(name));
                self.block_break();
            }
            return false;
        }
        let is_page_header = name == "header" && self.focus.is_none();
        if (SKIPPED.contains(&name) || is_page_header || is_hidden(attrs)) && !void {
            self.skip = Some(Region::new(name));
            return false;
        }
        true
    }

    fn start_in_pre(&mut self, name: &str, attrs: &[(String, String)]) {
        match name {
            "br" => self.out.push('\n'),
            "code" => {
                if let Some((_, lang @ None)) = &mut self.pre {
                    *lang = code_language(attrs);
                }
            }
            _ => {}
        }
    }

    fn start_element(&mut self, name: &str, attrs: &[(String, String)]) {
        match name {
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => self.open_heading(name),
            "p" => self.open_paragraph(),
            "div" | "section" | "article" | "main" | "header" | "figure" | "figcaption"
            | "details" | "summary" | "address" | "dl" | "dt" => self.line_break(),
            "dd" => {
                self.line_break();
                self.out.push_str("  ");
            }
            "br" => self.open_line_break(),
            "hr" => self.horizontal_rule(),
            "ul" | "ol" => self.open_list(name, attrs),
            "li" => self.open_list_item(),
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
            "tr" => self.open_row(),
            "td" | "th" => self.open_cell(),
            _ => self.start_inline(name, attrs),
        }
    }

    fn end(&mut self, name: &str) {
        if !self.leave(name) {
            return;
        }
        if self.pre.is_some() {
            if name == "pre" {
                self.close_pre();
            }
            return;
        }
        self.end_element(name);
    }

    /// Track skipped and focus regions for a closing tag; returns whether
    /// the tag itself is rendered.
    fn leave(&mut self, name: &str) -> bool {
        if let Some(skip) = &mut self.skip {
            if !skip.track(name, false) {
                self.skip = None;
            }
            return false;
        }
        if let Some(region) = &mut self.focus_region {
            if !region.track(name, false) {
                self.focus_region = None;
                self.block_break();
                return false;
            }
        } else if self.focus.is_some() {
            return false;
        }
        true
    }

    fn end_element(&mut self, name: &str) {
        match name {
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "p" | "table" => self.close_block(name),
            "div" | "section" | "article" | "main" | "header" | "figure" | "figcaption"
            | "details" | "summary" | "address" | "dl" | "dt" | "dd" | "li" => self.line_break(),
            "ul" | "ol" => self.close_list(),
            "blockquote" => self.close_blockquote(),
            "td" | "th" => self.close_cell(),
            "tr" => self.close_row(),
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
