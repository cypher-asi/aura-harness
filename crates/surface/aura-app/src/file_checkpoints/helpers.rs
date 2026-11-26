//! Small helpers: diffs, file IO, hashing and previews.

use super::*;

/// Each file `manifests` changed, with what it held before the first of
/// them changed it, by path.
pub(super) fn earliest<'a>(
    manifests: impl IntoIterator<Item = &'a TurnManifest>,
) -> BTreeMap<&'a PathBuf, &'a Option<String>> {
    let mut sorted: Vec<&TurnManifest> = manifests.into_iter().collect();
    sorted.sort_by_key(|m| m.started_ms);
    let mut out = BTreeMap::new();
    for file in sorted.iter().flat_map(|m| &m.files) {
        out.entry(&file.path).or_insert(&file.blob);
    }
    out
}

/// Unified diff hunks from `before` to `after` (`None` for a missing file).
/// `None` when either side is not UTF-8 text or both are very large.
#[must_use]
pub fn unified_diff(before: Option<&[u8]>, after: Option<&[u8]>) -> Option<String> {
    let old = std::str::from_utf8(before.unwrap_or_default()).ok()?;
    let new = std::str::from_utf8(after.unwrap_or_default()).ok()?;
    if old.len() + new.len() > MAX_DIFF_BYTES {
        return None;
    }
    Some(
        TextDiff::from_lines(old, new)
            .unified_diff()
            .context_radius(3)
            .to_string(),
    )
}

/// The file at `path`, or `None` when there is none.
pub(super) fn read_optional(path: &Path) -> std::io::Result<Option<Vec<u8>>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

pub(super) fn write_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, bytes)
}

pub(super) fn display_path(path: &Path, workspace: Option<&Path>) -> String {
    workspace
        .and_then(|w| path.strip_prefix(w).ok())
        .unwrap_or(path)
        .display()
        .to_string()
}

pub(super) fn hash(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

/// The first line of `prompt`, cut to [`PROMPT_CHARS`].
pub(super) fn preview(prompt: &str) -> String {
    let line = prompt.lines().next().unwrap_or("").trim();
    let mut out: String = line.chars().take(PROMPT_CHARS).collect();
    if line.chars().count() > PROMPT_CHARS || prompt.trim().lines().nth(1).is_some() {
        out.push('…');
    }
    out
}
