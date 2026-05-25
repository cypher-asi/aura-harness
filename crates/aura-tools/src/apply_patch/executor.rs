//! Validate-all-then-apply-all executor for [`super::Patch`].
//!
//! The executor is split into two phases so a multi-file patch is
//! atomic-ish: if *any* hunk fails to apply (context mismatch, missing
//! target, etc.), no files are mutated.
//!
//! Phase 1 — validate: read each target file, anchor every Update hunk
//! against its current contents, build the expected post-patch
//! content. Add/Delete preconditions are checked here too.
//!
//! Phase 2 — apply: only entered if phase 1 succeeded. Writes the
//! pre-computed content for each Add/Update and removes the target
//! for each Delete.

use std::collections::HashMap;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use super::parser::{FileChange, Hunk, HunkLine, Patch, PatchError};

/// One file mutation that landed on disk during [`execute_apply_patch`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppliedChange {
    pub path: String,
    pub kind: AppliedChangeKind,
    /// Lines added by this change (relative to the prior on-disk
    /// content). Always 0 for `Deleted`.
    pub lines_added: u32,
    /// Lines removed by this change. Always 0 for `Added`.
    pub lines_removed: u32,
}

/// Whether the change created, modified, or deleted the target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppliedChangeKind {
    Added,
    Updated,
    Deleted,
}

/// Successful outcome of [`execute_apply_patch`].
#[derive(Debug, Clone)]
pub struct ApplyPatchResult {
    pub changes: Vec<AppliedChange>,
    /// One-line human-readable summary, e.g. "Applied: 2 added, 1
    /// updated, 0 deleted (3 files)".
    pub summary: String,
}

/// Failure modes for [`execute_apply_patch`].
#[derive(Debug)]
pub enum ApplyPatchError {
    /// The patch failed to parse.
    Parse(PatchError),
    /// `*** Add File:` targeted a path that already exists on disk.
    TargetAlreadyExists { path: String },
    /// `*** Update File:` / `*** Delete File:` targeted a missing path.
    TargetNotFound { path: String },
    /// Patch path resolved outside the workspace root after
    /// canonicalization.
    PathEscape { path: String },
    /// An Update hunk could not be anchored against the target file's
    /// current contents.
    ContextMismatch {
        path: String,
        hunk_index: usize,
        reason: String,
    },
    /// Two top-level directives touched the same path in a way that
    /// can't be reconciled atomically (e.g. Add then Update of the
    /// same path within one patch).
    ConflictingChanges { path: String, reason: String },
    /// Filesystem error during validation or apply.
    Io { path: String, source: io::Error },
}

impl fmt::Display for ApplyPatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Parse(e) => write!(f, "patch parse failed: {e}"),
            Self::TargetAlreadyExists { path } => write!(
                f,
                "*** Add File: {path}: target already exists; use `*** Update File:` instead"
            ),
            Self::TargetNotFound { path } => write!(f, "target file not found: {path}"),
            Self::PathEscape { path } => write!(
                f,
                "path {path:?} resolves outside the workspace root"
            ),
            Self::ContextMismatch {
                path,
                hunk_index,
                reason,
            } => write!(
                f,
                "*** Update File: {path}: hunk #{} failed context match: {reason}",
                hunk_index + 1
            ),
            Self::ConflictingChanges { path, reason } => {
                write!(f, "conflicting changes for {path}: {reason}")
            }
            Self::Io { path, source } => write!(f, "io error on {path}: {source}"),
        }
    }
}

impl std::error::Error for ApplyPatchError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Parse(e) => Some(e),
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl From<PatchError> for ApplyPatchError {
    fn from(err: PatchError) -> Self {
        Self::Parse(err)
    }
}

