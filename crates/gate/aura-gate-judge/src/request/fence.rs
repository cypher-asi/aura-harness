//! The user's prompt as a model is shown it to label or read: the same
//! for the same prompt, and fenced as text, so a prompt that reads as
//! instructions is not taken for the model's own task.

use super::cut::excerpt;

/// The name of the tags that fence a prompt.
const TAG: &str = "request";

/// `prompt` as a model is shown it: its size, then its text between
/// `<request>` tags, normalized (line ends, control characters, trailing
/// spaces and runs of blank lines) and, when long, cut to its first
/// `head` and last `tail` characters around a note of how many were left
/// out. Tags in the text that would open or close the fence are broken
/// up.
pub(crate) fn fenced(prompt: &str, head: usize, tail: usize) -> String {
    let text = normalize(prompt);
    let chars = text.chars().count();
    let lines = text.lines().count();
    format!(
        "Length: {chars} characters, {lines} lines.\n{}",
        fence(&text, chars, head, tail)
    )
}

/// `prompt` as [`fenced`] shows it, without its size: a model judging how
/// hard the work is reads a long prompt's size as a sign it is hard.
pub(crate) fn fenced_text(prompt: &str, head: usize, tail: usize) -> String {
    let text = normalize(prompt);
    let chars = text.chars().count();
    fence(&text, chars, head, tail)
}

/// Normalized `text`, of `chars` characters, cut and between `<request>`
/// tags as [`fenced`] puts it.
fn fence(text: &str, chars: usize, head: usize, tail: usize) -> String {
    format!(
        "<request>\n{}\n</request>",
        unfenced(&excerpt(text, chars, head, tail))
    )
}

/// `prompt` with `\n` line ends, no control characters but tabs, no
/// trailing spaces on its lines, at most one blank line in a row, and no
/// blank lines at its start or end.
fn normalize(prompt: &str) -> String {
    let mut out = String::with_capacity(prompt.len());
    let mut blank = false;
    let lines = prompt
        .split("\r\n")
        .flat_map(|piece| piece.split(['\r', '\n']));
    for line in lines {
        let line: String = line
            .chars()
            .filter(|c| *c == '\t' || !c.is_control())
            .collect();
        let line = line.trim_end();
        if line.is_empty() {
            blank = !out.is_empty();
            continue;
        }
        if !out.is_empty() {
            out.push_str(if blank { "\n\n" } else { "\n" });
        }
        blank = false;
        out.push_str(line);
    }
    out
}

/// `text` with every `<request` or `</request` (any case) broken up by a
/// space after the `<`, so it cannot open or close the fence.
fn unfenced(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    for (at, _) in text.match_indices('<') {
        let rest = &text[at + 1..];
        let name = rest.strip_prefix('/').unwrap_or(rest);
        if name
            .get(..TAG.len())
            .is_some_and(|word| word.eq_ignore_ascii_case(TAG))
        {
            out.push_str(&text[copied..=at]);
            out.push(' ');
            copied = at + 1;
        }
    }
    out.push_str(&text[copied..]);
    out
}
