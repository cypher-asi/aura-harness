//! What an attempt left behind, as Gate's attempt log keeps it.

use serde::{Deserialize, Serialize};

/// Paths kept of the files an attempt changed.
pub(super) const KEPT_PATHS: usize = 40;

/// A check the attempt ran to its end.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckEvidence {
    /// Its command line
    pub command: String,
    /// Whether the command exited with success, which is what Gate counts
    /// as the check passing
    pub passed: bool,
    /// The end of what it printed
    pub output: String,
}

/// What one attempt left behind.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Evidence {
    /// Model calls of the attempt
    pub calls: u32,
    /// Commands it ran
    pub commands: u32,
    /// Checks it ran to their end
    pub checks_run: u32,
    /// Times Gate asked it to check a change it had not checked
    #[serde(default, skip_serializing_if = "is_zero")]
    pub nudges: u32,
    /// The last of those checks, latest last
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checks: Vec<CheckEvidence>,
    /// Files its file tools changed. A file changed by a command it ran
    /// is not among them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files_edited: Vec<String>,
    /// Those of [`Self::files_edited`] its file tools first changed by
    /// writing them whole, as a new file is written
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files_written: Vec<String>,
    /// Files it changed, when the harness counted them
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files_changed: Option<u32>,
    /// The files it changed by any means, relative to the workspace,
    /// when the harness compared the workspace before and after: those
    /// changed by commands it ran are among them
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changed_paths: Option<Vec<String>>,
    /// Whether it changed a file after the last check it ran to its end,
    /// as far as its tool calls show
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub changed_after_check: bool,
    /// Whether it stopped on a limit rather than by finishing, when known
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hit_limit: Option<bool>,
    /// The start of its final reply
    pub reply: String,
}

#[allow(clippy::trivially_copy_pass_by_ref)]
const fn is_zero(n: &u32) -> bool {
    *n == 0
}

impl Evidence {
    /// Note the files the attempt changed by any means, as the harness
    /// found by comparing the workspace before and after.
    pub fn note_changed(&mut self, paths: &[String]) {
        self.files_changed = Some(u32::try_from(paths.len()).unwrap_or(u32::MAX));
        self.changed_paths = Some(paths.iter().take(KEPT_PATHS).cloned().collect());
    }

    /// The changed files its file tools did not change: `None` when the
    /// harness did not compare the workspace, or when more files were
    /// changed than are kept.
    pub(super) fn changed_by_commands(&self) -> Option<Vec<&str>> {
        let changed = self.changed_paths.as_ref()?;
        if changed.len() >= KEPT_PATHS || self.files_edited.len() >= KEPT_PATHS {
            return None;
        }
        Some(
            changed
                .iter()
                .filter(|path| {
                    !self
                        .files_edited
                        .iter()
                        .any(|edited| same_file(edited, path))
                })
                .map(String::as_str)
                .collect(),
        )
    }

    /// The paths known of what it changed: every changed file when the
    /// harness compared the workspace, else those its file tools changed.
    pub(super) fn known_paths(&self) -> &[String] {
        self.changed_paths.as_deref().unwrap_or(&self.files_edited)
    }

    /// The last check the attempt ran to its end.
    #[must_use]
    pub fn last_check(&self) -> Option<&CheckEvidence> {
        self.checks.last()
    }
}

/// Whether `a` and `b` name one file: equal, or one the other's path
/// from further up (an absolute path and one relative to the workspace).
pub(super) fn same_file(a: &str, b: &str) -> bool {
    let (a, b) = (a.trim_start_matches("./"), b.trim_start_matches("./"));
    let under = |long: &str, short: &str| {
        long.strip_suffix(short)
            .is_some_and(|rest| rest.is_empty() || rest.ends_with('/'))
    };
    under(a, b) || under(b, a)
}
