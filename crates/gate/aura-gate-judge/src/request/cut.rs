//! Long parts of a prompt cut to what a model is shown of them: their
//! start, their end, or both around a note of what was left out.

use std::borrow::Cow;

/// `text` cut to its first `max` characters, saying that `what` was cut
/// when it was.
pub(crate) fn cut<'a>(text: &'a str, max: usize, what: &str) -> Cow<'a, str> {
    match start_end(text, max) {
        Some(end) => Cow::Owned(format!("{}\n... ({what} cut)", &text[..end])),
        None => Cow::Borrowed(text),
    }
}

/// The last `max` characters of `text`, saying so when its start was
/// cut.
pub(crate) fn tail(text: &str, max: usize) -> Cow<'_, str> {
    match end_start(text, max) {
        Some(start) => Cow::Owned(format!("(start cut) ...\n{}", &text[start..])),
        None => Cow::Borrowed(text),
    }
}

/// `text`, `chars` characters long, whole when it is at most `head +
/// tail`, else its first `head` and last `tail` characters around a note
/// of how many were left out.
pub(super) fn excerpt(text: &str, chars: usize, head: usize, tail: usize) -> Cow<'_, str> {
    if chars <= head + tail {
        return Cow::Borrowed(text);
    }
    let (Some(end), Some(start)) = (start_end(text, head), end_start(text, tail)) else {
        return Cow::Borrowed(text);
    };
    Cow::Owned(format!(
        "{}\n[... {} characters omitted ...]\n{}",
        &text[..end],
        chars - head - tail,
        &text[start..]
    ))
}

/// What `show` makes of `text`, or `blank` in its place when `text` is
/// blank.
pub(crate) fn or_blank<'a>(
    text: &'a str,
    blank: &'static str,
    show: impl FnOnce(&'a str) -> Cow<'a, str>,
) -> Cow<'a, str> {
    if text.trim().is_empty() {
        Cow::Borrowed(blank)
    } else {
        show(text)
    }
}

/// Where the first `n` characters of `text` end, as a byte index;
/// `None` when it has no more than `n`.
fn start_end(text: &str, n: usize) -> Option<usize> {
    text.char_indices().nth(n).map(|(at, _)| at)
}

/// Where the last `n` characters of `text` start, as a byte index;
/// `None` when it has no more than `n`.
fn end_start(text: &str, n: usize) -> Option<usize> {
    text.char_indices()
        .nth_back(n)
        .map(|(at, c)| at + c.len_utf8())
}
