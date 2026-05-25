//! Parser for the codex `*** Begin Patch / *** End Patch` envelope format.
//!
//! See [`super`] for envelope semantics and end-to-end usage. This module
//! is pure-string: it only validates structure and partitions the body
//! into typed [`FileChange`] / [`Hunk`] records. Actual filesystem
//! application lives in [`super::executor`].

use std::fmt;

/// Top-level parsed patch: an ordered list of per-file changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Patch {
    pub changes: Vec<FileChange>,
}

/// One file-scoped directive within a patch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileChange {
    /// `*** Add File: <path>` followed by `+`-prefixed content lines.
    Add { path: String, content: String },
    /// `*** Update File: <path>` followed by one or more [`Hunk`]s.
    Update { path: String, hunks: Vec<Hunk> },
    /// `*** Delete File: <path>` with no body.
    Delete { path: String },
}

/// One `@@ ... @@` block inside an `Update File` body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    /// Text between the `@@` markers on the header line (informational
    /// only — matching uses the body's context lines, not the header).
    pub context_header: Option<String>,
    pub lines: Vec<HunkLine>,
}

/// One line inside a hunk body. Markers are stripped; the stored text
/// preserves any indentation that followed the marker character.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HunkLine {
    /// `<space>content` — must match the target file at the inferred
    /// location.
    Context(String),
    /// `-content` — line to remove. Must also match the target file.
    Removal(String),
    /// `+content` — line to add. Does not need to match anything in
    /// the target file.
    Addition(String),
}

/// Structural failures detected at parse time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatchError {
    /// Input did not contain a `*** Begin Patch` line.
    MissingBeginMarker,
    /// Input had `*** Begin Patch` but no matching `*** End Patch`.
    MissingEndMarker,
    /// Encountered a `*** ` line that wasn't `Begin Patch`,
    /// `End Patch`, `Add File:`, `Update File:`, or `Delete File:`.
    UnknownDirective { line_number: usize, line: String },
    /// A hunk body was malformed (e.g. content before the first `@@`
    /// inside an `Update File` block, or a line with no recognized
    /// marker character).
    MalformedHunk {
        file: String,
        line_number: usize,
        reason: String,
    },
    /// `*** Add File:` was present but the body contained no
    /// `+`-prefixed content lines.
    EmptyAddFile { file: String },
    /// The path was syntactically rejected: absolute, contained `..`,
    /// was empty, or referenced a Windows drive letter / UNC prefix.
    InvalidPath { path: String, reason: String },
}

impl fmt::Display for PatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingBeginMarker => {
                write!(f, "patch is missing the `*** Begin Patch` envelope marker")
            }
            Self::MissingEndMarker => {
                write!(f, "patch is missing the `*** End Patch` envelope marker")
            }
            Self::UnknownDirective { line_number, line } => {
                write!(
                    f,
                    "unknown patch directive at line {line_number}: {line:?}"
                )
            }
            Self::MalformedHunk {
                file,
                line_number,
                reason,
            } => {
                write!(
                    f,
                    "malformed hunk in {file:?} near line {line_number}: {reason}"
                )
            }
            Self::EmptyAddFile { file } => {
                write!(f, "`*** Add File: {file}` has no `+`-prefixed content lines")
            }
            Self::InvalidPath { path, reason } => {
                write!(f, "invalid patch path {path:?}: {reason}")
            }
        }
    }
}

impl std::error::Error for PatchError {}

const BEGIN_MARKER: &str = "*** Begin Patch";
const END_MARKER: &str = "*** End Patch";
const ADD_PREFIX: &str = "*** Add File: ";
const UPDATE_PREFIX: &str = "*** Update File: ";
const DELETE_PREFIX: &str = "*** Delete File: ";

