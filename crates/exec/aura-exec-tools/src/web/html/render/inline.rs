//! Inline elements: links, emphasis, code spans and images.

use super::super::text::collapse_whitespace;
use super::super::tokenize::attr;
use super::Renderer;

/// An open inline element whose rendered text gets rewritten on close.
pub(super) struct Inline {
    tag: String,
    start: usize,
    kind: InlineKind,
}

enum InlineKind {
    Link(Option<String>),
    Wrap(&'static str),
    Code,
}

impl Renderer<'_> {
    pub(super) fn start_inline(&mut self, name: &str, attrs: &[(String, String)]) {
        match name {
            "a" => {
                let href = attr(attrs, "href").and_then(|h| self.resolve(h));
                self.open_inline(name, InlineKind::Link(href));
            }
            "strong" | "b" => self.open_inline(name, InlineKind::Wrap("**")),
            "em" | "i" => self.open_inline(name, InlineKind::Wrap("*")),
            "del" | "s" => self.open_inline(name, InlineKind::Wrap("~~")),
            "code" | "kbd" | "samp" => self.open_inline(name, InlineKind::Code),
            "img" => self.image(attrs),
            _ => {}
        }
    }

    fn image(&mut self, attrs: &[(String, String)]) {
        let alt = attr(attrs, "alt").map(collapse_whitespace);
        let src = attr(attrs, "src").and_then(|s| self.resolve(s));
        if let (Some(alt), Some(src)) = (alt, src) {
            let alt = alt.trim();
            if !alt.is_empty() {
                self.out.push_str(&format!("![{alt}]({src})"));
            }
        }
    }

    fn open_inline(&mut self, tag: &str, kind: InlineKind) {
        self.inline.push(Inline {
            tag: tag.to_string(),
            start: self.out.len(),
            kind,
        });
    }

    pub(super) fn close_inline(&mut self, tag: &str) {
        let Some(pos) = self.inline.iter().rposition(|i| i.tag == tag) else {
            return;
        };
        // Close anything left open inside it too (unclosed tags are common).
        let mut closing = self.inline.split_off(pos);
        while let Some(inline) = closing.pop() {
            self.rewrite_inline(inline);
        }
    }

    pub(super) fn rewrite_inline(&mut self, inline: Inline) {
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
}
