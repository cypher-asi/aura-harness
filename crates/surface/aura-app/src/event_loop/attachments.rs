//! What a user message carries besides its text: files and folders named
//! with `@path`, and pasted images.
//!
//! The chat and the record keep the message as typed; only the model gets
//! the file contents, appended to the text when the turn starts.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tracing::warn;

/// Most bytes of one file given to the model.
pub(crate) const MAX_FILE_BYTES: usize = 64 * 1024;

/// Most bytes of all files in one message, so a handful of mentions can't
/// fill the context window.
const MAX_TOTAL_BYTES: usize = 256 * 1024;

/// Most entries listed for a mentioned folder.
const MAX_FOLDER_ENTRIES: usize = 200;

/// Most mentions attached from one message.
const MAX_MENTIONS: usize = 20;

/// Bytes checked for NUL to tell a binary file from text.
const BINARY_SNIFF_BYTES: usize = 8 * 1024;

/// How long reading the mentioned files may take. A file on a hung network
/// mount must not hold up the turn forever.
const READ_TIMEOUT: Duration = Duration::from_secs(10);

/// Characters a mention may end with that are punctuation, not part of
/// the path (`see @src/main.rs.`).
const TRAILING_PUNCTUATION: &[char] = &['.', ',', ';', ':', '!', '?', ')', ']', '}', '"', '\''];

/// A message's text as the model gets it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Expanded {
    /// The text with the mentioned files and folders appended
    pub text: String,
    /// Mentions that were attached, as typed (without the `@`)
    pub attached: Vec<String>,
}

/// `@path` words of `text`, in order and without repeats. Only a word that
/// starts the text or follows a blank counts, so email addresses don't.
pub(crate) fn mentioned_paths(text: &str) -> Vec<&str> {
    let mut paths: Vec<&str> = Vec::new();
    for word in text.split_whitespace() {
        let Some(path) = word.strip_prefix('@') else {
            continue;
        };
        if !path.is_empty() && !paths.contains(&path) {
            paths.push(path);
        }
    }
    paths
}

/// Append the files and folders `text` mentions to it. Mentions that don't
/// name something inside `root` are left as plain text.
pub(crate) fn expand_mentions(root: &Path, text: &str) -> Expanded {
    let mut out = text.to_string();
    let mut attached = Vec::new();
    let mut budget = MAX_TOTAL_BYTES;
    let Ok(root) = root.canonicalize() else {
        return Expanded {
            text: out,
            attached,
        };
    };
    for mention in mentioned_paths(text) {
        if attached.len() >= MAX_MENTIONS {
            break;
        }
        let Some((typed, path)) = resolve(&root, mention) else {
            continue;
        };
        let section = if path.is_dir() {
            folder_section(&root, &path, typed)
        } else {
            match file_section(&path, typed, budget) {
                Some((section, used)) => {
                    budget = budget.saturating_sub(used);
                    section
                }
                None => continue,
            }
        };
        out.push_str("\n\n");
        out.push_str(&section);
        attached.push(typed.to_string());
    }
    Expanded {
        text: out,
        attached,
    }
}

/// [`expand_mentions`] off the async runtime, with a time limit. On a
/// timeout the model gets the text as typed.
pub(crate) async fn expand_mentions_in(root: PathBuf, text: String) -> Expanded {
    let fallback = text.clone();
    let task = tokio::task::spawn_blocking(move || expand_mentions(&root, &text));
    match tokio::time::timeout(READ_TIMEOUT, task).await {
        Ok(Ok(expanded)) => expanded,
        Ok(Err(e)) => {
            warn!(error = %e, "Reading mentioned files failed");
            unexpanded(fallback)
        }
        Err(_) => {
            warn!("Reading mentioned files timed out");
            unexpanded(fallback)
        }
    }
}

fn unexpanded(text: String) -> Expanded {
    Expanded {
        text,
        attached: Vec::new(),
    }
}

/// The path `mention` names inside `root`, trying it without trailing
/// punctuation too. Returns the mention as matched and the canonical path.
/// Anything that resolves outside `root` (`..`, absolute paths, symlinks
/// out) is refused: the agent's own tools can't read there either.
fn resolve<'a>(root: &Path, mention: &'a str) -> Option<(&'a str, PathBuf)> {
    let mut typed = mention;
    loop {
        if let Ok(path) = root.join(typed).canonicalize() {
            if path.starts_with(root) {
                return Some((typed, path));
            }
            return None;
        }
        let trimmed = typed.trim_end_matches(TRAILING_PUNCTUATION);
        if trimmed == typed || trimmed.is_empty() {
            return None;
        }
        typed = trimmed;
    }
}

/// `<attached_file>` section for `path`, holding at most
/// [`MAX_FILE_BYTES`] and at most `budget` bytes, and how many bytes of
/// content it used. `None` when the file can't be read.
fn file_section(path: &Path, typed: &str, budget: usize) -> Option<(String, usize)> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) => {
            warn!(path = %path.display(), error = %e, "Cannot read mentioned file");
            return None;
        }
    };
    let total = bytes.len();
    let sniff = &bytes[..total.min(BINARY_SNIFF_BYTES)];
    if sniff.contains(&0) {
        let note = format!(
            "<attached_file path=\"{typed}\">\n[binary file, {}, not shown]\n</attached_file>",
            kib(total)
        );
        return Some((note, 0));
    }
    let cap = MAX_FILE_BYTES.min(budget);
    let shown = utf8_prefix(&bytes, cap);
    let mut section = format!("<attached_file path=\"{typed}\">\n{shown}");
    if !shown.is_empty() && !shown.ends_with('\n') {
        section.push('\n');
    }
    if shown.len() < total {
        let _ = writeln!(
            section,
            "[truncated: showing the first {} of {}]",
            kib(shown.len()),
            kib(total)
        );
    }
    section.push_str("</attached_file>");
    Some((section, shown.len()))
}

