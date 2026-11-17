//! `@` mentions: a fuzzy file and folder picker shown above the input
//! while an `@path` word is typed.
//!
//! The picker only completes text. The surface reads the mentioned files
//! when the message is sent, so the chat keeps the short `@path`.

use super::{App, AppState};
use crate::terminal::KeyResult;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Most files and folders indexed under the workspace root. A huge tree
/// would stall typing while it is walked.
const MAX_INDEXED: usize = 20_000;

/// Deepest level walked below the root.
const MAX_DEPTH: usize = 16;

/// Most matches offered at once.
pub const MAX_MATCHES: usize = 50;

/// How long a walk of the workspace is reused while one `@` word is
/// typed. A new `@` word walks again so new files show up.
const INDEX_TTL: Duration = Duration::from_secs(30);

/// Folders never offered: build output, VCS data, and installed packages
/// are huge and rarely what the user means.
const SKIPPED_DIRS: &[&str] = &[".git", "target", "node_modules"];

/// A file or folder the picker can insert.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MentionCandidate {
    /// Path relative to the workspace root with `/` separators; folders
    /// end in `/`
    pub path: String,
    /// Whether it is a folder
    pub is_dir: bool,
}

impl MentionCandidate {
    fn name(&self) -> &str {
        let trimmed = self.path.trim_end_matches('/');
        trimmed.rsplit('/').next().unwrap_or(trimmed)
    }
}

/// Files and folders under one workspace root, shallowest first.
#[derive(Debug)]
pub(super) struct MentionIndex {
    root: PathBuf,
    built_at: Instant,
    entries: Vec<MentionCandidate>,
}

impl MentionIndex {
    /// Walk `root` breadth first, skipping [`SKIPPED_DIRS`] and names the
    /// root `.gitignore` lists. Paths with whitespace are left out: a
    /// mention ends at the first blank.
    pub(super) fn build(root: &Path) -> Self {
        let ignore = GitIgnore::read(root);
        let mut entries = Vec::new();
        let mut queue = VecDeque::from([(root.to_path_buf(), String::new(), 0usize)]);
        while let Some((dir, prefix, depth)) = queue.pop_front() {
            let Ok(read_dir) = std::fs::read_dir(&dir) else {
                continue;
            };
            let mut children: Vec<(String, bool)> = read_dir
                .filter_map(Result::ok)
                .filter_map(|entry| {
                    let name = entry.file_name().to_str()?.to_string();
                    // Symlinks are listed as files and never walked, which
                    // keeps the index inside the root.
                    let is_dir = entry.file_type().ok()?.is_dir();
                    Some((name, is_dir))
                })
                .filter(|(name, is_dir)| {
                    let skipped = *is_dir && SKIPPED_DIRS.contains(&name.as_str());
                    let ignored = skipped
                        || name.contains(char::is_whitespace)
                        || ignore.matches(name, *is_dir);
                    !ignored
                })
                .collect();
            children.sort_by(|a, b| {
                b.1.cmp(&a.1)
                    .then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase()))
            });
            for (name, is_dir) in children {
                if entries.len() >= MAX_INDEXED {
                    return Self::finished(root, entries);
                }
                let path = format!("{prefix}{name}{}", if is_dir { "/" } else { "" });
                if is_dir && depth + 1 < MAX_DEPTH {
                    queue.push_back((dir.join(&name), path.clone(), depth + 1));
                }
                entries.push(MentionCandidate { path, is_dir });
            }
        }
        Self::finished(root, entries)
    }

    fn finished(root: &Path, entries: Vec<MentionCandidate>) -> Self {
        Self {
            root: root.to_path_buf(),
            built_at: Instant::now(),
            entries,
        }
    }

    fn is_fresh_for(&self, root: &Path) -> bool {
        self.root == root && self.built_at.elapsed() < INDEX_TTL
    }

    /// Best matches for `query`, best first, at most [`MAX_MATCHES`].
    pub(super) fn rank(&self, query: &str) -> Vec<MentionCandidate> {
        rank(&self.entries, query)
    }
}

/// How well a candidate matches; lower sorts first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum MatchTier {
    /// The file name starts with the query
    NamePrefix,
    /// The relative path starts with the query
    PathPrefix,
    /// The file name contains the query
    NameContains,
    /// The relative path contains the query
    PathContains,
    /// The query's characters appear in order in the path
    Subsequence,
}

