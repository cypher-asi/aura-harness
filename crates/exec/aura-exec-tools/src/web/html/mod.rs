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

mod render;
mod text;
mod tokenize;

use render::{focus_element, render};
use reqwest::Url;
use text::{collapse_whitespace, decode_entities};
use tokenize::{tokenize, Token};

/// A converted page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    /// Contents of `<title>`, if any.
    pub title: Option<String>,
    /// The page body as markdown.
    pub markdown: String,
}

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

#[cfg(test)]
mod tests;
