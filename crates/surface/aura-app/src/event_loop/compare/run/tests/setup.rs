//! The example tasks' files, put in place for their runs, or not.

use super::super::setup::{fixture, set_up, FIXTURES};
use crate::bench_pack::{self, ExampleTask};
use crate::repo_source::{ImageSource, Source};
use aura_ui_terminal::events::BenchTaskSetup;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn a_task_whose_files_cannot_be_had_fails_alone() {
    let mut plain = bench_pack::tasks().iter().filter(|t| t.source.is_none());
    let (first, last) = (plain.next().unwrap(), plain.next().unwrap());
    let mut broken = first.clone();
    broken.id = "swe/broken".to_string();
    broken.source = Some(Source::Image(ImageSource {
        image: "not a reference".to_string(),
    }));
    let broken: &'static ExampleTask = Box::leak(Box::new(broken));
    let dir = tempfile::tempdir().unwrap();
    let examples = vec![first, broken, last];
    let (states, mut seen) = watch::channel(vec![BenchTaskSetup::Queued; 3]);
    set_up(
        examples.clone(),
        dir.path().to_path_buf(),
        states,
        CancellationToken::new(),
    )
    .await;

    let states = seen.borrow_and_update().clone();
    assert_eq!(states[0], BenchTaskSetup::Ready);
    assert!(
        matches!(&states[1], BenchTaskSetup::Failed(why) if why.contains("not a plain image reference")),
        "{states:?}"
    );
    assert_eq!(states[2], BenchTaskSetup::Ready);
    assert!(fixture(dir.path(), first).is_dir());
    assert!(fixture(dir.path(), last).is_dir());
}

#[tokio::test]
async fn setting_up_stops_with_the_comparison() {
    let dir = tempfile::tempdir().unwrap();
    let examples: Vec<_> = bench_pack::tasks()
        .iter()
        .filter(|t| t.source.is_none())
        .take(2)
        .collect();
    let (states, seen) = watch::channel(vec![BenchTaskSetup::Queued; 2]);
    let cancel = CancellationToken::new();
    cancel.cancel();
    set_up(examples, dir.path().to_path_buf(), states, cancel).await;
    assert!(seen.borrow().iter().all(|s| *s == BenchTaskSetup::Queued));
    assert!(!dir.path().join(FIXTURES).exists());
}
