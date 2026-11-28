//! Subtask names that need no model: the description a `task` call gave,
//! or one made from the subtask's prompt.
//!
//! Prompts handed to helpers rarely open with the work. They open with
//! where to work ("In the Rust workspace at /workspace, ...") or who the
//! helper is ("You are working in ..."), so the name is made from the
//! first sentence that says what to do, with such boilerplate skipped.

use super::MAX_SUBTITLE_CHARS;
use aura_agent::titles::{cut_at_word, sanitize};

/// Leading verbs a subtask name made from its prompt drops: they open most
/// task prompts ("Implement the ...") and say nothing about the subtask.
const FILLER_VERBS: &[&str] = &[
    "implement",
    "add",
    "fix",
    "finish",
    "create",
    "update",
    "write",
    "build",
    "make",
];

/// Articles dropped after a filler verb.
const ARTICLES: &[&str] = &["the", "a", "an"];

/// Openings of sentences that set the helper up instead of naming the
/// work, lower-cased.
const BOILERPLATE: &[&str] = &[
    "you are",
    "you're",
    "you will",
    "you work",
    "you have",
    "you can",
    "you may",
    "work only",
    "only work",
    "work in",
    "do not",
    "don't",
    "never",
    "always",
    "make sure",
    "remember",
    "note",
    "important",
    "this is",
    "the repo",
    "the repository",
    "the workspace",
    "the project",
    "repo rules",
    "follow",
    "when done",
    "when you are done",
    "report",
];

/// Openings that lead into the work itself, dropped to keep what follows
/// ("Your task is to write the inbox tests"), lower-cased.
const LEAD_INS: &[&str] = &[
    "your task is to ",
    "your job is to ",
    "your goal is to ",
    "the task is to ",
    "the goal is to ",
    "you need to ",
    "i need you to ",
    "i want you to ",
    "we need to ",
    "please ",
];

/// Words that start a clause saying where to work ("In the Rust
/// workspace, ...").
const LOCATIONS: &[&str] = &["in", "inside", "within", "under", "at", "from"];

/// Labels and headings that only open a section ("Context:", "## Task"):
/// a section of setup is skipped, and the label of the work is dropped.
const SETUP_SECTIONS: &[&str] = &[
    "context",
    "background",
    "note",
    "notes",
    "rules",
    "constraints",
    "important",
    "setup",
    "environment",
    "workspace",
    "repo",
    "repository",
];

/// Labels that name the work after them ("Task: review the parser").
const WORK_SECTIONS: &[&str] = &[
    "task",
    "goal",
    "objective",
    "subtask",
    "job",
    "todo",
    "scope",
    "overview",
    "summary",
    "instructions",
    "requirements",
    "details",
];

/// The subtitle a `task` call's `description` makes: cleaned and cut to
/// fit a subtask row. `None` when there is none, or it says nothing.
pub(in crate::event_loop) fn described_subtitle(description: Option<&str>) -> Option<String> {
    let title = sanitize(description?, MAX_SUBTITLE_CHARS)?;
    let title = trim_dangling(&title);
    (!title.is_empty()).then_some(title)
}

/// The subtitle of an agent spawned with `description`, else made from
/// its first prompt, which `brief` reads only when it is needed. `None`
/// when there is neither.
pub(in crate::event_loop) fn subtitle_of_spawn(
    description: Option<&str>,
    brief: impl FnOnce() -> Option<String>,
) -> Option<String> {
    described_subtitle(description).or_else(|| brief().map(|prompt| fallback_subtitle(&prompt)))
}

/// A subtask name made from its task `prompt`: its first sentence that
/// says what to do, without boilerplate or a leading filler verb
/// ("Implement the engine core" names "Engine core"), cut to fit a
/// subtask row. A prompt of only boilerplate is named from its first
/// line.
pub(in crate::event_loop) fn fallback_subtitle(prompt: &str) -> String {
    let first_line = || {
        prompt
            .lines()
            .map(plain)
            .find(|line| !line.is_empty())
            .unwrap_or_default()
    };
    let work = sentences(prompt)
        .into_iter()
        .find_map(|sentence| the_work(&sentence))
        .unwrap_or_else(first_line);
    let title = name_from(&work);
    if title.is_empty() {
        "Subtask".to_string()
    } else {
        title
    }
}

/// The sentences of `prompt`, line by line, each without markdown marks.
fn sentences(prompt: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in prompt.lines().map(plain) {
        let mut rest = line.as_str();
        while let Some(end) = sentence_end(rest) {
            out.push(rest[..end].trim().to_string());
            rest = &rest[end + 1..];
        }
        out.push(rest.trim().to_string());
    }
    out.retain(|sentence| !sentence.is_empty());
    out
}

/// Byte index of the `.`, `!` or `?` that ends the first sentence of
/// `text`: one followed by a space. A dot inside a path or a number
/// ("main.rs", "1.85") doesn't end it.
fn sentence_end(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    bytes.iter().enumerate().find_map(|(at, byte)| {
        let ends = matches!(byte, b'.' | b'!' | b'?');
        let then_space = bytes.get(at + 1).is_some_and(u8::is_ascii_whitespace);
        (ends && then_space).then_some(at)
    })
}