/// Parse a codex-format patch envelope.
///
/// Lines outside `*** Begin Patch` / `*** End Patch` are treated as
/// preamble/trailing chatter and ignored. CRLF endings are stripped
/// from each line before classification.
///
/// On success returns the structured [`Patch`]. On failure returns the
/// first [`PatchError`] encountered; the model is expected to re-emit
/// a corrected patch.
///
/// # Errors
///
/// Returns one of the [`PatchError`] variants when the envelope is
/// missing, a directive is unknown, a path fails validation, an
/// `Add File` body is empty, or an `Update File` hunk is malformed.
pub fn parse_patch(input: &str) -> Result<Patch, PatchError> {
    let lines: Vec<&str> = input.lines().collect();

    let begin_idx = lines
        .iter()
        .position(|l| strip_cr(l) == BEGIN_MARKER)
        .ok_or(PatchError::MissingBeginMarker)?;

    let end_idx = lines
        .iter()
        .enumerate()
        .skip(begin_idx + 1)
        .find_map(|(i, l)| if strip_cr(l) == END_MARKER { Some(i) } else { None })
        .ok_or(PatchError::MissingEndMarker)?;

    let body = &lines[begin_idx + 1..end_idx];
    let body_base = begin_idx + 2; // 1-indexed line number of body[0]

    let mut changes = Vec::new();
    let mut i = 0;

    while i < body.len() {
        let raw = body[i];
        let line = strip_cr(raw);
        let line_number = body_base + i;

        if let Some(rest) = line.strip_prefix(ADD_PREFIX) {
            let path = validate_path(rest)?;
            i += 1;
            let mut content = String::new();
            let mut saw_any = false;
            while i < body.len() {
                let next = strip_cr(body[i]);
                if next.starts_with("*** ") {
                    break;
                }
                if let Some(rest) = next.strip_prefix('+') {
                    if saw_any {
                        content.push('\n');
                    }
                    content.push_str(rest);
                    saw_any = true;
                    i += 1;
                } else if next.is_empty() {
                    // Tolerate the occasional blank line inside an Add
                    // File body (some models emit it between paragraphs);
                    // treat it as an empty content line.
                    if saw_any {
                        content.push('\n');
                    }
                    saw_any = true;
                    i += 1;
                } else {
                    return Err(PatchError::MalformedHunk {
                        file: path,
                        line_number: body_base + i,
                        reason: format!(
                            "expected `+content` line in Add File body, got {next:?}"
                        ),
                    });
                }
            }
            if !saw_any {
                return Err(PatchError::EmptyAddFile { file: path });
            }
            changes.push(FileChange::Add { path, content });
            continue;
        }

        if let Some(rest) = line.strip_prefix(UPDATE_PREFIX) {
            let path = validate_path(rest)?;
            i += 1;
            let (hunks, consumed) = parse_update_body(&body[i..], body_base + i, &path)?;
            i += consumed;
            changes.push(FileChange::Update { path, hunks });
            continue;
        }

        if let Some(rest) = line.strip_prefix(DELETE_PREFIX) {
            let path = validate_path(rest)?;
            changes.push(FileChange::Delete { path });
            i += 1;
            continue;
        }

        if line.is_empty() {
            // Blank separator lines between top-level directives are
            // tolerated. Anything non-empty that doesn't match a known
            // directive is rejected.
            i += 1;
            continue;
        }

        return Err(PatchError::UnknownDirective {
            line_number,
            line: line.to_string(),
        });
    }

    Ok(Patch { changes })
}

/// Parse one or more hunks from the body of a `*** Update File:` block.
///
/// Returns the parsed hunks and the number of body lines consumed (so
/// the outer loop can advance past them). Stops at the next `*** `
/// directive or the end of the body.
fn parse_update_body(
    body: &[&str],
    base_line: usize,
    file: &str,
) -> Result<(Vec<Hunk>, usize), PatchError> {
    let mut hunks: Vec<Hunk> = Vec::new();
    let mut current: Option<Hunk> = None;
    let mut i = 0;

    while i < body.len() {
        let line = strip_cr(body[i]);

        if line.starts_with("*** ") {
            break;
        }

        if let Some(header) = parse_hunk_header(line) {
            if let Some(h) = current.take() {
                push_hunk(&mut hunks, h, file, base_line + i)?;
            }
            current = Some(Hunk {
                context_header: header,
                lines: Vec::new(),
            });
            i += 1;
            continue;
        }

        let Some(h) = current.as_mut() else {
            // Tolerate blank padding before the first @@ marker. Some
            // models emit a blank line after the `*** Update File:`
            // header to visually separate the directive from the
            // first hunk; treat it as filler.
            if line.is_empty() {
                i += 1;
                continue;
            }
            return Err(PatchError::MalformedHunk {
                file: file.to_string(),
                line_number: base_line + i,
                reason: format!("expected `@@` hunk header before content, got {line:?}"),
            });
        };

        // Classify the marker character. Use byte-level inspection so
        // we preserve any trailing whitespace / non-ASCII content
        // verbatim (raw `&str` slicing on a one-byte ASCII prefix is
        // safe).
        match line.as_bytes().first() {
            Some(b' ') => h.lines.push(HunkLine::Context(line[1..].to_string())),
            Some(b'-') => h.lines.push(HunkLine::Removal(line[1..].to_string())),
            Some(b'+') => h.lines.push(HunkLine::Addition(line[1..].to_string())),
            // A completely empty line inside a hunk body is the
            // codex-rs convention for "blank context line"; many
            // models drop the leading space.
            None => h.lines.push(HunkLine::Context(String::new())),
            Some(_) => {
                return Err(PatchError::MalformedHunk {
                    file: file.to_string(),
                    line_number: base_line + i,
                    reason: format!(
                        "expected hunk line starting with ` `, `-`, or `+`, got {line:?}"
                    ),
                });
            }
        }
        i += 1;
    }

    if let Some(h) = current.take() {
        push_hunk(&mut hunks, h, file, base_line + i)?;
    }

    if hunks.is_empty() {
        return Err(PatchError::MalformedHunk {
            file: file.to_string(),
            line_number: base_line,
            reason: "`*** Update File:` block had no `@@` hunks".to_string(),
        });
    }

    Ok((hunks, i))
}

