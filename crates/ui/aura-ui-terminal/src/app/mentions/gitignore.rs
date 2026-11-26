//! The root `.gitignore` patterns the `@` picker's walk skips.

use std::path::Path;

/// The plain names and `*.ext` patterns of a root `.gitignore`. Anything
/// more elaborate (negation, nested paths, character classes) is ignored:
/// the skip list is a convenience, not a security boundary.
#[derive(Debug, Default)]
pub(super) struct GitIgnore {
    names: Vec<String>,
    dir_names: Vec<String>,
    suffixes: Vec<String>,
}

impl GitIgnore {
    pub(super) fn read(root: &Path) -> Self {
        let Ok(text) = std::fs::read_to_string(root.join(".gitignore")) else {
            return Self::default();
        };
        let mut ignore = Self::default();
        for line in text.lines().map(str::trim) {
            if line.is_empty() || line.starts_with('#') || line.starts_with('!') {
                continue;
            }
            let line = line.trim_start_matches('/');
            let (pattern, dir_only) = match line.strip_suffix('/') {
                Some(dir) => (dir, true),
                None => (line, false),
            };
            if pattern.contains('/') {
                continue;
            }
            if let Some(suffix) = pattern.strip_prefix('*') {
                if !suffix.is_empty() && !suffix.contains(['*', '?', '[']) {
                    ignore.suffixes.push(suffix.to_string());
                }
            } else if !pattern.contains(['*', '?', '[']) {
                if dir_only {
                    ignore.dir_names.push(pattern.to_string());
                } else {
                    ignore.names.push(pattern.to_string());
                }
            }
        }
        ignore
    }

    pub(super) fn matches(&self, name: &str, is_dir: bool) -> bool {
        self.names.iter().any(|n| n == name)
            || (is_dir && self.dir_names.iter().any(|n| n == name))
            || self.suffixes.iter().any(|s| name.ends_with(s.as_str()))
    }
}
