//! Picking a run: its diff, and applying it to the user's folder, which
//! removes the copies.

use super::*;
use std::path::Path;

fn git_available() -> bool {
    std::process::Command::new("git")
        .arg("--version")
        .output()
        .is_ok_and(|out| out.status.success())
}

fn git(dir: &Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .current_dir(dir)
        .args(args)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

/// Makes `repo` a git repository whose `lib.txt` is committed, then edited.
fn seed_repo(repo: &Path) {
    git(repo, &["init", "--quiet", "-b", "main"]);
    git(repo, &["config", "user.email", "test@example.com"]);
    git(repo, &["config", "user.name", "Test"]);
    std::fs::write(repo.join("lib.txt"), "seed\n").unwrap();
    git(repo, &["add", "."]);
    git(repo, &["commit", "--quiet", "-m", "seed"]);
    std::fs::write(repo.join("lib.txt"), "seed, edited\n").unwrap();
}

/// The path of the first file of each diff shown, once `commands` are
/// drained.
fn diffs_shown(commands: &mut mpsc::Receiver<UiCommand>) -> Vec<String> {
    drain(commands)
        .into_iter()
        .filter_map(|(_, c)| match c {
            UiCommand::ShowDiff { files, .. } => Some(files[0].path.clone()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn apply_brings_the_chosen_run_home() {
    if !git_available() {
        return;
    }
    let (fixture, mut commands) = fixture_with_gate(steady_gate_without_delegation());
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut state = loop_state(&fixture, &mut b);
    state.models = gate_on_zos(&fixture);
    state.sync_turn_base();
    let repo = &fixture.workspace;
    seed_repo(repo);

    let args = "write the answer --repeats 1";
    let payload = run_comparison(&mut state, &links, &mut commands, args).await;
    let gate = payload.run("a-1").unwrap().clone();
    let other = payload.run("b-1").unwrap().clone();
    assert_eq!(gate.copy.git_dir, None, "a git repository gets a worktree");
    assert_eq!(
        std::fs::read_to_string(gate.copy.workdir.join("lib.txt")).unwrap(),
        "seed, edited\n",
        "uncommitted work is carried into the copy"
    );
    assert_eq!(gate.files_changed, 1, "only the run's own change counts");

    compare(&mut state, &links, "diff a-1").await;
    assert_eq!(diffs_shown(&mut commands), ["answer.txt"]);

    compare(&mut state, &links, "apply b-1").await;
    let applied = system_texts(&drain(&mut commands));
    assert!(applied.contains("applied b-1 (1 file)"), "{applied}");
    assert_eq!(
        std::fs::read_to_string(repo.join("answer.txt")).unwrap(),
        OPUS
    );
    assert_eq!(
        std::fs::read_to_string(repo.join("lib.txt")).unwrap(),
        "seed, edited\n"
    );
    assert!(!gate.copy.root.exists() && !other.copy.root.exists());
    assert_eq!(
        latest(&state).status,
        CompareStatus::Applied("b-1".to_string())
    );

    compare(&mut state, &links, "dismiss").await;
    assert!(system_texts(&drain(&mut commands)).contains("already removed"));
}
