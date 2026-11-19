//! Directory walking shared by `search_code` and `find_files`.
//!
//! Honours `.gitignore` (including those in parent folders), `.ignore`
//! and git's global and per-repo excludes, whether or not the folder is a
//! git repository, so generated and vendored trees the project ignores
//! never reach the model. Hidden files are walked; each tool decides
//! which directory names to skip on top of the ignore rules. Entries come
//! sorted by name so repeated calls return the same order.

use std::path::Path;

/// Walk `root`, skipping directories whose name `skip_dir` rejects.
pub(super) fn walk(root: &Path, skip_dir: fn(&str) -> bool) -> ignore::Walk {
    ignore::WalkBuilder::new(root)
        .hidden(false)
        .parents(true)
        .ignore(true)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .require_git(false)
        .follow_links(false)
        .sort_by_file_name(std::cmp::Ord::cmp)
        .filter_entry(move |entry| {
            let is_dir = entry.file_type().is_some_and(|t| t.is_dir());
            !(is_dir && entry.depth() > 0 && skip_dir(&entry.file_name().to_string_lossy()))
        })
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn names(root: &Path, skip_dir: fn(&str) -> bool) -> Vec<String> {
        walk(root, skip_dir)
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
            .map(|e| {
                e.path()
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect()
    }

    #[test]
    fn gitignored_files_are_skipped_without_a_git_repo() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(".gitignore"), "gen/\n*.log\n").unwrap();
        fs::create_dir(dir.path().join("gen")).unwrap();
        fs::write(dir.path().join("gen/out.rs"), "x").unwrap();
        fs::write(dir.path().join("debug.log"), "x").unwrap();
        fs::write(dir.path().join("lib.rs"), "x").unwrap();

        let found = names(dir.path(), |_| false);
        assert!(found.contains(&"lib.rs".to_string()), "{found:?}");
        assert!(!found.iter().any(|f| f.starts_with("gen/")), "{found:?}");
        assert!(!found.contains(&"debug.log".to_string()), "{found:?}");
    }

    #[test]
    fn hidden_files_are_walked_and_skip_dir_applies_below_the_root() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join(".github")).unwrap();
        fs::write(dir.path().join(".github/ci.yml"), "x").unwrap();
        fs::create_dir(dir.path().join("skipme")).unwrap();
        fs::write(dir.path().join("skipme/a.rs"), "x").unwrap();

        let found = names(dir.path(), |name| name == "skipme");
        assert_eq!(found, vec![".github/ci.yml".to_string()]);
    }

    #[test]
    fn entries_come_sorted_by_name() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["c.rs", "a.rs", "b.rs"] {
            fs::write(dir.path().join(name), "x").unwrap();
        }
        assert_eq!(names(dir.path(), |_| false), vec!["a.rs", "b.rs", "c.rs"]);
    }
}
