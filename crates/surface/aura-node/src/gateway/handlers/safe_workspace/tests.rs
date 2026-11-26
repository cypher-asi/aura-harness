use super::checkpoints::{
    apply_to_source_blocking, list_checkpoints_blocking, restore_checkpoint_blocking,
    take_checkpoint,
};
use super::git::{local_git_command, run_git};
use super::setup::prepare_workspace_blocking;
use super::storage::session_root;
use super::WorkspaceMetadata;
use std::fs;
use std::path::Path;

fn git(cwd: &Path, args: &[&str]) {
    run_git(cwd, args).expect("git command should succeed");
}

fn fixture() -> (tempfile::TempDir, tempfile::TempDir, WorkspaceMetadata) {
    let source = tempfile::tempdir().expect("source tempdir");
    git(source.path(), &["init"]);
    fs::write(source.path().join("tracked.txt"), "baseline\n").unwrap();
    git(source.path(), &["add", "tracked.txt"]);
    git(
        source.path(),
        &[
            "-c",
            "user.name=Aura Test",
            "-c",
            "user.email=aura@test.invalid",
            "commit",
            "-m",
            "baseline",
        ],
    );
    fs::write(source.path().join("tracked.txt"), "dirty source\n").unwrap();
    fs::write(source.path().join("untracked.txt"), "copied\n").unwrap();

    let data = tempfile::tempdir().expect("data tempdir");
    let metadata =
        prepare_workspace_blocking(data.path(), "project-id", "session-id", source.path())
            .expect("prepare safe workspace");
    (source, data, metadata)
}

#[test]
fn local_git_command_allowlist_rejects_network_operations() {
    for subcommand in ["clone", "fetch", "pull", "push", "remote", "submodule"] {
        assert!(local_git_command(Path::new("."), &[subcommand]).is_err());
    }
    assert!(local_git_command(
        Path::new("."),
        &[
            "-c",
            "user.name=Aura Test",
            "-c",
            "user.email=aura@test.invalid",
            "commit",
            "--allow-empty",
        ],
    )
    .is_ok());
}

#[test]
fn initializes_an_imported_non_git_workspace_without_losing_files() {
    let source = tempfile::tempdir().unwrap();
    fs::write(source.path().join("app.txt"), "imported\n").unwrap();
    let data = tempfile::tempdir().unwrap();

    let metadata =
        prepare_workspace_blocking(data.path(), "project-id", "session-id", source.path())
            .expect("prepare imported workspace");

    assert_eq!(
        fs::read_to_string(metadata.workspace_root.join("app.txt")).unwrap(),
        "imported\n"
    );
    assert!(source.path().join(".git").is_dir());
    assert_eq!(
        fs::read_to_string(source.path().join("app.txt")).unwrap(),
        "imported\n"
    );
}

#[test]
fn restore_is_exact_and_apply_is_incremental() {
    let (source, data, metadata) = fixture();
    let baseline = list_checkpoints_blocking(&metadata).unwrap()[0].id.clone();
    fs::write(metadata.workspace_root.join("tracked.txt"), "first edit\n").unwrap();
    let root = session_root(data.path(), "project-id", "session-id");
    let first = apply_to_source_blocking(&root).unwrap();
    assert!(first.applied);
    assert_eq!(
        fs::read_to_string(source.path().join("tracked.txt")).unwrap(),
        "first edit\n"
    );

    let checkpoint = take_checkpoint(&metadata, "before bad edit").unwrap();
    fs::write(metadata.workspace_root.join("tracked.txt"), "bad edit\n").unwrap();
    let restored = restore_checkpoint_blocking(&root, &checkpoint).unwrap();
    assert!(!restored.undo_checkpoint_id.is_empty());
    assert_eq!(
        fs::read_to_string(metadata.workspace_root.join("tracked.txt")).unwrap(),
        "first edit\n"
    );

    fs::write(metadata.workspace_root.join("tracked.txt"), "second edit\n").unwrap();
    let second = apply_to_source_blocking(&root).unwrap();
    assert!(second.applied);
    assert_eq!(
        fs::read_to_string(source.path().join("tracked.txt")).unwrap(),
        "second edit\n"
    );
    assert!(!baseline.is_empty());
}
