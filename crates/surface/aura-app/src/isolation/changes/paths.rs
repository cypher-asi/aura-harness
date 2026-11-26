//! Reading what git lists: names, patches, skipped folders.

use std::path::Path;

pub(super) fn skipped(relative: &Path, skip: &[&str]) -> bool {
    relative
        .components()
        .any(|part| skip.iter().any(|name| part.as_os_str() == *name))
}

/// Files `patch` (as [`diff_since`] writes it) changes.
#[must_use]
pub fn files_in(patch: &str) -> usize {
    patch
        .lines()
        .filter(|line| line.starts_with("diff --git "))
        .count()
}

/// The files `patch` (as [`diff_since`] writes it) changes, relative to
/// the root it was taken in, in the patch's order. A renamed file is
/// named as it is after the patch.
#[must_use]
pub fn paths_in(patch: &str) -> Vec<String> {
    patch
        .lines()
        .filter_map(|line| line.strip_prefix("diff --git "))
        .filter_map(|names| {
            // `a/<old> b/<new>`; a name with spaces or quotes is taken
            // from the last ` b/`.
            let new = names.rsplit_once(" b/").map(|(_, new)| new)?;
            Some(new.trim_matches('"').to_string())
        })
        .collect()
}

/// The NUL-separated names in `listed`.
pub(super) fn names(listed: &[u8]) -> impl Iterator<Item = String> + '_ {
    listed
        .split(|b| *b == 0)
        .filter(|name| !name.is_empty())
        .map(|name| String::from_utf8_lossy(name).into_owned())
}