/// Rank `entries` for `query`: prefix matches first, then substrings,
/// then subsequences, each preferring tighter and shorter paths. An empty
/// query keeps the index order (shallowest first).
fn rank(entries: &[MentionCandidate], query: &str) -> Vec<MentionCandidate> {
    if query.is_empty() {
        return entries.iter().take(MAX_MATCHES).cloned().collect();
    }
    let query = query.to_lowercase();
    let mut scored: Vec<((MatchTier, usize, usize), &MentionCandidate)> = entries
        .iter()
        .filter_map(|candidate| {
            let path = candidate.path.to_lowercase();
            let name = candidate.name().to_lowercase();
            let tier = if name.starts_with(&query) {
                MatchTier::NamePrefix
            } else if path.starts_with(&query) {
                MatchTier::PathPrefix
            } else if name.contains(&query) {
                MatchTier::NameContains
            } else if path.contains(&query) {
                MatchTier::PathContains
            } else {
                let span = subsequence_span(&path, &query)?;
                return Some(((MatchTier::Subsequence, span, path.len()), candidate));
            };
            Some(((tier, 0, path.len()), candidate))
        })
        .collect();
    scored.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.path.cmp(&b.1.path)));
    scored
        .into_iter()
        .take(MAX_MATCHES)
        .map(|(_, candidate)| candidate.clone())
        .collect()
}

/// Length of the shortest stretch of `text` holding `query`'s characters
/// in order, or `None` when they don't all appear.
fn subsequence_span(text: &str, query: &str) -> Option<usize> {
    let text: Vec<char> = text.chars().collect();
    let query: Vec<char> = query.chars().collect();
    let first = *query.first()?;
    let mut best: Option<usize> = None;
    for start in (0..text.len()).filter(|&i| text[i] == first) {
        let mut want = 1;
        let mut end = start;
        for (i, &c) in text.iter().enumerate().skip(start + 1) {
            if want == query.len() {
                break;
            }
            if c == query[want] {
                want += 1;
                end = i;
            }
        }
        if want == query.len() {
            let span = end - start + 1;
            best = Some(best.map_or(span, |b| b.min(span)));
        }
    }
    best
}

/// The plain names and `*.ext` patterns of a root `.gitignore`. Anything
/// more elaborate (negation, nested paths, character classes) is ignored:
/// the skip list is a convenience, not a security boundary.
#[derive(Debug, Default)]
struct GitIgnore {
    names: Vec<String>,
    dir_names: Vec<String>,
    suffixes: Vec<String>,
}

impl GitIgnore {
    fn read(root: &Path) -> Self {
        let Ok(text) = std::fs::read_to_string(root.join(".gitignore")) else {
            return Self::default();
        };
        let mut ignore = Self::default();
        for line in text.lines().map(str::trim) {
            if line.is_empty() || line.starts_with('#') || line.starts_with('!') {
                continue;
            }
            let line = line.trim_start_matches('/');
            let (pattern, dir_only) = match line.strip_suffix('/') {
                Some(dir) => (dir, true),
                None => (line, false),
            };
            if pattern.contains('/') {
                continue;
            }
            if let Some(suffix) = pattern.strip_prefix('*') {
                if !suffix.is_empty() && !suffix.contains(['*', '?', '[']) {
                    ignore.suffixes.push(suffix.to_string());
                }
            } else if !pattern.contains(['*', '?', '[']) {
                if dir_only {
                    ignore.dir_names.push(pattern.to_string());
                } else {
                    ignore.names.push(pattern.to_string());
                }
            }
        }
        ignore
    }

    fn matches(&self, name: &str, is_dir: bool) -> bool {
        self.names.iter().any(|n| n == name)
            || (is_dir && self.dir_names.iter().any(|n| n == name))
            || self.suffixes.iter().any(|s| name.ends_with(s.as_str()))
    }
}

/// The `@word` around `cursor`: where its `@` is, where it ends, and the
/// text typed between the `@` and the cursor. Only a word that starts at
/// the beginning of the input or after a blank counts, so an email
/// address never opens the picker.
fn mention_at(input: &str, cursor: usize) -> Option<(usize, usize, &str)> {
    let cursor = cursor.min(input.len());
    let before = input.get(..cursor)?;
    let start = before
        .char_indices()
        .rev()
        .find(|(_, c)| c.is_whitespace())
        .map_or(0, |(i, c)| i + c.len_utf8());
    let query = before[start..].strip_prefix('@')?;
    let end = input[cursor..]
        .find(char::is_whitespace)
        .map_or(input.len(), |i| cursor + i);
    Some((start, end, query))
}