fn push_hunk(
    hunks: &mut Vec<Hunk>,
    hunk: Hunk,
    file: &str,
    line_number: usize,
) -> Result<(), PatchError> {
    if hunk.lines.is_empty() {
        return Err(PatchError::MalformedHunk {
            file: file.to_string(),
            line_number,
            reason: "hunk had no `+`/`-`/` ` lines".to_string(),
        });
    }
    hunks.push(hunk);
    Ok(())
}

/// Parse a `@@ ... @@` header. Returns `Some(Some(header))` when the
/// line is a header with text between the markers, `Some(None)` when
/// it's a bare `@@` separator, and `None` for everything else.
fn parse_hunk_header(line: &str) -> Option<Option<String>> {
    let trimmed = line.trim();
    if !trimmed.starts_with("@@") {
        return None;
    }
    // Header must look like `@@ ... @@` or just `@@`. We are lenient:
    // any line starting with `@@` is treated as a hunk header; the
    // header text (if any) is informational only.
    let inner = trimmed.trim_start_matches('@').trim();
    let inner = inner.trim_end_matches('@').trim();
    if inner.is_empty() {
        Some(None)
    } else {
        Some(Some(inner.to_string()))
    }
}

/// Strip a trailing `\r`. Cheap CRLF tolerance for inputs pasted from
/// Windows terminals.
fn strip_cr(line: &str) -> &str {
    line.strip_suffix('\r').unwrap_or(line)
}

