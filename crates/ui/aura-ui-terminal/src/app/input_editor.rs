//! Ctrl+G: write the input in the user's editor.
//!
//! The input goes to a temp file, the terminal opens it with the same
//! editor plumbing as permanent agents ([`App::take_editor_request`]),
//! and whatever the file holds when the editor exits becomes the input.

use super::{App, NotificationType};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// A temp file for the input, unique to this process and moment, created
/// so no other user can read what is being written.
fn create_input_file(text: &str) -> std::io::Result<PathBuf> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let path = std::env::temp_dir().join(format!("aura-prompt-{}-{nanos}.md", std::process::id()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&path)?;
    file.write_all(text.as_bytes())?;
    Ok(path)
}

impl App {
    /// Open the input in `$VISUAL`/`$EDITOR` (Ctrl+G).
    pub(super) fn edit_input_in_editor(&mut self) {
        match create_input_file(&self.input.text) {
            Ok(path) => {
                self.pending.input_editor_file = Some(path.clone());
                self.pending.editor_request = Some(path);
            }
            Err(e) => {
                self.notifications.current = Some((
                    format!("Cannot create a file for the editor: {e}"),
                    NotificationType::Error,
                ));
            }
        }
    }

    /// Finish a Ctrl+G edit if `path` is its file: the file's text
    /// replaces the input. Returns `false` for any other file.
    pub(super) fn input_editor_closed(
        &mut self,
        path: &Path,
        result: &std::io::Result<()>,
    ) -> bool {
        if self.pending.input_editor_file.as_deref() != Some(path) {
            return false;
        }
        self.pending.input_editor_file = None;
        let text = match result {
            Ok(()) => std::fs::read_to_string(path).map_err(|e| e.to_string()),
            Err(e) => Err(e.to_string()),
        };
        match text {
            Ok(text) => {
                // Editors end the file with a newline the user didn't type.
                self.input.text = text.trim_end_matches(['\n', '\r']).to_string();
                self.input.cursor_pos = self.input.text.len();
                self.input.reset_slash_menu();
                self.sync_mentions();
            }
            Err(e) => {
                self.notifications.current = Some((
                    format!("Kept the input; the editor failed: {e}"),
                    NotificationType::Error,
                ));
            }
        }
        if let Err(e) = std::fs::remove_file(path) {
            tracing::debug!(path = %path.display(), error = %e, "Could not remove the input file");
        }
        true
    }
}
