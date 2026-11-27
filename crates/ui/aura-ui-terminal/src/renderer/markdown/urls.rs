//! Web addresses written out in prose, without markdown around them.

use crate::app::chat_links::{is_url_char, strip_prefix_ignore_case};

/// Characters that end a sentence or a quote rather than a URL, when a
/// URL ends with them.
const TRAILING: [char; 13] = [
    '.', ',', ';', ':', '!', '?', '\'', '"', '*', '_', '~', '\u{2019}', '\u{201d}',
];

/// The `http://` or `https://` URL starting at `i` in `text`, and the
/// index just past it. It starts a word, so `xhttp://` is not one, and
/// runs to the next space or character a URL escapes, less what ends the
/// sentence after it: `.`, `,`, `;`, `:`, `!`, `?`, quotes, and a `)` or
/// `]` the URL did not open, as in `(see https://x.dev)`.
pub(super) fn bare_url(text: &str, i: usize) -> Option<(&str, usize)> {
    let starts_word = text[..i]
        .chars()
        .next_back()
        .is_none_or(|c| !c.is_alphanumeric() && !matches!(c, '/' | '.' | '@' | '-'));
    if !starts_word {
        return None;
    }
    let rest = &text[i..];
    let after = strip_prefix_ignore_case(rest, "https://")
        .or_else(|| strip_prefix_ignore_case(rest, "http://"))?;
    let scheme = rest.len() - after.len();
    let len = rest.find(|c: char| !is_url_char(c)).unwrap_or(rest.len());
    let url = trim_trailing(&rest[..len]);
    (url.len() > scheme && !url[scheme..].starts_with(['/', '.'])).then(|| (url, i + url.len()))
}

/// `url` without the punctuation of the sentence it ends.
fn trim_trailing(mut url: &str) -> &str {
    loop {
        let Some(last) = url.chars().next_back() else {
            return url;
        };
        let unbalanced = |open: char| url.matches(open).count() < url.matches(last).count();
        let trims = TRAILING.contains(&last)
            || (last == ')' && unbalanced('('))
            || (last == ']' && unbalanced('['));
        if !trims {
            return url;
        }
        url = &url[..url.len() - last.len_utf8()];
    }
}

#[cfg(test)]
mod tests {
    use super::bare_url;

    fn found(text: &str) -> Option<&str> {
        let i = text.find(['h', 'H'])?;
        bare_url(text, i).map(|(url, _)| url)
    }

    #[test]
    fn a_url_ends_before_the_sentence_around_it() {
        for (text, url) in [
            ("see https://x.dev.", "https://x.dev"),
            ("https://x.dev, then", "https://x.dev"),
            ("is it https://x.dev/a?", "https://x.dev/a"),
            ("go: https://x.dev/a?b=1;", "https://x.dev/a?b=1"),
            ("'https://x.dev/'", "https://x.dev/"),
            ("(see https://x.dev/a)", "https://x.dev/a"),
            (
                "https://en.wikipedia.org/wiki/Rust_(language))",
                "https://en.wikipedia.org/wiki/Rust_(language)",
            ),
            ("[https://x.dev]", "https://x.dev"),
            ("HTTP://X.DEV!", "HTTP://X.DEV"),
            ("“https://x.dev”", "https://x.dev"),
            ("at http://localhost:8080/", "http://localhost:8080/"),
        ] {
            assert_eq!(found(text), Some(url), "{text}");
        }
    }

    #[test]
    fn what_only_looks_like_a_url_is_not_one() {
        for text in [
            "xhttps://x.dev",
            "https://",
            "https://.",
            "http:/x.dev",
            "hello",
            "https:///x",
        ] {
            assert_eq!(found(text), None, "{text}");
        }
    }

    #[test]
    fn the_end_is_just_past_the_url() {
        let text = "a https://x.dev. b";
        assert_eq!(bare_url(text, 2), Some(("https://x.dev", 15)));
    }
}
