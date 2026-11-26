//! `@` mentions: a fuzzy file and folder picker shown above the input
//! while an `@path` word is typed. In the chat of a channel's coordinator
//! the channel's members come first, to be mentioned by code.
//!
//! The picker only completes text. The surface reads the mentioned files
//! when the message is sent, so the chat keeps the short `@path`; a
//! `@CODE` names a member, matched without regard to case.

use super::{App, AppState};
use crate::events::AgentSummary;
use crate::terminal::KeyResult;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use gitignore::GitIgnore;
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

/// A file, folder or channel member the picker can insert.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MentionCandidate {
    /// What goes after the `@`: a path relative to the workspace root
    /// with `/` separators, folders ending in `/`, or a member's code
    pub path: String,
    /// Whether it is a folder
    pub is_dir: bool,
    /// Set when it is a member of the channel
    pub member: Option<MentionMember>,
}

/// A member of a channel as the picker lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MentionMember {
    /// Display name
    pub name: String,
    /// `coordinator`, `agent` or `subagent`
    pub role: &'static str,
}

/// The members of a channel coordinated by `coordinator_id` whose code or
/// name starts with `query`, ignoring case, in the channel's order. A
/// member without a code cannot be mentioned, so is left out.
fn member_matches(
    members: &[AgentSummary],
    coordinator_id: &str,
    query: &str,
) -> Vec<MentionCandidate> {
    let query = query.to_lowercase();
    members
        .iter()
        .filter_map(|member| {
            let code = member.code.as_deref().filter(|code| !code.is_empty())?;
            let matches = code.to_lowercase().starts_with(&query)
                || member.name.to_lowercase().starts_with(&query);
            matches.then(|| MentionCandidate {
                path: code.to_string(),
                is_dir: false,
                member: Some(MentionMember {
                    name: member.name.clone(),
                    role: member_role(member, coordinator_id),
                }),
            })
        })
        .collect()
}

/// What `member` is in a channel coordinated by `coordinator_id`: its
/// agents sit beside the coordinator at depth 0, subagents below them.
pub(in crate::app) fn member_role(member: &AgentSummary, coordinator_id: &str) -> &'static str {
    if member.id == coordinator_id {
        "coordinator"
    } else if member.depth == 0 {
        "agent"
    } else {
        "subagent"
    }
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
                entries.push(MentionCandidate {
                    path,
                    is_dir,
                    member: None,
                });
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

impl MentionMenu {
    /// Whether the matches are already ranked for `query` at `start`.
    fn is_ranked(&self, start: usize, query: &str) -> bool {
        self.query
            .as_ref()
            .is_some_and(|(s, q)| *s == start && q == query)
    }

    /// Files and folders under `root` matching `query`, best first.
    fn file_matches(&mut self, root: &Path, query: &str) -> Vec<MentionCandidate> {
        // A new `@` word walks the tree again so files created since
        // show up; typing within one word reuses the walk.
        let opening = self.query.is_none();
        let stale = self
            .index
            .as_ref()
            .is_none_or(|index| opening || !index.is_fresh_for(root));
        if stale {
            self.index = Some(MentionIndex::build(root));
        }
        self.index
            .as_ref()
            .map(|index| index.rank(query))
            .unwrap_or_default()
    }
}

impl App {
    /// Close the `@` picker, as when another agent's draft comes into the
    /// input; the next key opens it again on an `@word` there.
    pub(super) fn close_mentions(&mut self) {
        self.input.mentions.query = None;
        self.input.mentions.matches.clear();
        self.input.mentions.cursor = 0;
        self.input.mentions.dismissed = None;
    }

    /// Files and folders matching the `@word` at the cursor. Empty while
    /// the picker is closed, and while an approval prompt or another modal
    /// state has the keyboard: those can start without a key press (a tool
    /// call asking), so the picker is not re-synced before they show.
    #[must_use]
    pub fn mention_matches(&self) -> &[MentionCandidate] {
        let typing = matches!(self.state, AppState::Idle | AppState::Processing);
        if typing && self.input.mentions.query.is_some() {
            &self.input.mentions.matches
        } else {
            &[]
        }
    }

    /// Highlighted row in the `@` picker.
    #[must_use]
    pub fn mention_cursor(&self) -> usize {
        self.input
            .mentions
            .cursor
            .min(self.input.mentions.matches.len().saturating_sub(1))
    }

    /// Bring the `@` picker in line with the input: open it on an `@word`
    /// at the cursor, re-rank when the word changed, close it otherwise.
    pub(super) fn sync_mentions(&mut self) {
        let active = matches!(self.state, AppState::Idle | AppState::Processing)
            && self.detail.view.is_none();
        let word = active
            .then(|| mention_at(&self.input.text, self.input.cursor_pos))
            .flatten();
        // Esc closes the picker for one word; leaving that word forgets it.
        if word.map(|(start, _, _)| start) != self.input.mentions.dismissed {
            self.input.mentions.dismissed = None;
        }
        let menu = &mut self.input.mentions;
        let word = word.filter(|(start, _, _)| menu.dismissed != Some(*start));
        let (coordinator_id, members) = self
            .channels
            .coordinated_by(&self.active_agent_id)
            .map_or(("", &[][..]), |channel| {
                (channel.coordinator_id.as_str(), channel.members.as_slice())
            });
        // A cloud agent's files are on its machine, not here.
        let root = self.sandbox.root().filter(|_| !self.sandbox.is_remote());
        let Some((start, _, query)) = word.filter(|_| root.is_some() || !members.is_empty()) else {
            menu.query = None;
            menu.matches.clear();
            return;
        };
        if menu.is_ranked(start, query) {
            return;
        }
        let mut matches = member_matches(members, coordinator_id, query);
        if let Some(root) = root {
            matches.extend(menu.file_matches(root, query));
        }
        menu.matches = matches;
        menu.query = Some((start, query.to_string()));
        menu.cursor = 0;
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
                self.input.mentions.cursor = self.mention_cursor().checked_sub(1).unwrap_or(last);
            }
            KeyCode::Down => {
                self.input.mentions.cursor = if self.mention_cursor() >= last {
                    0
                } else {
                    self.mention_cursor() + 1
                };
            }
            // Tab on a folder stays in the picker to go into it.
            KeyCode::Tab => self.insert_mention(&selected, !selected.is_dir),
            KeyCode::Enter => self.insert_mention(&selected, true),
            KeyCode::Esc => {
                self.input.mentions.dismissed =
                    self.input.mentions.query.as_ref().map(|(start, _)| *start);
                self.input.mentions.query = None;
            }
            _ => return None,
        }
        Some(KeyResult::continue_running())
    }

    /// Replace the `@word` at the cursor with `@path`, followed by a blank
    /// when `finish` is set.
    fn insert_mention(&mut self, candidate: &MentionCandidate, finish: bool) {
        let Some((start, end, _)) = mention_at(&self.input.text, self.input.cursor_pos) else {
            return;
        };
        let mut text = format!("@{}", candidate.path);
        let next_is_blank = self.input.text[end..].starts_with(char::is_whitespace);
        if finish && !next_is_blank {
            text.push(' ');
        }
        self.input.text.replace_range(start..end, &text);
        self.input.cursor_pos = start + text.len() + usize::from(finish && next_is_blank);
        if finish {
            self.input.mentions.query = None;
        }
    }
}

mod gitignore;
#[cfg(test)]
mod tests;
