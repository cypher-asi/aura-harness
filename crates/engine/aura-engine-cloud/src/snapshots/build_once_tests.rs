//! Agents started together share one project build, and a build the
//! machine failed is tried once more.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use super::tests::{builder, project};
use crate::fake::FakeProvider;
use aura_core_types::VmEvent;

#[tokio::test]
async fn starts_launched_together_share_one_project_build() {
    let dir = project();
    let home = tempfile::tempdir().expect("home");
    let fake = FakeProvider::new();
    let (builder, _) = builder(&fake, home.path());
    let start = || builder.ensure_project("p1", dir.path(), None, false);
    let (a, b, c) = tokio::join!(start(), start(), start());
    let (a, b, c) = (a.expect("a"), b.expect("b"), c.expect("c"));
    assert_eq!((&a, &b), (&b, &c), "all three use the one snapshot");
    assert_eq!(
        fake.calls_matching("SaveSnapshot(").len(),
        2,
        "the base and p1, once each"
    );
    let builds = fake
        .calls()
        .iter()
        .filter(|c| c.contains("cargo fetch"))
        .count();
    assert_eq!(builds, 1, "{:?}", fake.calls());
}

/// A fake whose project build fails with `stderr` the first `fails`
/// times; returns it and the count of builds.
fn failing_build(stderr: &'static str, fails: usize) -> (FakeProvider, Arc<AtomicUsize>) {
    let fake = FakeProvider::new();
    let builds = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&builds);
    fake.on_exec(Arc::new(move |_, command, _| {
        let failed =
            command.contains("cargo fetch") && counted.fetch_add(1, Ordering::SeqCst) < fails;
        crate::ExecOutput {
            exit_code: if failed { 101 } else { 0 },
            stdout: Vec::new(),
            stderr: if failed {
                stderr.as_bytes().to_vec()
            } else {
                Vec::new()
            },
        }
    }));
    (fake, builds)
}

#[tokio::test]
async fn a_build_killed_for_memory_is_tried_once_more_on_a_fresh_builder() {
    let dir = project();
    let home = tempfile::tempdir().expect("home");
    let killed = "error: could not compile `aura-app` (lib test)\n\nCaused by:\n  \
                  process didn't exit successfully: `rustc` (signal: 9, SIGKILL: kill)\n";
    let (fake, builds) = failing_build(killed, 1);
    let (builder, ledger) = builder(&fake, home.path());
    builder
        .ensure_project("p1", dir.path(), None, false)
        .await
        .expect("the second build");
    assert_eq!(builds.load(Ordering::SeqCst), 2);
    assert_eq!(
        ledger.events("0://cloud/project/p1"),
        vec![VmEvent::Error, VmEvent::SnapshotSaved]
    );
    assert!(fake.machine_names().is_empty(), "both builders are gone");
}

#[tokio::test]
async fn a_compile_error_is_not_tried_again_and_is_quoted() {
    let dir = project();
    let home = tempfile::tempdir().expect("home");
    let compile = "error[E0063]: missing field `run_started` in initializer of `TurnExtras`\n\
                   warning: unused dependency `serde_json`\n";
    let (fake, builds) = failing_build(compile, 2);
    let (builder, _) = builder(&fake, home.path());
    let err = builder
        .ensure_project("p1", dir.path(), None, false)
        .await
        .expect_err("a compile error");
    assert_eq!(builds.load(Ordering::SeqCst), 1);
    assert!(
        err.to_string().contains("missing field `run_started`"),
        "{err}"
    );
}

#[tokio::test]
async fn a_commit_that_failed_to_build_is_not_built_again_until_it_moves() {
    let dir = project();
    let home = tempfile::tempdir().expect("home");
    let compile = "error[E0063]: missing field `run_started` in initializer of `TurnExtras`\n";
    let (fake, builds) = failing_build(compile, 1);
    let (builder, _) = builder(&fake, home.path());
    builder
        .ensure_project("p1", dir.path(), None, false)
        .await
        .expect_err("the compile error");
    let again = builder
        .ensure_project("p1", dir.path(), None, false)
        .await
        .expect_err("refused");
    assert!(
        matches!(&again, crate::CloudError::BuildFailedBefore { error, .. } if error.contains("run_started")),
        "{again}"
    );
    assert_eq!(builds.load(Ordering::SeqCst), 1, "no second builder");

    // A new commit builds again.
    std::fs::write(dir.path().join("fix.rs"), "fixed").expect("write");
    for args in [&["add", "-A"][..], &["commit", "-qm", "fix"][..]] {
        assert!(std::process::Command::new("git")
            .arg("-C")
            .arg(dir.path())
            .args(args)
            .status()
            .expect("git")
            .success());
    }
    builder
        .ensure_project("p1", dir.path(), None, false)
        .await
        .expect("built at the fix");
    assert_eq!(builds.load(Ordering::SeqCst), 2);
}