/// State of the `@` picker, kept between keys.
#[derive(Debug, Default)]
pub(crate) struct MentionMenu {
    /// Walk of the workspace the matches come from
    index: Option<MentionIndex>,
    /// `@` offset and query the matches were computed for
    query: Option<(usize, String)>,
    /// Matches for `query`, best first
    matches: Vec<MentionCandidate>,
    /// Highlighted row
    cursor: usize,
    /// `@` offset of the word whose picker Esc closed
    dismissed: Option<usize>,
}

impl App {
    /// Files and folders matching the `@word` at the cursor. Empty while
    /// the picker is closed, and while an approval prompt or another modal
    /// state has the keyboard: those can start without a key press (a tool
    /// call asking), so the picker is not re-synced before they show.
    #[must_use]
    pub fn mention_matches(&self) -> &[MentionCandidate] {
        let typing = matches!(self.state, AppState::Idle | AppState::Processing);
        if typing && self.mentions.query.is_some() {
            &self.mentions.matches
        } else {
            &[]
        }
    }

    /// Highlighted row in the `@` picker.
    #[must_use]
    pub fn mention_cursor(&self) -> usize {
        self.mentions
            .cursor
            .min(self.mentions.matches.len().saturating_sub(1))
    }

    /// Bring the `@` picker in line with the input: open it on an `@word`
    /// at the cursor, re-rank when the word changed, close it otherwise.
    pub(super) fn sync_mentions(&mut self) {
        let active =
            matches!(self.state, AppState::Idle | AppState::Processing) && self.detail.is_none();
        let word = active
            .then(|| mention_at(&self.input, self.cursor_pos))
            .flatten();
        // Esc closes the picker for one word; leaving that word forgets it.
        if word.map(|(start, _, _)| start) != self.mentions.dismissed {
            self.mentions.dismissed = None;
        }
        let word = word.filter(|(start, _, _)| self.mentions.dismissed != Some(*start));
        let (Some((start, _, query)), Some(root)) = (word, self.sandbox.root()) else {
            self.mentions.query = None;
            self.mentions.matches.clear();
            return;
        };
        if self
            .mentions
            .query
            .as_ref()
            .is_some_and(|(s, q)| *s == start && q == query)
        {
            return;
        }
        // A new `@` word walks the tree again so files created since show
        // up; typing within one word reuses the walk.
        let opening = self.mentions.query.is_none();
        let stale = self
            .mentions
            .index
            .as_ref()
            .is_none_or(|index| opening || !index.is_fresh_for(root));
        if stale {
            self.mentions.index = Some(MentionIndex::build(root));
        }
        self.mentions.matches = self
            .mentions
            .index
            .as_ref()
            .map(|index| index.rank(query))
            .unwrap_or_default();
        self.mentions.query = Some((start, query.to_string()));
        self.mentions.cursor = 0;
    }