/// `line` trimmed, without the markdown marks of a heading, quote, list
/// item or emphasis.
fn plain(line: &str) -> String {
    let line = line
        .trim()
        .trim_start_matches(['#', '>', '-', '*', '+'])
        .trim_start();
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    let line = match line[digits..].strip_prefix(['.', ')']) {
        Some(rest) if digits > 0 => rest.trim_start(),
        _ => line,
    };
    line.replace(['*', '`'], "")
        .replace("__", "")
        .trim()
        .to_string()
}

/// What `sentence` asks to be done, or `None` when it only sets the
/// helper up: a setup section or a lone heading, a sentence that is only
/// where to work, or boilerplate. A label of the work, a lead-in and a
/// clause saying where to work are dropped.
fn the_work(sentence: &str) -> Option<String> {
    let sentence = after_label(sentence)?;
    let sentence = after_location(sentence)?;
    let sentence = after_lead_in(sentence);
    let lower = sentence.to_lowercase();
    let lone_heading = is_one_of(&lower, SETUP_SECTIONS) || is_one_of(&lower, WORK_SECTIONS);
    let boilerplate = BOILERPLATE
        .iter()
        .any(|opening| starts_with_words(&lower, opening));
    let says_little = sentence.chars().filter(|c| c.is_alphabetic()).count() < 3;
    (!lone_heading && !boilerplate && !says_little).then(|| sentence.to_string())
}

/// `sentence` after a label of the work ("Task: ..."); `None` for one in
/// a setup section ("Context: ...") or a label with nothing after it.
fn after_label(sentence: &str) -> Option<&str> {
    let Some((label, rest)) = sentence.split_once(':') else {
        return Some(sentence);
    };
    let label = label.trim().to_lowercase();
    if is_one_of(&label, SETUP_SECTIONS) {
        return None;
    }
    if !is_one_of(&label, WORK_SECTIONS) {
        return Some(sentence);
    }
    let rest = rest.trim();
    (!rest.is_empty()).then_some(rest)
}

/// `sentence` after a clause saying where to work ("In the Rust
/// workspace at /workspace, find ..." reads "find ..."); `None` when it
/// says nothing else.
fn after_location(sentence: &str) -> Option<&str> {
    let first = sentence.split_whitespace().next().unwrap_or_default();
    if !is_one_of(&first.to_lowercase(), LOCATIONS) {
        return Some(sentence);
    }
    let (_, rest) = sentence.split_once(',')?;
    let rest = rest.trim();
    (!rest.is_empty()).then_some(rest)
}

/// `sentence` after a lead-in to the work ("Please ...").
fn after_lead_in(sentence: &str) -> &str {
    let lower = sentence.to_lowercase();
    LEAD_INS
        .iter()
        .find(|lead_in| lower.starts_with(*lead_in))
        // The lead-ins are ASCII, so their length is the same in `sentence`.
        .and_then(|lead_in| sentence.get(lead_in.len()..))
        .unwrap_or(sentence)
        .trim_start()
}

/// A subtask name from `work`, the sentence that names it: a leading
/// filler verb and its article dropped, cut at a word to fit a subtask
/// row, words a cut left dangling trimmed, and capitalized.
fn name_from(work: &str) -> String {
    let work = work.trim_end_matches(['.', '!', '?', ':', ';', ',']);
    let words: Vec<&str> = work.split_whitespace().collect();
    let mut start = 0;
    if words.first().is_some_and(|w| is_one_of(w, FILLER_VERBS)) {
        start = 1;
        if words.get(1).is_some_and(|w| is_one_of(w, ARTICLES)) {
            start = 2;
        }
    }
    // Keep the verb when nothing would be left without it.
    let rest = words.get(start..).filter(|rest| !rest.is_empty());
    let rest = rest.unwrap_or(&words);
    capitalize(&trim_dangling(&cut_at_word(
        &rest.join(" "),
        MAX_SUBTITLE_CHARS,
    )))
}

/// Whether `word` is one of `list`, ignoring ASCII case.
fn is_one_of(word: &str, list: &[&str]) -> bool {
    list.iter().any(|entry| word.eq_ignore_ascii_case(entry))
}

/// Whether `lower` opens with the words of `opening`, a whole word at
/// its end ("note" opens "note that", not "notes").
fn starts_with_words(lower: &str, opening: &str) -> bool {
    lower
        .strip_prefix(opening)
        .is_some_and(|rest| rest.chars().next().is_none_or(|c| !c.is_alphanumeric()))
}

/// `text` without the articles, prepositions and conjunctions a cut left
/// dangling at its end ("Engine core in the" reads "Engine core"). Keeps
/// at least the first word.
fn trim_dangling(text: &str) -> String {
    const DANGLING: &[&str] = &[
        "the", "a", "an", "in", "of", "for", "to", "and", "or", "with", "on", "at", "from", "by",
    ];
    let mut words: Vec<&str> = text.split_whitespace().collect();
    while words.len() > 1 && words.last().is_some_and(|w| is_one_of(w, DANGLING)) {
        words.pop();
    }
    words.join(" ")
}

/// `text` with its first character upper-cased.
fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

#[cfg(test)]
#[path = "subtask_tests.rs"]
mod tests;
