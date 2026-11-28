//! `aura changelog`: assemble the `changelog.d/` fragments into
//! CHANGELOG.md, newest first, for a release.
//!
//! A fragment's age is when git added it (`git log --diff-filter=A`); one
//! not committed yet counts as newest. Without `--write` nothing changes:
//! the entries that would go under `[Unreleased]` are printed. Every mode
//! first checks that no fragment's entry is already in CHANGELOG.md;
//! `--check` does only that.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Context as _;
use aura_exec_merge_queue::changelog::{
    assemble, check, newest_first, paths_in, section, Fragment, CHANGELOG_FILE, FRAGMENT_DIR,
};

use crate::cli::ChangelogArgs;

/// Run `aura changelog`.
///
/// # Errors
/// The folder is not in a git repo, or the fragments or CHANGELOG.md
/// can't be read, assembled or written.
pub async fn cmd_changelog(args: ChangelogArgs) -> anyhow::Result<()> {
    tokio::task::spawn_blocking(move || run(&args))
        .await
        .context("the changelog task stopped")?
}

fn run(args: &ChangelogArgs) -> anyhow::Result<()> {
    let start = args.repo.clone().unwrap_or_else(|| PathBuf::from("."));
    let top = repo_top(&start)?;
    let dir = top.join(FRAGMENT_DIR);
    let fragments = Fragment::read_dir(&dir)?;
    let path = top.join(CHANGELOG_FILE);
    let changelog =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    // Every mode checks first: a duplicate would be listed twice.
    check(&changelog, &fragments)?;
    if args.check {
        println!(
            "{} changelog fragments in {}; none is already in {}.",
            fragments.len(),
            dir.display(),
            path.display()
        );
        return Ok(());
    }
    if fragments.is_empty() {
        println!("No changelog fragments in {}.", dir.display());
        return Ok(());
    }
    let fragments = newest_first(fragments, &added_times(&top));
    if !args.write {
        println!("{}", section(&fragments)?);
        return Ok(());
    }
    let assembled = assemble(&changelog, &fragments)?;
    std::fs::write(&path, assembled).with_context(|| format!("writing {}", path.display()))?;
    for fragment in paths_in(&dir, &fragments) {
        std::fs::remove_file(&fragment)
            .with_context(|| format!("deleting {}", fragment.display()))?;
    }
    println!(
        "Put {} fragments into {}; commit it with the deleted fragments.",
        fragments.len(),
        path.display()
    );
    Ok(())
}

/// The top of the repo `dir` is in.
fn repo_top(dir: &Path) -> anyhow::Result<PathBuf> {
    let out = git(dir, &["rev-parse", "--show-toplevel"])?;
    Ok(PathBuf::from(out.trim()))
}

/// When git added each fragment, by file name; the latest add wins.
/// Empty when git can't tell (the order is by name then).
fn added_times(top: &Path) -> HashMap<String, i64> {
    let args = [
        "log",
        "--diff-filter=A",
        "--format=%x00%ct",
        "--name-only",
        "--",
        FRAGMENT_DIR,
    ];
    git(top, &args).map_or_else(|_| HashMap::new(), |log| parse_added(&log))
}

/// `git log --format=%x00%ct --name-only` output as file name to time,
/// keeping the first (newest) time each name appears with.
fn parse_added(log: &str) -> HashMap<String, i64> {
    let mut added = HashMap::new();
    let mut time = None;
    for line in log.lines() {
        if let Some(stamp) = line.strip_prefix('\0') {
            time = stamp.trim().parse::<i64>().ok();
        } else if let (Some(at), Some(name)) = (time, Path::new(line.trim()).file_name()) {
            added
                .entry(name.to_string_lossy().into_owned())
                .or_insert(at);
        }
    }
    added
}

fn git(dir: &Path, args: &[&str]) -> anyhow::Result<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .with_context(|| format!("running git {}", args.join(" ")))?;
    anyhow::ensure!(
        out.status.success(),
        "git {} in {}: {}",
        args.first().copied().unwrap_or_default(),
        dir.display(),
        String::from_utf8_lossy(&out.stderr).trim()
    );
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_newest_add_of_each_fragment_wins() {
        let log = "\u{0}300\n\nchangelog.d/b.md\n\u{0}200\n\nchangelog.d/a.md\nchangelog.d/b.md\n";
        let added = parse_added(log);
        assert_eq!(added.get("b.md"), Some(&300));
        assert_eq!(added.get("a.md"), Some(&200));
        assert_eq!(added.len(), 2);
    }
}