    /// Handle a key while the `@` picker is open. `None` when it is closed
    /// or the key is not one it uses.
    pub(super) fn handle_mention_menu_key(&mut self, key: KeyEvent) -> Option<KeyResult> {
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return None;
        }
        let last = self.mention_matches().len().checked_sub(1)?;
        let selected = self.mention_matches()[self.mention_cursor()].clone();
        match key.code {
            KeyCode::Up => {
                self.mentions.cursor = self.mention_cursor().checked_sub(1).unwrap_or(last);
            }
            KeyCode::Down => {
                self.mentions.cursor = if self.mention_cursor() >= last {
                    0
                } else {
                    self.mention_cursor() + 1
                };
            }
            // Tab on a folder stays in the picker to go into it.
            KeyCode::Tab => self.insert_mention(&selected, !selected.is_dir),
            KeyCode::Enter => self.insert_mention(&selected, true),
            KeyCode::Esc => {
                self.mentions.dismissed = self.mentions.query.as_ref().map(|(start, _)| *start);
                self.mentions.query = None;
            }
            _ => return None,
        }
        Some(KeyResult::continue_running())
    }

    /// Replace the `@word` at the cursor with `@path`, followed by a blank
    /// when `finish` is set.
    fn insert_mention(&mut self, candidate: &MentionCandidate, finish: bool) {
        let Some((start, end, _)) = mention_at(&self.input, self.cursor_pos) else {
            return;
        };
        let mut text = format!("@{}", candidate.path);
        let next_is_blank = self.input[end..].starts_with(char::is_whitespace);
        if finish && !next_is_blank {
            text.push(' ');
        }
        self.input.replace_range(start..end, &text);
        self.cursor_pos = start + text.len() + usize::from(finish && next_is_blank);
        if finish {
            self.mentions.query = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn candidate(path: &str) -> MentionCandidate {
        MentionCandidate {
            path: path.to_string(),
            is_dir: path.ends_with('/'),
        }
    }

    fn ranked(paths: &[&str], query: &str) -> Vec<String> {
        let entries: Vec<_> = paths.iter().map(|p| candidate(p)).collect();
        rank(&entries, query).into_iter().map(|c| c.path).collect()
    }

    #[test]
    fn prefix_matches_beat_substrings_and_subsequences() {
        let paths = [
            "docs/main_notes.md",
            "src/domain.rs",
            "src/main.rs",
            "src/app/remain.rs",
            "Makefile",
        ];
        assert_eq!(
            ranked(&paths, "main"),
            [
                "src/main.rs",
                "docs/main_notes.md",
                "src/domain.rs",
                "src/app/remain.rs",
            ]
        );
        assert_eq!(
            ranked(&paths, "src/"),
            ["src/main.rs", "src/domain.rs", "src/app/remain.rs"]
        );
        assert_eq!(
            ranked(&paths, "smr"),
            ["src/main.rs", "src/domain.rs", "src/app/remain.rs"]
        );
        assert_eq!(ranked(&paths, "MAKE"), ["Makefile"]);
        assert!(ranked(&paths, "zzz").is_empty());
    }

    #[test]
    fn subsequence_span_prefers_tight_matches() {
        assert_eq!(subsequence_span("src/main.rs", "smr"), Some(10));
        assert_eq!(subsequence_span("a_b_ab", "ab"), Some(2));
        assert_eq!(subsequence_span("abc", "abc"), Some(3));
        assert_eq!(subsequence_span("abc", "cab"), None);
    }

    #[test]
    fn index_skips_build_output_and_gitignored_names() {
        let dir = tempfile::tempdir().unwrap();
        for sub in ["src", "target/debug", ".git", "node_modules/x", "dist"] {
            fs::create_dir_all(dir.path().join(sub)).unwrap();
        }
        fs::write(dir.path().join("src/main.rs"), "").unwrap();
        fs::write(dir.path().join("target/debug/app"), "").unwrap();
        fs::write(dir.path().join("debug.log"), "").unwrap();
        fs::write(dir.path().join("my notes.txt"), "").unwrap();
        fs::write(dir.path().join(".gitignore"), "/dist/\n*.log\n").unwrap();

        let index = MentionIndex::build(dir.path());
        let paths: Vec<&str> = index.entries.iter().map(|c| c.path.as_str()).collect();
        assert_eq!(paths, ["src/", ".gitignore", "src/main.rs"]);
    }

    #[test]
    fn matches_are_capped() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..MAX_MATCHES + 10 {
            fs::write(dir.path().join(format!("f{i:03}")), "").unwrap();
        }
        let index = MentionIndex::build(dir.path());
        assert_eq!(index.entries.len(), MAX_MATCHES + 10);
        assert_eq!(index.rank("").len(), MAX_MATCHES);
        assert_eq!(index.rank("f0").len(), MAX_MATCHES);
        assert_eq!(index.rank("f0")[0].path, "f000");
    }

    #[test]
    fn mention_word_needs_a_blank_before_the_at() {
        assert_eq!(mention_at("see @src/ma", 11), Some((4, 11, "src/ma")));
        assert_eq!(mention_at("@", 1), Some((0, 1, "")));
        assert_eq!(mention_at("mail me@host", 12), None);
        assert_eq!(mention_at("@src/main.rs now", 3), Some((0, 12, "sr")));
        assert_eq!(mention_at("@a b", 3), None);
    }
}