/// Apply [`Patch`] to the filesystem rooted at `workspace_root`.
///
/// Validates every directive first; only commits writes if validation
/// passes. Returns the list of changes that landed on disk plus a
/// short summary string.
///
/// # Errors
///
/// Any [`ApplyPatchError`] variant. On error, the filesystem is left
/// unmutated.
pub async fn execute_apply_patch(
    patch: Patch,
    workspace_root: &Path,
) -> Result<ApplyPatchResult, ApplyPatchError> {
    let plan = build_plan(&patch, workspace_root)?;
    commit_plan(plan).await
}

/// Internal: one staged change ready to commit in phase 2.
struct StagedChange {
    /// Path as it appeared in the patch (forward-slash, workspace-
    /// relative). Returned in the result for caller-side reporting.
    rel_path: String,
    /// Full absolute path on disk.
    abs_path: PathBuf,
    kind: AppliedChangeKind,
    /// Final content to write (None for Delete).
    new_content: Option<String>,
    lines_added: u32,
    lines_removed: u32,
}

/// Phase 1: validate every directive and stage the post-patch content
/// in memory. No disk writes happen here.
fn build_plan(
    patch: &Patch,
    workspace_root: &Path,
) -> Result<Vec<StagedChange>, ApplyPatchError> {
    let mut staged: Vec<StagedChange> = Vec::with_capacity(patch.changes.len());
    // Track conflicting changes within the same patch. A patch that
    // adds and then updates the same path is rejected; the model
    // should fold those into a single Add with the final content.
    let mut seen_paths: HashMap<String, AppliedChangeKind> = HashMap::new();

    for change in &patch.changes {
        match change {
            FileChange::Add { path, content } => {
                detect_conflict(&seen_paths, path, AppliedChangeKind::Added)?;
                let abs_path = resolve_workspace_path(workspace_root, path)?;
                if abs_path.exists() {
                    return Err(ApplyPatchError::TargetAlreadyExists {
                        path: path.clone(),
                    });
                }
                let lines_added = u32::try_from(count_lines(content)).unwrap_or(u32::MAX);
                seen_paths.insert(path.clone(), AppliedChangeKind::Added);
                staged.push(StagedChange {
                    rel_path: path.clone(),
                    abs_path,
                    kind: AppliedChangeKind::Added,
                    new_content: Some(content.clone()),
                    lines_added,
                    lines_removed: 0,
                });
            }
            FileChange::Update { path, hunks } => {
                detect_conflict(&seen_paths, path, AppliedChangeKind::Updated)?;
                let abs_path = resolve_workspace_path(workspace_root, path)?;
                if !abs_path.exists() {
                    return Err(ApplyPatchError::TargetNotFound { path: path.clone() });
                }
                let raw =
                    std::fs::read_to_string(&abs_path).map_err(|e| ApplyPatchError::Io {
                        path: path.clone(),
                        source: e,
                    })?;
                let had_crlf = raw.contains("\r\n");
                let original_lf = raw.replace("\r\n", "\n");
                let (new_lf, lines_added, lines_removed) =
                    apply_hunks_to_content(path, &original_lf, hunks)?;

                let to_write = if had_crlf {
                    new_lf.replace('\n', "\r\n")
                } else {
                    new_lf
                };
                seen_paths.insert(path.clone(), AppliedChangeKind::Updated);
                staged.push(StagedChange {
                    rel_path: path.clone(),
                    abs_path,
                    kind: AppliedChangeKind::Updated,
                    new_content: Some(to_write),
                    lines_added,
                    lines_removed,
                });
            }
            FileChange::Delete { path } => {
                detect_conflict(&seen_paths, path, AppliedChangeKind::Deleted)?;
                let abs_path = resolve_workspace_path(workspace_root, path)?;
                if !abs_path.exists() {
                    return Err(ApplyPatchError::TargetNotFound { path: path.clone() });
                }
                let lines_removed = std::fs::read_to_string(&abs_path)
                    .map(|s| u32::try_from(count_lines(&s)).unwrap_or(u32::MAX))
                    .unwrap_or(0);
                seen_paths.insert(path.clone(), AppliedChangeKind::Deleted);
                staged.push(StagedChange {
                    rel_path: path.clone(),
                    abs_path,
                    kind: AppliedChangeKind::Deleted,
                    new_content: None,
                    lines_added: 0,
                    lines_removed,
                });
            }
        }
    }

    Ok(staged)
}

