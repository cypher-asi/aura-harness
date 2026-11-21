//! Read-only preview of a sandbox file, shown over the chat panel.

use super::vim::{Outcome, Screen, Vim};
use crate::components::syntax::{syntax_for_path, syntax_for_token, HighlightCache};
use crossterm::event::KeyEvent;
use std::io::Read;
use std::path::{Path, PathBuf};
use syntect::parsing::SyntaxReference;

/// Largest file shown in full; bigger files show only a note.
const MAX_PREVIEW_BYTES: u64 = 1024 * 1024;

/// Bytes checked for NUL when deciding whether a file is binary.
const BINARY_SNIFF_BYTES: usize = 8 * 1024;

/// What the preview shows for a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileBody {
    /// UTF-8 text.
    Text(String),
    /// Why the content is not shown (binary, too large, unreadable).
    Note(String),
}

/// A sandbox file opened for viewing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileView {
    /// Absolute path of the file.
    pub path: PathBuf,
    /// Path shown in the title, relative to the sandbox root when inside it.
    pub title: String,
    /// File content or a note explaining why it is not shown.
    pub body: FileBody,
    /// Cursor and vim normal-mode state.
    pub vim: Vim,
    /// Number shown in the gutter for the first line (1 for whole files,
    /// the start line for a slice a tool read).
    pub first_line: usize,
    /// Highlighting to use instead of the one the path's extension picks.
    pub language: Option<String>,
    /// Highlighted lines computed so far; resets itself when the body or
    /// syntax it was built for changes.
    pub(crate) highlight: HighlightCache,
}

impl FileView {
    /// Read `path` for viewing, titling it relative to `root`.
    #[must_use]
    pub fn load(path: &Path, root: Option<&Path>) -> Self {
        let title = root
            .and_then(|root| path.strip_prefix(root).ok())
            .filter(|rel| !rel.as_os_str().is_empty())
            .unwrap_or(path)
            .display()
            .to_string();
        Self {
            path: path.to_path_buf(),
            title,
            body: read_body(path),
            vim: Vim::default(),
            first_line: 1,
            language: None,
            highlight: HighlightCache::default(),
        }
    }

    /// Text that is not read from disk (a tool's output), titled `title`
    /// and highlighted as `language`, numbered from `first_line`.
    #[must_use]
    pub fn snapshot(title: &str, text: &str, language: &str, first_line: usize) -> Self {
        Self {
            path: PathBuf::from(title),
            title: title.to_string(),
            body: FileBody::Text(text.to_string()),
            vim: Vim::default(),
            first_line: first_line.max(1),
            language: Some(language.to_string()),
            highlight: HighlightCache::default(),
        }
    }

    /// Put the cursor on 0-based `line` and show it at the top.
    pub fn jump_to(&mut self, line: usize) {
        let lines = self.body.lines();
        self.vim.jump_to(line, &lines);
    }

    /// Apply a vim key to the cursor and the view's `scroll`.
    pub fn handle_key(&mut self, key: KeyEvent, scroll: &mut usize, screen: Screen) -> Outcome {
        let lines = self.body.lines();
        self.vim.handle_key(key, &lines, scroll, screen)
    }

    /// Lowercased file extension, used to pick syntax highlighting.
    #[must_use]
    pub fn extension(&self) -> String {
        if let Some(language) = &self.language {
            return language.clone();
        }
        self.path
            .extension()
            .map(|ext| ext.to_string_lossy().to_lowercase())
            .unwrap_or_default()
    }

    /// The syntax the body is highlighted as: the explicit language when
    /// it names one, else what the file name, extension or first line
    /// suggests (plain text when nothing matches).
    pub(crate) fn syntax(&self) -> &'static SyntaxReference {
        if let Some(syntax) = self.language.as_deref().and_then(syntax_for_token) {
            return syntax;
        }
        let first_line = match &self.body {
            FileBody::Text(text) => text.lines().next().unwrap_or_default(),
            FileBody::Note(_) => "",
        };
        syntax_for_path(&self.path, first_line)
    }
}

impl FileBody {
    /// Lines the cursor moves over; a note has none.
    #[must_use]
    pub fn lines(&self) -> Vec<&str> {
        match self {
            Self::Text(text) => text.lines().collect(),
            Self::Note(_) => Vec::new(),
        }
    }
}

fn read_body(path: &Path) -> FileBody {
    let size = match std::fs::metadata(path) {
        Ok(meta) => meta.len(),
        Err(e) => return FileBody::Note(format!("Cannot read file: {e}")),
    };
    if size > MAX_PREVIEW_BYTES {
        return FileBody::Note(format!("Too large to preview ({size} bytes)"));
    }
    let mut bytes = Vec::new();
    if let Err(e) = std::fs::File::open(path).and_then(|mut f| f.read_to_end(&mut bytes)) {
        return FileBody::Note(format!("Cannot read file: {e}"));
    }
    if bytes[..bytes.len().min(BINARY_SNIFF_BYTES)].contains(&0) {
        return FileBody::Note(format!("Binary file ({} bytes)", bytes.len()));
    }
    match String::from_utf8(bytes) {
        Ok(text) => FileBody::Text(text),
        Err(e) => FileBody::Note(format!("Binary file ({} bytes)", e.as_bytes().len())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn text_file_is_titled_relative_to_root() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("src")).unwrap();
        let path = dir.path().join("src/main.rs");
        fs::write(&path, "fn main() {}\n").unwrap();

        let view = FileView::load(&path, Some(dir.path()));
        assert_eq!(
            view.title,
            format!("src{}main.rs", std::path::MAIN_SEPARATOR)
        );
        assert_eq!(view.body, FileBody::Text("fn main() {}\n".to_string()));
        assert_eq!(view.extension(), "rs");
        assert_eq!(view.syntax().name, "Rust");
    }

    #[test]
    fn syntax_prefers_the_language_then_the_path() {
        assert_eq!(
            FileView::snapshot("x.txt", "{}", "json", 1).syntax().name,
            "JSON"
        );
        let read = FileView::snapshot("docker/Dockerfile", "FROM x", "", 1);
        assert_eq!(read.syntax().name, "Dockerfile");
        let script = FileView::snapshot("run", "#!/usr/bin/env python3", "", 1);
        assert_eq!(script.syntax().name, "Python");
    }

    #[test]
    fn binary_file_shows_note() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("blob.bin");
        fs::write(&path, [0x7f, b'E', b'L', b'F', 0, 1, 2]).unwrap();

        let view = FileView::load(&path, Some(dir.path()));
        assert_eq!(
            view.body,
            FileBody::Note("Binary file (7 bytes)".to_string())
        );
    }

    #[test]
    fn oversized_file_shows_note() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.txt");
        let file = fs::File::create(&path).unwrap();
        file.set_len(MAX_PREVIEW_BYTES + 1).unwrap();

        let view = FileView::load(&path, Some(dir.path()));
        assert!(matches!(view.body, FileBody::Note(note) if note.starts_with("Too large")));
    }

    #[test]
    fn missing_file_shows_note() {
        let dir = tempfile::tempdir().unwrap();
        let view = FileView::load(&dir.path().join("gone.txt"), Some(dir.path()));
        assert!(matches!(view.body, FileBody::Note(note) if note.starts_with("Cannot read")));
    }
}