/// The longest prefix of `bytes` of at most `cap` bytes that is text,
/// cut at a character boundary. Invalid UTF-8 is replaced.
fn utf8_prefix(bytes: &[u8], cap: usize) -> String {
    let head = &bytes[..bytes.len().min(cap)];
    match std::str::from_utf8(head) {
        Ok(text) => text.to_string(),
        // The cap split a character: drop the partial one.
        Err(e) if e.error_len().is_none() => {
            String::from_utf8_lossy(&head[..e.valid_up_to()]).into_owned()
        }
        Err(_) => String::from_utf8_lossy(head).into_owned(),
    }
}

/// `<attached_folder>` section listing the entries of `dir`, folders
/// first, each relative to `root`.
fn folder_section(root: &Path, dir: &Path, typed: &str) -> String {
    let mut entries: Vec<(bool, String)> = std::fs::read_dir(dir)
        .map(|read_dir| {
            read_dir
                .filter_map(Result::ok)
                .map(|entry| {
                    let is_dir = entry.file_type().is_ok_and(|t| t.is_dir());
                    let path = entry.path();
                    let relative = path.strip_prefix(root).unwrap_or(&path);
                    let mut name = relative.to_string_lossy().replace('\\', "/");
                    if is_dir {
                        name.push('/');
                    }
                    (is_dir, name)
                })
                .collect()
        })
        .unwrap_or_default();
    entries.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    let total = entries.len();
    let mut section = format!("<attached_folder path=\"{typed}\">\n");
    for (_, name) in entries.iter().take(MAX_FOLDER_ENTRIES) {
        section.push_str(name);
        section.push('\n');
    }
    if total > MAX_FOLDER_ENTRIES {
        let _ = writeln!(
            section,
            "[{} more entries not listed]",
            total - MAX_FOLDER_ENTRIES
        );
    }
    section.push_str("</attached_folder>");
    section
}

/// `12 KiB`, rounded up so a small file never reads as 0.
fn kib(bytes: usize) -> String {
    format!("{} KiB", bytes.div_ceil(1024))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn workspace() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src/nested")).unwrap();
        fs::write(dir.path().join("src/main.rs"), "fn main() {}\n").unwrap();
        fs::write(dir.path().join("src/lib.rs"), "pub fn lib() {}").unwrap();
        dir
    }

    #[test]
    fn mentions_are_words_starting_with_at() {
        assert_eq!(
            mentioned_paths("read @a.rs and @b/ then @a.rs, mail x@y.z @"),
            ["a.rs", "b/", "a.rs,"]
        );
    }

    #[test]
    fn a_mentioned_file_is_appended_for_the_model() {
        let dir = workspace();
        let expanded = expand_mentions(dir.path(), "explain @src/main.rs.");
        assert_eq!(expanded.attached, ["src/main.rs"]);
        assert_eq!(
            expanded.text,
            "explain @src/main.rs.\n\n<attached_file path=\"src/main.rs\">\nfn main() {}\n</attached_file>"
        );
    }

    #[test]
    fn a_mentioned_folder_attaches_a_listing() {
        let dir = workspace();
        let expanded = expand_mentions(dir.path(), "@src/ what is here?");
        assert_eq!(expanded.attached, ["src/"]);
        assert!(expanded.text.ends_with(
            "<attached_folder path=\"src/\">\nsrc/nested/\nsrc/lib.rs\nsrc/main.rs\n</attached_folder>"
        ));
    }

    #[test]
    fn large_files_are_truncated_with_a_note() {
        let dir = workspace();
        fs::write(dir.path().join("big.txt"), "é".repeat(MAX_FILE_BYTES)).unwrap();
        let expanded = expand_mentions(dir.path(), "@big.txt");
        let body = expanded.text.split_once('\n').unwrap().1;
        assert!(body.len() < MAX_FILE_BYTES + 200, "{}", body.len());
        assert!(expanded
            .text
            .contains("[truncated: showing the first 64 KiB of 128 KiB]"));
    }

    #[test]
    fn binary_files_are_described_not_shown() {
        let dir = workspace();
        fs::write(dir.path().join("blob.bin"), [0u8, 1, 2, 3]).unwrap();
        let expanded = expand_mentions(dir.path(), "@blob.bin");
        assert!(expanded.text.contains("[binary file, 1 KiB, not shown]"));
    }

    #[test]
    fn mentions_outside_the_workspace_or_missing_stay_text() {
        let dir = workspace();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("secret"), "nope").unwrap();
        let escape = format!("@{}", outside.path().join("secret").display());
        let text = format!("@../secret {escape} @missing.rs");
        let expanded = expand_mentions(dir.path(), &text);
        assert!(expanded.attached.is_empty());
        assert_eq!(expanded.text, text);
    }
}
