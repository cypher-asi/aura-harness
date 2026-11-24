//! The marker a request's [`crate::ModelRequest::cache_salt`] is sent as.
//!
//! Providers cache prompt prefixes: Anthropic at its `cache_control`
//! breakpoints, in the order tools, system, messages; OpenAI on every
//! prefix of the prompt, from its first token. Two requests share a cache
//! entry only when their bytes agree up to it, so a marker in the earliest
//! content a provider caches keeps differently salted requests apart from
//! the first entry on. The marker is one short line that asks nothing of
//! the model.

use std::borrow::Cow;

/// `salt` as the line a marker holds.
fn marker(salt: &str) -> String {
    format!("[cache scope: {salt}]")
}

/// `text` with `salt`'s marker as its last line (the marker alone for
/// empty `text`); `text` itself without a salt.
pub(crate) fn closing<'a>(text: &'a str, salt: Option<&str>) -> Cow<'a, str> {
    match salt {
        Some(salt) if text.is_empty() => Cow::Owned(marker(salt)),
        Some(salt) => Cow::Owned(format!("{text}\n\n{}", marker(salt))),
        None => Cow::Borrowed(text),
    }
}

/// `text` with `salt`'s marker as its first line (the marker alone for
/// empty `text`); `text` itself without a salt.
pub(crate) fn opening<'a>(text: &'a str, salt: Option<&str>) -> Cow<'a, str> {
    match salt {
        Some(salt) if text.is_empty() => Cow::Owned(marker(salt)),
        Some(salt) => Cow::Owned(format!("{}\n\n{text}", marker(salt))),
        None => Cow::Borrowed(text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_salt_leaves_text_as_it_is() {
        assert!(matches!(closing("tools", None), Cow::Borrowed("tools")));
        assert!(matches!(opening("system", None), Cow::Borrowed("system")));
    }

    #[test]
    fn salt_closes_or_opens_text() {
        assert_eq!(
            closing("Reads.", Some("x:A")),
            "Reads.\n\n[cache scope: x:A]"
        );
        assert_eq!(
            opening("Be brief.", Some("x:A")),
            "[cache scope: x:A]\n\nBe brief."
        );
        assert_eq!(opening("", Some("x:B")), "[cache scope: x:B]");
        assert_eq!(closing("", Some("x:B")), "[cache scope: x:B]");
    }
}
