//! The files one attempt changed.

use super::*;

impl AttemptFiles {
    /// The files, in the order they were first changed.
    pub fn paths(&self) -> impl Iterator<Item = &Path> {
        self.0.iter().map(|(path, _)| path.as_path())
    }

    /// Whether the attempt changed no file.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// How the files differ now from before the attempt, as one unified
    /// diff per file under its path (relative to `workspace` when inside
    /// it). Files back in their old state are left out; empty when
    /// nothing differs.
    #[must_use]
    pub fn diff(&self, workspace: Option<&Path>) -> String {
        let workspace = workspace.map(|w| w.canonicalize().unwrap_or_else(|_| w.to_path_buf()));
        let mut out = String::new();
        for (path, before) in &self.0 {
            let Ok(now) = read_optional(path) else {
                continue;
            };
            if *before == now {
                continue;
            }
            let diff = unified_diff(before.as_deref(), now.as_deref())
                .unwrap_or_else(|| "(binary or very large file changed; no diff)\n".to_string());
            out.push_str("=== ");
            out.push_str(&display_path(path, workspace.as_deref()));
            out.push('\n');
            out.push_str(&diff);
            if !diff.ends_with('\n') {
                out.push('\n');
            }
        }
        out
    }

    /// Put every file back as it was before the attempt: written back, or
    /// removed if it didn't exist.
    #[must_use]
    pub fn restore(&self) -> RestoreReport {
        let mut report = RestoreReport::default();
        for (path, before) in &self.0 {
            let restored = match before {
                Some(bytes) => write_file(path, bytes),
                None => match std::fs::remove_file(path) {
                    Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
                    _ => Ok(()),
                },
            };
            match restored {
                Ok(()) => report.restored.push(path.clone()),
                Err(e) => report.failed.push((path.clone(), e.to_string())),
            }
        }
        report
    }
}