/// Phase 2: write staged changes to disk in document order.
async fn commit_plan(
    plan: Vec<StagedChange>,
) -> Result<ApplyPatchResult, ApplyPatchError> {
    let mut applied = Vec::with_capacity(plan.len());
    let mut added = 0_usize;
    let mut updated = 0_usize;
    let mut deleted = 0_usize;

    for staged in plan {
        match staged.kind {
            AppliedChangeKind::Added | AppliedChangeKind::Updated => {
                if let Some(parent) = staged.abs_path.parent() {
                    if !parent.exists() {
                        tokio::fs::create_dir_all(parent).await.map_err(|e| {
                            ApplyPatchError::Io {
                                path: staged.rel_path.clone(),
                                source: e,
                            }
                        })?;
                    }
                }
                let content = staged.new_content.unwrap_or_default();
                tokio::fs::write(&staged.abs_path, &content)
                    .await
                    .map_err(|e| ApplyPatchError::Io {
                        path: staged.rel_path.clone(),
                        source: e,
                    })?;
                if matches!(staged.kind, AppliedChangeKind::Added) {
                    added += 1;
                } else {
                    updated += 1;
                }
            }
            AppliedChangeKind::Deleted => {
                tokio::fs::remove_file(&staged.abs_path)
                    .await
                    .map_err(|e| ApplyPatchError::Io {
                        path: staged.rel_path.clone(),
                        source: e,
                    })?;
                deleted += 1;
            }
        }
        applied.push(AppliedChange {
            path: staged.rel_path,
            kind: staged.kind,
            lines_added: staged.lines_added,
            lines_removed: staged.lines_removed,
        });
    }

    let summary = format!(
        "Applied: {added} added, {updated} updated, {deleted} deleted ({total} file{plural})",
        total = applied.len(),
        plural = if applied.len() == 1 { "" } else { "s" },
    );

    Ok(ApplyPatchResult {
        changes: applied,
        summary,
    })
}

/// Reject conflicting directives on the same path within one patch.
fn detect_conflict(
    seen: &HashMap<String, AppliedChangeKind>,
    path: &str,
    next: AppliedChangeKind,
) -> Result<(), ApplyPatchError> {
    if let Some(prior) = seen.get(path) {
        return Err(ApplyPatchError::ConflictingChanges {
            path: path.to_string(),
            reason: format!(
                "path was already targeted by {prior:?} earlier in the patch; cannot also \
                 apply {next:?}",
            ),
        });
    }
    Ok(())
}

/// Resolve a workspace-relative path against the root, refusing to
/// produce anything that escapes the root.
fn resolve_workspace_path(root: &Path, rel: &str) -> Result<PathBuf, ApplyPatchError> {
    let joined = root.join(rel);
    let lexical = lexical_normalize(&joined);
    let lexical_root = lexical_normalize(root);
    if !lexical.starts_with(&lexical_root) {
        return Err(ApplyPatchError::PathEscape {
            path: rel.to_string(),
        });
    }
    Ok(lexical)
}