/// Validate and normalize a patch path.
///
/// - Trim surrounding whitespace.
/// - Normalize backslashes to forward slashes (the canonical form).
/// - Reject empty paths.
/// - Reject absolute paths (Unix `/...`, Windows drive letter, UNC).
/// - Reject any path containing a `..` segment.
fn validate_path(raw: &str) -> Result<String, PatchError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(PatchError::InvalidPath {
            path: raw.to_string(),
            reason: "path must not be empty".to_string(),
        });
    }

    let normalized = trimmed.replace('\\', "/");

    if normalized.starts_with('/') {
        return Err(PatchError::InvalidPath {
            path: normalized,
            reason: "absolute paths are not allowed (must be workspace-relative)".to_string(),
        });
    }
    if normalized.starts_with("//") || normalized.starts_with("\\\\") {
        return Err(PatchError::InvalidPath {
            path: normalized,
            reason: "UNC paths are not allowed".to_string(),
        });
    }
    // Windows drive letter, e.g. `C:/foo` or `c:foo`.
    if let Some(second) = normalized.as_bytes().get(1) {
        if *second == b':' && normalized.as_bytes()[0].is_ascii_alphabetic() {
            return Err(PatchError::InvalidPath {
                path: normalized,
                reason: "Windows drive-letter paths are not allowed".to_string(),
            });
        }
    }
    if normalized.starts_with("./") {
        return Err(PatchError::InvalidPath {
            path: normalized,
            reason: "leading `./` is not allowed; supply a bare relative path".to_string(),
        });
    }
    for segment in normalized.split('/') {
        if segment == ".." {
            return Err(PatchError::InvalidPath {
                path: normalized.clone(),
                reason: "`..` segments are not allowed".to_string(),
            });
        }
    }

    Ok(normalized)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn add(path: &str, content: &str) -> FileChange {
        FileChange::Add {
            path: path.to_string(),
            content: content.to_string(),
        }
    }

    fn delete(path: &str) -> FileChange {
        FileChange::Delete {
            path: path.to_string(),
        }
    }

    #[test]
    fn parse_minimal_add_file() {
        let input = "*** Begin Patch\n\
                     *** Add File: src/new.rs\n\
                     +pub fn new() {}\n\
                     *** End Patch\n";
        let patch = parse_patch(input).expect("parse");
        assert_eq!(patch.changes.len(), 1);
        assert_eq!(patch.changes[0], add("src/new.rs", "pub fn new() {}"));
    }

    #[test]
    fn parse_minimal_add_file_multiline() {
        let input = "*** Begin Patch\n\
                     *** Add File: src/new.rs\n\
                     +line one\n\
                     +line two\n\
                     +line three\n\
                     *** End Patch\n";
        let patch = parse_patch(input).expect("parse");
        assert_eq!(
            patch.changes[0],
            add("src/new.rs", "line one\nline two\nline three")
        );
    }

    #[test]
    fn parse_minimal_update_file_one_hunk() {
        let input = concat!(
            "*** Begin Patch\n",
            "*** Update File: src/lib.rs\n",
            "@@ fn main @@\n",
            " let x = 1;\n",
            "-let y = 2;\n",
            "+let y = 3;\n",
            " let z = x + y;\n",
            "*** End Patch\n",
        );
        let patch = parse_patch(input).expect("parse");
        assert_eq!(patch.changes.len(), 1);
        match &patch.changes[0] {
            FileChange::Update { path, hunks } => {
                assert_eq!(path, "src/lib.rs");
                assert_eq!(hunks.len(), 1);
                assert_eq!(hunks[0].context_header.as_deref(), Some("fn main"));
                assert_eq!(
                    hunks[0].lines,
                    vec![
                        HunkLine::Context("let x = 1;".to_string()),
                        HunkLine::Removal("let y = 2;".to_string()),
                        HunkLine::Addition("let y = 3;".to_string()),
                        HunkLine::Context("let z = x + y;".to_string()),
                    ]
                );
            }
            other => panic!("expected Update, got {other:?}"),
        }
    }

    #[test]
    fn parse_minimal_delete_file() {
        let input = "*** Begin Patch\n\
                     *** Delete File: src/old.rs\n\
                     *** End Patch\n";
        let patch = parse_patch(input).expect("parse");
        assert_eq!(patch.changes, vec![delete("src/old.rs")]);
    }

    #[test]
    fn parse_multi_file_patch() {
        let input = "*** Begin Patch\n\
                     *** Add File: src/a.rs\n\
                     +pub mod a;\n\
                     *** Update File: src/b.rs\n\
                     @@\n\
                     -old\n\
                     +new\n\
                     *** Delete File: src/c.rs\n\
                     *** End Patch\n";
        let patch = parse_patch(input).expect("parse");
        assert_eq!(patch.changes.len(), 3);
        assert!(matches!(&patch.changes[0], FileChange::Add { path, .. } if path == "src/a.rs"));
        assert!(
            matches!(&patch.changes[1], FileChange::Update { path, hunks } if path == "src/b.rs" && hunks.len() == 1)
        );
        assert_eq!(patch.changes[2], delete("src/c.rs"));
    }

    #[test]
    fn parse_multi_hunk_update() {
        let input = "*** Begin Patch\n\
                     *** Update File: src/lib.rs\n\
                     @@ fn one @@\n\
                     -a\n\
                     +b\n\
                     @@ fn two @@\n\
                     -c\n\
                     +d\n\
                     *** End Patch\n";
        let patch = parse_patch(input).expect("parse");
        let FileChange::Update { hunks, .. } = &patch.changes[0] else {
            panic!("expected update");
        };
        assert_eq!(hunks.len(), 2);
        assert_eq!(hunks[0].context_header.as_deref(), Some("fn one"));
        assert_eq!(hunks[1].context_header.as_deref(), Some("fn two"));
    }

    #[test]
    fn parse_crlf_tolerant() {
        let input = "*** Begin Patch\r\n*** Add File: src/new.rs\r\n+pub fn new() {}\r\n*** End Patch\r\n";
        let patch = parse_patch(input).expect("parse");
        assert_eq!(patch.changes[0], add("src/new.rs", "pub fn new() {}"));
    }

    #[test]
    fn parse_strips_trailing_cr() {
        // Ensure stray \r inside a hunk doesn't poison the content.
        let input = "*** Begin Patch\n\
                     *** Update File: src/lib.rs\r\n\
                     @@\r\n\
                     \x20context line\r\n\
                     -old\r\n\
                     +new\r\n\
                     *** End Patch\r\n";
        let patch = parse_patch(input).expect("parse");
        let FileChange::Update { path, hunks } = &patch.changes[0] else {
            panic!();
        };
        assert_eq!(path, "src/lib.rs");
        assert_eq!(hunks[0].lines[0], HunkLine::Context("context line".into()));
        assert_eq!(hunks[0].lines[1], HunkLine::Removal("old".into()));
        assert_eq!(hunks[0].lines[2], HunkLine::Addition("new".into()));
    }

    #[test]
    fn parse_preserves_indentation() {
        let input = concat!(
            "*** Begin Patch\n",
            "*** Update File: src/lib.rs\n",
            "@@\n",
            "     let four = 4;\n",
            "-    let two = 2;\n",
            "+    let two = 22;\n",
            "*** End Patch\n",
        );
        let patch = parse_patch(input).expect("parse");
        let FileChange::Update { hunks, .. } = &patch.changes[0] else {
            panic!();
        };
        assert_eq!(
            hunks[0].lines,
            vec![
                HunkLine::Context("    let four = 4;".to_string()),
                HunkLine::Removal("    let two = 2;".to_string()),
                HunkLine::Addition("    let two = 22;".to_string()),
            ]
        );
    }

    #[test]
    fn parse_rejects_missing_begin_marker() {
        let input = "*** Add File: src/new.rs\n+content\n*** End Patch\n";
        assert_eq!(parse_patch(input), Err(PatchError::MissingBeginMarker));
    }

    #[test]
    fn parse_rejects_missing_end_marker() {
        let input = "*** Begin Patch\n*** Add File: src/new.rs\n+content\n";
        assert_eq!(parse_patch(input), Err(PatchError::MissingEndMarker));
    }

    #[test]
    fn parse_rejects_unknown_directive() {
        let input = "*** Begin Patch\n\
                     *** Rename File: src/old.rs -> src/new.rs\n\
                     *** End Patch\n";
        match parse_patch(input) {
            Err(PatchError::UnknownDirective { line, .. }) => {
                assert!(line.starts_with("*** Rename"));
            }
            other => panic!("expected UnknownDirective, got {other:?}"),
        }
    }

    #[test]
    fn parse_rejects_absolute_path() {
        let input = "*** Begin Patch\n\
                     *** Add File: /etc/passwd\n\
                     +bad\n\
                     *** End Patch\n";
        match parse_patch(input) {
            Err(PatchError::InvalidPath { reason, .. }) => {
                assert!(reason.contains("absolute"));
            }
            other => panic!("expected InvalidPath, got {other:?}"),
        }
    }

    #[test]
    fn parse_rejects_windows_drive_letter() {
        let input = "*** Begin Patch\n\
                     *** Add File: C:/Windows/System32/cmd.exe\n\
                     +bad\n\
                     *** End Patch\n";
        match parse_patch(input) {
            Err(PatchError::InvalidPath { reason, .. }) => {
                assert!(reason.contains("drive-letter"));
            }
            other => panic!("expected InvalidPath, got {other:?}"),
        }
    }

    #[test]
    fn parse_rejects_dotdot_path() {
        let input = "*** Begin Patch\n\
                     *** Add File: ../escape.rs\n\
                     +bad\n\
                     *** End Patch\n";
        match parse_patch(input) {
            Err(PatchError::InvalidPath { reason, .. }) => {
                assert!(reason.contains(".."));
            }
            other => panic!("expected InvalidPath, got {other:?}"),
        }
    }

    #[test]
    fn parse_rejects_leading_dot_slash() {
        let input = "*** Begin Patch\n\
                     *** Add File: ./local.rs\n\
                     +bad\n\
                     *** End Patch\n";
        match parse_patch(input) {
            Err(PatchError::InvalidPath { reason, .. }) => {
                assert!(reason.contains("./"));
            }
            other => panic!("expected InvalidPath, got {other:?}"),
        }
    }

    #[test]
    fn parse_rejects_empty_path() {
        let input = "*** Begin Patch\n\
                     *** Add File: \n\
                     +bad\n\
                     *** End Patch\n";
        match parse_patch(input) {
            Err(PatchError::InvalidPath { reason, .. }) => {
                assert!(reason.contains("empty"));
            }
            other => panic!("expected InvalidPath, got {other:?}"),
        }
    }

    #[test]
    fn parse_rejects_empty_add_file_body() {
        let input = "*** Begin Patch\n\
                     *** Add File: src/empty.rs\n\
                     *** End Patch\n";
        match parse_patch(input) {
            Err(PatchError::EmptyAddFile { file }) => {
                assert_eq!(file, "src/empty.rs");
            }
            other => panic!("expected EmptyAddFile, got {other:?}"),
        }
    }

    #[test]
    fn parse_rejects_update_body_without_hunk_header() {
        let input = "*** Begin Patch\n\
                     *** Update File: src/lib.rs\n\
                     -lost\n\
                     +found\n\
                     *** End Patch\n";
        match parse_patch(input) {
            Err(PatchError::MalformedHunk { reason, .. }) => {
                assert!(reason.contains("@@"));
            }
            other => panic!("expected MalformedHunk, got {other:?}"),
        }
    }

    #[test]
    fn parse_normalizes_backslashes() {
        let input = "*** Begin Patch\n\
                     *** Add File: src\\foo\\bar.rs\n\
                     +content\n\
                     *** End Patch\n";
        let patch = parse_patch(input).expect("parse");
        match &patch.changes[0] {
            FileChange::Add { path, .. } => assert_eq!(path, "src/foo/bar.rs"),
            _ => panic!(),
        }
    }

    #[test]
    fn parse_ignores_preamble_and_trailing_text() {
        let input = "the model said something here first\n\
                     more chatter\n\
                     *** Begin Patch\n\
                     *** Delete File: src/old.rs\n\
                     *** End Patch\n\
                     and a closing remark\n";
        let patch = parse_patch(input).expect("parse");
        assert_eq!(patch.changes, vec![delete("src/old.rs")]);
    }

    #[test]
    fn parse_supports_bare_at_at_header() {
        let input = "*** Begin Patch\n\
                     *** Update File: src/lib.rs\n\
                     @@\n\
                     -a\n\
                     +b\n\
                     *** End Patch\n";
        let patch = parse_patch(input).expect("parse");
        let FileChange::Update { hunks, .. } = &patch.changes[0] else {
            panic!();
        };
        assert_eq!(hunks[0].context_header, None);
        assert_eq!(hunks[0].lines.len(), 2);
    }

    #[test]
    fn parse_supports_multiple_hunks_with_context_only_first() {
        let input = concat!(
            "*** Begin Patch\n",
            "*** Update File: src/lib.rs\n",
            "@@ outer @@\n",
            " keep\n",
            "-drop\n",
            "+add\n",
            "@@\n",
            " keep2\n",
            "-drop2\n",
            "+add2\n",
            "*** End Patch\n",
        );
        let patch = parse_patch(input).expect("parse");
        let FileChange::Update { hunks, .. } = &patch.changes[0] else {
            panic!();
        };
        assert_eq!(hunks.len(), 2);
        assert_eq!(hunks[0].context_header.as_deref(), Some("outer"));
        assert_eq!(hunks[1].context_header, None);
    }

    #[test]
    fn parse_handles_blank_line_inside_hunk_as_blank_context() {
        let input = concat!(
            "*** Begin Patch\n",
            "*** Update File: src/lib.rs\n",
            "@@\n",
            " before\n",
            "\n",
            " after\n",
            "-removed\n",
            "+added\n",
            "*** End Patch\n",
        );
        let patch = parse_patch(input).expect("parse");
        let FileChange::Update { hunks, .. } = &patch.changes[0] else {
            panic!();
        };
        assert_eq!(
            hunks[0].lines,
            vec![
                HunkLine::Context("before".into()),
                HunkLine::Context(String::new()),
                HunkLine::Context("after".into()),
                HunkLine::Removal("removed".into()),
                HunkLine::Addition("added".into()),
            ]
        );
    }
}
