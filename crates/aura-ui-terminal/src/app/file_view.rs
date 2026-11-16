//! Read-only preview of a sandbox file, shown over the chat panel.

use super::vim::{Outcome, Screen, Vim};
use crossterm::event::KeyEvent;
use std::io::Read;
use std::path::{Path, PathBuf};

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
        }
    }

    /// Apply a vim key to the cursor and the view's `scroll`.
    pub fn handle_key(&mut self, key: KeyEvent, scroll: &mut usize, screen: Screen) -> Outcome {
        let lines = self.body.lines();
        self.vim.handle_key(key, &lines, scroll, screen)
    }

    /// Lowercased file extension, used to pick syntax highlighting.
    #[must_use]
    pub fn extension(&self) -> String {
        self.path
            .extension()
            .map(|ext| ext.to_string_lossy().to_lowercase())
            .unwrap_or_default()
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