/// Resolve `.` and `..` components lexically (no FS calls). Mirrors the
/// approach used in `aura_agent::file_ops::lexical_normalize` so we
/// avoid `canonicalize`'s Windows `\\?\` prefix surprises.
fn lexical_normalize(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

/// Count lines in a string the same way `str::lines` does, but without
/// allocating. A trailing newline is not counted as a separate line
/// (e.g. "a\nb\n" -> 2 lines).
fn count_lines(s: &str) -> usize {
    if s.is_empty() {
        return 0;
    }
    let mut count = 0_usize;
    let mut started = false;
    for ch in s.chars() {
        if !started {
            started = true;
            count += 1;
        }
        if ch == '\n' {
            // The next char (if any) starts a new line; flip `started`
            // back to false so we count it.
            started = false;
        }
    }
    count
}

// ---------------------------------------------------------------------------
// Hunk application (anchor matching)
// ---------------------------------------------------------------------------

/// Apply every hunk in `hunks` to `content` (already LF-normalized).
/// Returns the post-patch content, total lines added, and total lines
/// removed across all hunks.
fn apply_hunks_to_content(
    path: &str,
    content: &str,
    hunks: &[Hunk],
) -> Result<(String, u32, u32), ApplyPatchError> {
    let original: Vec<&str> = content.split('\n').collect();
    // After applying hunk N we keep working on the post-patch buffer
    // so successive hunks see each other's edits.
    let mut current: Vec<String> = original.iter().map(|s| (*s).to_string()).collect();
    let mut total_added = 0_u32;
    let mut total_removed = 0_u32;
    let mut anchor_floor = 0_usize;

    for (idx, hunk) in hunks.iter().enumerate() {
        let (new_buf, added, removed, anchor_end) =
            apply_one_hunk(path, idx, hunk, &current, anchor_floor)?;
        current = new_buf;
        total_added = total_added.saturating_add(added);
        total_removed = total_removed.saturating_add(removed);
        anchor_floor = anchor_end;
    }

    let mut joined = current.join("\n");
    // If the original ended with a trailing newline we preserve it; we
    // recreated `original` by splitting on `\n` which produces an
    // empty trailing element exactly when the input ended with `\n`.
    let trailing_nl = content.ends_with('\n');
    if trailing_nl && !joined.ends_with('\n') {
        joined.push('\n');
    }
    Ok((joined, total_added, total_removed))
}

/// Apply one hunk to `lines`. Returns the new buffer, added/removed
/// counts, and the post-anchor index (for "prefer nearest" picking on
/// the next hunk).
#[allow(clippy::type_complexity)]
fn apply_one_hunk(
    path: &str,
    hunk_index: usize,
    hunk: &Hunk,
    lines: &[String],
    anchor_floor: usize,
) -> Result<(Vec<String>, u32, u32, usize), ApplyPatchError> {
    // Pull the anchor text: every context + removal line, in order.
    // These are the lines that must exist contiguously in `lines`.
    let anchor: Vec<&str> = hunk
        .lines
        .iter()
        .filter_map(|hl| match hl {
            HunkLine::Context(c) | HunkLine::Removal(c) => Some(c.as_str()),
            HunkLine::Addition(_) => None,
        })
        .collect();

    let match_start = if anchor.is_empty() {
        // Pure-addition hunk with no anchor lines: append at end. This
        // is a niche case (typical for empty Update bodies, which we
        // already reject upstream) but we handle it defensively.
        lines.len()
    } else {
        find_anchor_match(&anchor, lines, anchor_floor).ok_or_else(|| {
            ApplyPatchError::ContextMismatch {
                path: path.to_string(),
                hunk_index,
                reason: build_mismatch_diagnostic(&anchor, lines),
            }
        })?
    };

    // Now walk the hunk body, advancing through `lines` starting at
    // `match_start`, and assemble the new buffer.
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    out.extend(lines[..match_start].iter().cloned());
    let mut cursor = match_start;
    let mut added = 0_u32;
    let mut removed = 0_u32;

    for (i, hl) in hunk.lines.iter().enumerate() {
        match hl {
            HunkLine::Context(c) => {
                if cursor >= lines.len() || lines[cursor] != *c {
                    return Err(ApplyPatchError::ContextMismatch {
                        path: path.to_string(),
                        hunk_index,
                        reason: format!(
                            "context drift at line {i} of hunk: expected {c:?}, found {:?} at line {} of file",
                            lines.get(cursor).map(String::as_str).unwrap_or("<eof>"),
                            cursor + 1,
                        ),
                    });
                }
                out.push(c.clone());
                cursor += 1;
            }
            HunkLine::Removal(c) => {
                if cursor >= lines.len() || lines[cursor] != *c {
                    return Err(ApplyPatchError::ContextMismatch {
                        path: path.to_string(),
                        hunk_index,
                        reason: format!(
                            "removal at line {i} of hunk did not match file: expected {c:?}, found {:?} at line {} of file",
                            lines.get(cursor).map(String::as_str).unwrap_or("<eof>"),
                            cursor + 1,
                        ),
                    });
                }
                cursor += 1;
                removed = removed.saturating_add(1);
            }
            HunkLine::Addition(c) => {
                out.push(c.clone());
                added = added.saturating_add(1);
            }
        }
    }

    out.extend(lines[cursor..].iter().cloned());
    Ok((out, added, removed, cursor))
}

/// Find the start index in `lines` where the anchor block matches
/// contiguously. Honors `anchor_floor` so successive hunks naturally
/// scan past prior edits; falls back to the global search if no match
/// occurs at or after the floor.
fn find_anchor_match(
    anchor: &[&str],
    lines: &[String],
    anchor_floor: usize,
) -> Option<usize> {
    if anchor.is_empty() || anchor.len() > lines.len() {
        return None;
    }

    let mut nearest: Option<usize> = None;
    for start in 0..=lines.len() - anchor.len() {
        if anchor.iter().zip(&lines[start..]).all(|(a, b)| *a == *b) {
            if start >= anchor_floor {
                // First match at or past the floor wins (document
                // order ordering of successive hunks).
                return Some(start);
            }
            nearest = Some(start);
        }
    }
    nearest
}

/// Build a small diagnostic for a context mismatch. Shows the first
/// few anchor lines the hunk wanted to find so the model can see what
/// to correct.
fn build_mismatch_diagnostic(anchor: &[&str], lines: &[String]) -> String {
    let want_preview: Vec<String> = anchor
        .iter()
        .take(3)
        .map(|s| format!("  | {s}"))
        .collect();
    let mut diag = String::from(
        "no contiguous match for the hunk's context lines (first 3 shown):\n",
    );
    diag.push_str(&want_preview.join("\n"));
    if anchor.len() > 3 {
        diag.push_str(&format!("\n  | ... ({} more)", anchor.len() - 3));
    }
    if lines.len() < 25 {
        diag.push_str("\nfile content was short; re-read the target with read_file before re-emitting the patch.");
    } else {
        diag.push_str(&format!(
            "\ntarget file has {} lines; re-read the relevant section with read_file before re-emitting the patch.",
            lines.len()
        ));
    }
    diag
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apply_patch::parser::parse_patch;
    use tempfile::TempDir;

    fn write_file(root: &Path, rel: &str, content: &str) {
        let path = root.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, content).unwrap();
    }

    fn read_file(root: &Path, rel: &str) -> String {
        std::fs::read_to_string(root.join(rel)).unwrap()
    }

    #[tokio::test]
    async fn executor_adds_new_file() {
        let dir = TempDir::new().unwrap();
        let patch = parse_patch(
            "*** Begin Patch\n\
             *** Add File: src/new.rs\n\
             +pub fn one() {}\n\
             +pub fn two() {}\n\
             *** End Patch\n",
        )
        .unwrap();
        let result = execute_apply_patch(patch, dir.path()).await.unwrap();
        assert_eq!(result.changes.len(), 1);
        assert_eq!(result.changes[0].kind, AppliedChangeKind::Added);
        assert_eq!(
            read_file(dir.path(), "src/new.rs"),
            "pub fn one() {}\npub fn two() {}"
        );
    }

    #[tokio::test]
    async fn executor_updates_existing_file_with_unique_context() {
        let dir = TempDir::new().unwrap();
        write_file(
            dir.path(),
            "src/lib.rs",
            "pub fn old() {\n    let x = 1;\n    let y = 2;\n}\n",
        );
        let patch = parse_patch(concat!(
            "*** Begin Patch\n",
            "*** Update File: src/lib.rs\n",
            "@@ pub fn old @@\n",
            " pub fn old() {\n",
            "     let x = 1;\n",
            "-    let y = 2;\n",
            "+    let y = 22;\n",
            " }\n",
            "*** End Patch\n",
        ))
        .unwrap();

        let result = execute_apply_patch(patch, dir.path()).await.unwrap();
        assert_eq!(result.changes.len(), 1);
        assert_eq!(result.changes[0].kind, AppliedChangeKind::Updated);
        assert_eq!(result.changes[0].lines_added, 1);
        assert_eq!(result.changes[0].lines_removed, 1);
        assert_eq!(
            read_file(dir.path(), "src/lib.rs"),
            "pub fn old() {\n    let x = 1;\n    let y = 22;\n}\n"
        );
    }

    #[tokio::test]
    async fn executor_deletes_existing_file() {
        let dir = TempDir::new().unwrap();
        write_file(dir.path(), "src/old.rs", "doomed\n");
        let patch = parse_patch(
            "*** Begin Patch\n\
             *** Delete File: src/old.rs\n\
             *** End Patch\n",
        )
        .unwrap();
        let result = execute_apply_patch(patch, dir.path()).await.unwrap();
        assert_eq!(result.changes.len(), 1);
        assert_eq!(result.changes[0].kind, AppliedChangeKind::Deleted);
        assert!(!dir.path().join("src/old.rs").exists());
    }

    #[tokio::test]
    async fn executor_multi_file_atomic_commit() {
        let dir = TempDir::new().unwrap();
        write_file(dir.path(), "src/b.rs", "alpha\nbeta\n");
        write_file(dir.path(), "src/c.rs", "doomed\n");
        let patch = parse_patch(concat!(
            "*** Begin Patch\n",
            "*** Add File: src/a.rs\n",
            "+new content\n",
            "*** Update File: src/b.rs\n",
            "@@\n",
            " alpha\n",
            "-beta\n",
            "+bee\n",
            "*** Delete File: src/c.rs\n",
            "*** End Patch\n",
        ))
        .unwrap();
        let result = execute_apply_patch(patch, dir.path()).await.unwrap();
        assert_eq!(result.changes.len(), 3);
        assert_eq!(read_file(dir.path(), "src/a.rs"), "new content");
        assert_eq!(read_file(dir.path(), "src/b.rs"), "alpha\nbee\n");
        assert!(!dir.path().join("src/c.rs").exists());
    }

    #[tokio::test]
    async fn executor_atomic_rollback_on_context_mismatch() {
        let dir = TempDir::new().unwrap();
        write_file(dir.path(), "src/good.rs", "the\nquick\nbrown\nfox\n");
        write_file(dir.path(), "src/bad.rs", "totally\nunrelated\n");

        let patch = parse_patch(concat!(
            "*** Begin Patch\n",
            "*** Update File: src/good.rs\n",
            "@@\n",
            " the\n",
            "-quick\n",
            "+slow\n",
            "*** Update File: src/bad.rs\n",
            "@@\n",
            " this\n",
            "-will\n",
            "+never\n",
            " match\n",
            "*** End Patch\n",
        ))
        .unwrap();
        let err = execute_apply_patch(patch, dir.path()).await.unwrap_err();
        assert!(
            matches!(err, ApplyPatchError::ContextMismatch { ref path, .. } if path == "src/bad.rs"),
            "expected ContextMismatch on src/bad.rs, got {err:?}"
        );

        // CRITICAL: neither file was mutated.
        assert_eq!(
            read_file(dir.path(), "src/good.rs"),
            "the\nquick\nbrown\nfox\n",
            "good.rs must be untouched after rollback"
        );
        assert_eq!(read_file(dir.path(), "src/bad.rs"), "totally\nunrelated\n");
    }

    #[tokio::test]
    async fn executor_rejects_add_when_target_exists() {
        let dir = TempDir::new().unwrap();
        write_file(dir.path(), "src/existing.rs", "already here\n");
        let patch = parse_patch(
            "*** Begin Patch\n\
             *** Add File: src/existing.rs\n\
             +oops\n\
             *** End Patch\n",
        )
        .unwrap();
        let err = execute_apply_patch(patch, dir.path()).await.unwrap_err();
        assert!(matches!(err, ApplyPatchError::TargetAlreadyExists { .. }));
        assert_eq!(read_file(dir.path(), "src/existing.rs"), "already here\n");
    }

    #[tokio::test]
    async fn executor_rejects_update_when_target_missing() {
        let dir = TempDir::new().unwrap();
        let patch = parse_patch(concat!(
            "*** Begin Patch\n",
            "*** Update File: src/ghost.rs\n",
            "@@\n",
            " boo\n",
            "-ghost\n",
            "+real\n",
            "*** End Patch\n",
        ))
        .unwrap();
        let err = execute_apply_patch(patch, dir.path()).await.unwrap_err();
        assert!(matches!(err, ApplyPatchError::TargetNotFound { ref path } if path == "src/ghost.rs"));
    }

    #[tokio::test]
    async fn executor_rejects_delete_when_target_missing() {
        let dir = TempDir::new().unwrap();
        let patch = parse_patch(
            "*** Begin Patch\n\
             *** Delete File: src/ghost.rs\n\
             *** End Patch\n",
        )
        .unwrap();
        let err = execute_apply_patch(patch, dir.path()).await.unwrap_err();
        assert!(matches!(err, ApplyPatchError::TargetNotFound { .. }));
    }

    #[tokio::test]
    async fn executor_handles_multi_hunk_update() {
        let dir = TempDir::new().unwrap();
        write_file(
            dir.path(),
            "src/two.rs",
            "fn one() {\n    let a = 1;\n}\n\nfn two() {\n    let b = 2;\n}\n",
        );
        let patch = parse_patch(concat!(
            "*** Begin Patch\n",
            "*** Update File: src/two.rs\n",
            "@@ fn one @@\n",
            " fn one() {\n",
            "-    let a = 1;\n",
            "+    let a = 11;\n",
            " }\n",
            "@@ fn two @@\n",
            " fn two() {\n",
            "-    let b = 2;\n",
            "+    let b = 22;\n",
            " }\n",
            "*** End Patch\n",
        ))
        .unwrap();
        let result = execute_apply_patch(patch, dir.path()).await.unwrap();
        assert_eq!(result.changes[0].lines_added, 2);
        assert_eq!(result.changes[0].lines_removed, 2);
        let new = read_file(dir.path(), "src/two.rs");
        assert!(new.contains("let a = 11;"));
        assert!(new.contains("let b = 22;"));
    }

    #[tokio::test]
    async fn executor_preserves_crlf_line_endings() {
        let dir = TempDir::new().unwrap();
        write_file(dir.path(), "win.rs", "alpha\r\nbeta\r\n");
        let patch = parse_patch(concat!(
            "*** Begin Patch\n",
            "*** Update File: win.rs\n",
            "@@\n",
            " alpha\n",
            "-beta\n",
            "+bee\n",
            "*** End Patch\n",
        ))
        .unwrap();
        execute_apply_patch(patch, dir.path()).await.unwrap();
        let on_disk = std::fs::read(dir.path().join("win.rs")).unwrap();
        let on_disk = String::from_utf8(on_disk).unwrap();
        assert!(
            on_disk.contains("alpha\r\n") && on_disk.contains("bee\r\n"),
            "expected CRLF preserved, got {on_disk:?}"
        );
    }

    #[tokio::test]
    async fn executor_rejects_path_escape() {
        let dir = TempDir::new().unwrap();
        // Build a Patch by hand to bypass parser-side rejection — we
        // want to prove the executor would also catch a `..` escape
        // that somehow snuck through.
        let patch = Patch {
            changes: vec![FileChange::Add {
                path: "evil/../../boom.txt".to_string(),
                content: "x".to_string(),
            }],
        };
        let err = execute_apply_patch(patch, dir.path()).await.unwrap_err();
        assert!(matches!(err, ApplyPatchError::PathEscape { .. }));
    }

    #[tokio::test]
    async fn executor_rejects_conflicting_changes_on_same_path() {
        let dir = TempDir::new().unwrap();
        // Two adds for the same file in one patch — bad.
        let patch = Patch {
            changes: vec![
                FileChange::Add {
                    path: "x.rs".to_string(),
                    content: "first".to_string(),
                },
                FileChange::Add {
                    path: "x.rs".to_string(),
                    content: "second".to_string(),
                },
            ],
        };
        let err = execute_apply_patch(patch, dir.path()).await.unwrap_err();
        assert!(matches!(err, ApplyPatchError::ConflictingChanges { .. }));
    }

    #[tokio::test]
    async fn executor_summary_reports_counts() {
        let dir = TempDir::new().unwrap();
        write_file(dir.path(), "del.rs", "doomed\n");
        write_file(dir.path(), "upd.rs", "a\nb\n");
        let patch = parse_patch(concat!(
            "*** Begin Patch\n",
            "*** Add File: new.rs\n",
            "+content\n",
            "*** Update File: upd.rs\n",
            "@@\n",
            " a\n",
            "-b\n",
            "+bee\n",
            "*** Delete File: del.rs\n",
            "*** End Patch\n",
        ))
        .unwrap();
        let result = execute_apply_patch(patch, dir.path()).await.unwrap();
        assert!(result.summary.contains("1 added"));
        assert!(result.summary.contains("1 updated"));
        assert!(result.summary.contains("1 deleted"));
        assert!(result.summary.contains("3 files"));
    }

    #[tokio::test]
    async fn executor_creates_parent_directories_for_add() {
        let dir = TempDir::new().unwrap();
        let patch = parse_patch(
            "*** Begin Patch\n\
             *** Add File: deep/nested/path/file.rs\n\
             +pub fn deep() {}\n\
             *** End Patch\n",
        )
        .unwrap();
        execute_apply_patch(patch, dir.path()).await.unwrap();
        assert_eq!(
            read_file(dir.path(), "deep/nested/path/file.rs"),
            "pub fn deep() {}"
        );
    }

    #[tokio::test]
    async fn executor_anchors_on_leading_context_then_swaps_trailing_line() {
        // Smoke-test the common shape: a couple of context lines used
        // purely for anchoring followed by a single `-`/`+` swap. The
        // returned line counts must reflect only the swap, not the
        // context lines.
        let dir = TempDir::new().unwrap();
        write_file(dir.path(), "file.rs", "alpha\nbeta\ngamma\n");
        let patch = parse_patch(concat!(
            "*** Begin Patch\n",
            "*** Update File: file.rs\n",
            "@@\n",
            " alpha\n",
            " beta\n",
            "-gamma\n",
            "+omega\n",
            "*** End Patch\n",
        ))
        .unwrap();
        let result = execute_apply_patch(patch, dir.path()).await.unwrap();
        assert_eq!(read_file(dir.path(), "file.rs"), "alpha\nbeta\nomega\n");
        assert_eq!(result.changes[0].lines_added, 1);
        assert_eq!(result.changes[0].lines_removed, 1);
    }

    #[test]
    fn count_lines_counts_no_trailing_newline_correctly() {
        assert_eq!(count_lines(""), 0);
        assert_eq!(count_lines("a"), 1);
        assert_eq!(count_lines("a\n"), 1);
        assert_eq!(count_lines("a\nb"), 2);
        assert_eq!(count_lines("a\nb\n"), 2);
        assert_eq!(count_lines("a\nb\nc\n"), 3);
    }
}
