//! Files the terminal UI hands to an editor outside the window.

use super::Effect;
use crate::tab::{Tab, Watch};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

/// How often a file opened in an external editor is checked for a save.
const POLL: Duration = Duration::from_secs(1);

/// Hand a file to the user's editor for `tab`. Editors that need a
/// terminal can't run in a window, so those (and no editor at all) fall
/// back to the system's default app, and saving the file counts as
/// closing it.
pub fn open(tab: &mut Tab, path: PathBuf, now: Instant) -> Option<Effect> {
    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .ok()
        .filter(|e| !e.trim().is_empty() && !is_terminal_editor(e));
    let Some(editor) = editor else {
        open_external(&path.display().to_string());
        tab.watch = Some(Watch {
            modified: modified(&path),
            path,
            checked: now,
        });
        return None;
    };
    Some(Effect::Edit {
        tab: tab.id,
        editor,
        path,
    })
}

/// Tell `tab`'s UI the file it is waiting on was saved, once it was.
pub fn poll_watch(tab: &mut Tab, now: Instant) {
    let Some(watch) = tab.watch.as_mut() else {
        return;
    };
    if now.saturating_duration_since(watch.checked) < POLL {
        return;
    }
    watch.checked = now;
    if modified(&watch.path) != watch.modified {
        if let Some(watch) = tab.watch.take() {
            tab.tui.editor_closed(watch.path, &Ok(()));
        }
    }
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Editors that need a terminal of their own, which a desktop window
/// doesn't have.
fn is_terminal_editor(editor: &str) -> bool {
    let program = editor.split_whitespace().next().unwrap_or_default();
    let name = Path::new(program)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(program);
    matches!(
        name,
        "vi" | "vim"
            | "nvim"
            | "nano"
            | "pico"
            | "micro"
            | "hx"
            | "helix"
            | "kak"
            | "ed"
            | "joe"
            | "mg"
    ) || (name == "emacs" && editor.contains("-nw"))
}

/// Open a URL or file with the platform's default handler.
fn open_external(target: &str) {
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open").arg(target).spawn();
    #[cfg(target_os = "windows")]
    let result = std::process::Command::new("cmd")
        .args(["/C", "start", ""])
        .arg(target)
        .spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let result = std::process::Command::new("xdg-open").arg(target).spawn();
    if let Err(e) = result {
        tracing::warn!(error = %e, target, "could not open");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_editors_are_recognised() {
        assert!(is_terminal_editor("nvim"));
        assert!(is_terminal_editor("/usr/bin/vim -u NONE"));
        assert!(is_terminal_editor("emacs -nw"));
        assert!(!is_terminal_editor("code --wait"));
        assert!(!is_terminal_editor("emacs"));
    }
}
