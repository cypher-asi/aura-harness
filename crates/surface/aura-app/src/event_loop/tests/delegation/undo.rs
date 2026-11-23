//! What of a failed attempt is undone before the retry: in a git
//! workspace the changes of its shell commands too, outside git only
//! those of its file tools.

use super::*;
use aura_core_types::SubagentExit;
use std::path::{Path, PathBuf};

/// Run git with `args` in `dir` and return its output.
fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

fn read(path: PathBuf) -> String {
    std::fs::read_to_string(path).unwrap()
}

/// Seed `workspace` with the files "change files by shell" works on and
/// an ignored `target/out`.
fn seed_shell_files(workspace: &Path) {
    std::fs::write(workspace.join(".gitignore"), "target/\n").unwrap();
    std::fs::write(workspace.join("tracked.txt"), "base\n").unwrap();
    std::fs::write(workspace.join("gone.txt"), "keep\n").unwrap();
    std::fs::write(workspace.join("wip.txt"), "wip\n").unwrap();
    std::fs::create_dir(workspace.join("target")).unwrap();
    std::fs::write(workspace.join("target/out"), "build\n").unwrap();
}

/// Make `ws`, seeded with [`seed_shell_files`], a git repository with
/// those files committed, an edit stashed and another left uncommitted.
fn commit_shell_files(ws: &Path) {
    git(ws, &["init", "--quiet", "-b", "main"]);
    git(ws, &["config", "user.email", "test@example.com"]);
    git(ws, &["config", "user.name", "Test"]);
    git(ws, &["add", "."]);
    git(ws, &["commit", "--quiet", "-m", "seed"]);
    std::fs::write(ws.join("wip.txt"), "stashed\n").unwrap();
    git(ws, &["stash", "--quiet"]);
    std::fs::write(ws.join("wip.txt"), "wip, not committed\n").unwrap();
}

/// What git holds of `ws`: its HEAD, refs, stash list and index.
fn repository(ws: &Path) -> (String, String, String, Vec<u8>) {
    (
        git(ws, &["rev-parse", "HEAD"]),
        git(ws, &["for-each-ref"]),
        git(ws, &["stash", "list"]),
        std::fs::read(ws.join(".git/index")).unwrap(),
    )
}

#[tokio::test]
async fn in_a_git_workspace_a_failed_attempts_shell_changes_are_undone_before_the_retry() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let first = fixture.first;
    let mut state = loop_state(&fixture, &mut b);
    select_gate(&fixture, &mut state);
    fixture.hub.set_permissions(first, yolo());
    let ws = fixture.workspace.clone();
    seed_shell_files(&ws);
    commit_shell_files(&ws);
    let before = repository(&ws);

    let check = format!("ls shell-{GPT}.txt");
    let result = dispatch(
        &fixture,
        task(first, "implement", "change files by shell", Some(&check)),
    )
    .await;

    assert!(
        matches!(result.exit, SubagentExit::Completed),
        "{}",
        result.final_message
    );
    assert_eq!(children(&fixture, first, "implement").len(), 2, "one retry");
    assert!(result
        .final_message
        .contains(&format!("Check `{check}`: passed")));
    assert!(
        !ws.join(format!("shell-{SONNET}.txt")).exists(),
        "the first attempt's new file was removed"
    );
    assert!(ws.join(format!("shell-{GPT}.txt")).exists());
    assert_eq!(
        read(ws.join("tracked.txt")),
        format!("base\n{GPT}\n"),
        "the first attempt's edit was undone"
    );
    assert_eq!(read(ws.join("gone.txt")), "keep\n", "and its delete");
    assert_eq!(read(ws.join("wip.txt")), "wip, not committed\n");
    assert_eq!(
        read(ws.join("target/out")),
        format!("build\n{SONNET}\n{GPT}\n"),
        "ignored files are left alone"
    );
    assert_eq!(
        repository(&ws),
        before,
        "index, HEAD, refs and stash are untouched"
    );
    assert!(before.2.contains("stash@{0}"));
}

#[tokio::test]
async fn outside_git_only_file_tool_changes_are_undone_before_the_retry() {
    let (fixture, _commands) = fixture();
    let mut b = borrowed(&fixture);
    let first = fixture.first;
    let mut state = loop_state(&fixture, &mut b);
    select_gate(&fixture, &mut state);
    fixture.hub.set_permissions(first, yolo());
    let ws = fixture.workspace.clone();
    seed_shell_files(&ws);

    let check = format!("ls shell-{GPT}.txt");
    let result = dispatch(
        &fixture,
        task(first, "implement", "change files by shell", Some(&check)),
    )
    .await;

    assert!(result
        .final_message
        .contains(&format!("Check `{check}`: passed")));
    assert!(
        ws.join(format!("shell-{SONNET}.txt")).exists(),
        "a shell command's changes stay outside git"
    );
    assert_eq!(
        read(ws.join("tracked.txt")),
        format!("base\n{SONNET}\n{GPT}\n")
    );
    assert!(!ws.join("gone.txt").exists());
}
