//! A run's diff split per file for the diff view.

use aura_ui::events::FileDiffData;

/// `patch` (a `git diff`) as one entry per file, each with its hunks.
pub(crate) fn split_patch(patch: &str) -> Vec<FileDiffData> {
    let mut files: Vec<FileDiffData> = Vec::new();
    let mut in_hunks = false;
    for line in patch.lines() {
        if let Some(header) = line.strip_prefix("diff --git ") {
            let path = header
                .rsplit_once(" b/")
                .map_or(header, |(_, path)| path)
                .to_string();
            files.push(FileDiffData {
                path,
                diff: String::new(),
            });
            in_hunks = false;
            continue;
        }
        let Some(file) = files.last_mut() else {
            continue;
        };
        if line.starts_with("@@") {
            in_hunks = true;
        }
        if line.starts_with("Binary files") || line == "GIT binary patch" {
            file.diff = "(binary file)".to_string();
            in_hunks = false;
            continue;
        }
        if in_hunks {
            file.diff.push_str(line);
            file.diff.push('\n');
        }
    }
    for file in &mut files {
        if file.diff.is_empty() {
            file.diff = "(mode change or empty file)".to_string();
        }
    }
    files
}
