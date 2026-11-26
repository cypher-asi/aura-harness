//! Splitting HTML into tags and text.

use super::text::decode_entities;

/// Elements whose text is not HTML (the tokenizer swallows them whole).
const RAW_TEXT: &[&str] = &["script", "style", "title", "textarea"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Token {
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

pub(super) fn tokenize(html: &str) -> Vec<Token> {
    let mut tokenizer = Tokenizer {
        html,
        tokens: Vec::new(),
        i: 0,
        text_start: 0,
    };
    tokenizer.run();
    tokenizer.finish()
}

struct Tokenizer<'a> {
    html: &'a str,
    tokens: Vec<Token>,
    /// Scan position.
    i: usize,
    /// Start of the text not yet pushed as a token.
    text_start: usize,
}

impl Tokenizer<'_> {
    fn run(&mut self) {
        let bytes = self.html.as_bytes();
        while self.i < bytes.len() {
            if bytes[self.i] != b'<' {
                self.i += 1;
                continue;
            }
            let Some(&next) = bytes.get(self.i + 1) else {
                break;
            };
            if let Some(end) = markup_end(self.html, self.i, next) {
                self.push_text_until(self.i);
                self.i = end;
                self.text_start = end;
                continue;
            }
            if !self.tag(next) {
                break;
            }
        }
    }

    fn finish(mut self) -> Vec<Token> {
        let len = self.html.len();
        push_text(&mut self.tokens, &self.html[self.text_start.min(len)..]);
        self.tokens
    }

    fn push_text_until(&mut self, end: usize) {
        push_text(&mut self.tokens, &self.html[self.text_start..end]);
    }

    /// Handle the `<` at the scan position, followed by `next`. Returns
    /// `false` when the tag is cut off by the end of the document.
    fn tag(&mut self, next: u8) -> bool {
        let html = self.html;
        let i = self.i;
        let is_end = next == b'/';
        let name_start = if is_end { i + 2 } else { i + 1 };
        if !html
            .as_bytes()
            .get(name_start)
            .is_some_and(u8::is_ascii_alphabetic)
        {
            // A bare `<` in text, e.g. `a < b`.
            self.i += 1;
            return true;
        }
        let Some(tag_len) = find_tag_end(&html[i..]) else {
            // A tag cut off at the end of the document.
            self.push_text_until(i);
            self.text_start = html.len();
            return false;
        };
        let tag_end = i + tag_len;
        self.push_text_until(i);
        let inner = &html[name_start..tag_end - 1];
        let name_len = inner
            .find(|c: char| c.is_ascii_whitespace() || c == '/' || c == '>')
            .unwrap_or(inner.len());
        let name = inner[..name_len].to_ascii_lowercase();
        self.i = tag_end;
        self.text_start = tag_end;

        if is_end {
            self.tokens.push(Token::End(name));
            return true;
        }
        let self_closing = inner.trim_end().ends_with('/');
        if RAW_TEXT.contains(&name.as_str()) && !self_closing {
            self.raw_text(name);
        } else {
            let attrs = parse_attrs(&inner[name_len..]);
            self.tokens.push(Token::Start { name, attrs });
        }
        true
    }

    /// Swallow the content of raw-text element `name` up to its close tag.
    fn raw_text(&mut self, name: String) {
        let html = self.html;
        let i = self.i;
        let close = format!("</{name}");
        let end = find_ci(&html[i..], &close).map_or(html.len(), |p| i + p);
        self.tokens.push(Token::Raw {
            tag: name,
            text: html[i..end].to_string(),
        });
        self.i = html[end..].find('>').map_or(html.len(), |p| end + p + 1);
        self.text_start = self.i;
    }
}

/// End of a comment, doctype or processing instruction starting at
/// `html[i] == '<'` (followed by `next`), or `None` when it is not one.
fn markup_end(html: &str, i: usize, next: u8) -> Option<usize> {
    if html[i..].starts_with("<!--") {
        Some(
            html[i + 4..]
                .find("-->")
                .map_or(html.len(), |p| i + 4 + p + 3),
        )
    } else if next == b'!' || next == b'?' {
        Some(html[i..].find('>').map_or(html.len(), |p| i + p + 1))
    } else {
        None
    }
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

pub(super) fn attr<'a>(attrs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    attrs
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, v)| v.as_str())
}
