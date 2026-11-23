//! A session replayed as a comparison's work: the prompts it replays,
//! the title it is shown under, and where its files sit in the
//! workspace.

#[cfg(test)]
mod tests;

use aura_model::{ContentBlock, Message, Role};
use std::path::{Path, PathBuf};

/// Longest session title, in characters.
const TITLE_CHARS: usize = 60;

/// What the user typed in `messages`, in order: every user message with
/// text that isn't tool results, or only those `is_named` accepts when it
/// accepts any.
#[must_use]
pub fn prompts(messages: &[Message], is_named: impl Fn(&str) -> bool) -> Vec<String> {
    let (named, others): (Vec<String>, Vec<String>) = messages
        .iter()
        .filter(|m| {
            m.role == Role::User
                && !m
                    .content
                    .iter()
                    .any(|block| matches!(block, ContentBlock::ToolResult { .. }))
        })
        .map(Message::text_content)
        .filter(|text| !text.trim().is_empty())
        .partition(|text| is_named(text));
    if named.is_empty() {
        others
    } else {
        named
    }
}

/// A session's title: the first line of its first prompt, cut to
/// `TITLE_CHARS` characters.
#[must_use]
pub fn title(prompt: &str) -> String {
    let line = prompt.lines().next().unwrap_or_default().trim();
    let mut chars = line.chars();
    let mut title: String = chars.by_ref().take(TITLE_CHARS).collect();
    if chars.next().is_some() {
        title.push('…');
    }
    title
}

/// `path` relative to `workspace`, when inside it as given or once the
/// symlinks of its folder are resolved.
#[must_use]
pub fn relative(path: &Path, workspace: &Path) -> Option<PathBuf> {
    if path.is_relative() {
        return Some(path.to_path_buf());
    }
    if let Ok(inside) = path.strip_prefix(workspace) {
        return Some(inside.to_path_buf());
    }
    let canonical = path
        .parent()
        .and_then(|parent| parent.canonicalize().ok())
        .zip(path.file_name())
        .map(|(parent, name)| parent.join(name))?;
    canonical
        .strip_prefix(workspace)
        .ok()
        .map(Path::to_path_buf)
}
